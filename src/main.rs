//! paperterm — terminal emulator for the reMarkable Paper Pro.
//! alacritty_terminal core + native qtfb client + PSF2 renderer + on-screen keyboard.

mod font;
mod hw_keyboard;
mod keys;
mod osk;
mod palette;
mod qtfb;
mod render;
mod screen;
mod selection;
mod session;
mod settings;
mod ui;

use std::io::ErrorKind;
use std::num::NonZeroUsize;
use std::os::fd::RawFd;
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::term::{Config, LineDamageBounds, Term, TermDamage};
use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};
use polling::{Event as PollEvent, Events, PollMode, Poller};

use crate::render::TermArea;
use crate::screen::{Canvas, Rotation, PANEL_H, PANEL_W};
use crate::selection::{Clipboard, Gesture};
use crate::session::Session;

const OSK_H: usize = 460;

/// The on-screen keyboard keeps its own font, so its labels always fit the keys.
const OSK_FONT_SIZE: usize = 36;

#[derive(Clone)]
struct Listener(mpsc::Sender<Event>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let _ = self.0.send(event);
    }
}

#[derive(Copy, Clone)]
pub(crate) struct GridSize {
    pub(crate) cols: usize,
    pub(crate) lines: usize,
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.lines
    }
    fn screen_lines(&self) -> usize {
        self.lines
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

struct FdSource(RawFd);
impl AsRawFd for FdSource {
    fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}

/// Union of dirty pixel rects for one frame.
#[derive(Default)]
struct DirtyRegion {
    rect: Option<(usize, usize, usize, usize)>,
}

impl DirtyRegion {
    fn add(&mut self, x: usize, y: usize, w: usize, h: usize) {
        self.rect = Some(match self.rect {
            None => (x, y, w, h),
            Some((rx, ry, rw, rh)) => {
                let x0 = rx.min(x);
                let y0 = ry.min(y);
                let x1 = (rx + rw).max(x + w);
                let y1 = (ry + rh).max(y + h);
                (x0, y0, x1 - x0, y1 - y0)
            }
        });
    }

    /// Returns whether anything was sent to the screen.
    fn flush(&mut self, client: &mut qtfb::QtfbClient, canvas: &Canvas, rotation: Rotation) -> bool {
        let Some(rect) = self.rect.take() else {
            return false;
        };
        let (x, y, w, h) = canvas.present(rotation, rect, client.framebuffer());
        let _ = client.update_partial(x as i32, y as i32, w as i32, h as i32);
        true
    }
}

fn term_area(font: &font::Font, screen_w: usize, term_h: usize) -> TermArea {
    TermArea {
        cols: screen_w / font.width,
        rows: term_h / font.height,
        cw: font.width,
        ch: font.height,
        x0: (screen_w % font.width) / 2,
        y0: 0,
    }
}

fn window_size(area: &TermArea) -> WindowSize {
    WindowSize {
        num_cols: area.cols as u16,
        num_lines: area.rows as u16,
        cell_width: area.cw as u16,
        cell_height: area.ch as u16,
    }
}

/// Fit the terminal and the shell to `area`.
fn resize_term(term: &mut Term<Listener>, session: &Session, area: &TermArea) {
    term.resize(GridSize {
        cols: area.cols,
        lines: area.rows,
    });
    let _ = session.resize(window_size(area));
    term.scroll_display(Scroll::Bottom);
}

fn single_instance_lock() -> bool {
    let path = std::ffi::CString::new("/tmp/paperterm.lock").unwrap();
    unsafe {
        let fd = libc::open(path.as_ptr(), libc::O_CREAT | libc::O_RDWR, 0o644);
        if fd < 0 {
            return true; // can't lock: proceed anyway
        }
        // Leak the fd deliberately: the lock lives as long as the process.
        libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) == 0
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let result = match session::holder_size(&args) {
        Some(size) => session::run_holder(size),
        None => run(),
    };
    if let Err(e) = result {
        eprintln!("paperterm: fatal: {e}");
        std::process::exit(1);
    }
}

fn run() -> std::io::Result<()> {
    let key: i32 = std::env::var("QTFB_KEY")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| {
            eprintln!("paperterm: QTFB_KEY not set (must be launched from AppLoad)");
            std::process::exit(2);
        });

    if !single_instance_lock() {
        eprintln!("paperterm: another instance is running, exiting");
        return Ok(());
    }

    let mut settings = settings::Settings::load();
    let mut font = font::Font::load(settings.font, settings.size, settings.crisp)
        .map_err(std::io::Error::other)?;
    let osk_font = font::Font::load(0, OSK_FONT_SIZE, false).map_err(std::io::Error::other)?;

    // ---- qtfb ----
    let mut client =
        qtfb::QtfbClient::connect(key, qtfb::FBFMT_RMPP_RGB565, PANEL_W, PANEL_H, 2)?;
    let mut rotation = client
        .wait_for_rotation(500)
        .and_then(Rotation::from_wire)
        .unwrap_or(Rotation::Deg0);
    let mut canvas = Canvas::new(rotation);
    canvas.dark = settings.dark;

    let mut osk_visible = !hw_keyboard::present();

    // Geometry: terminal on top, with the OSK band only when no hardware keyboard is present.
    let mut term_h = if osk_visible {
        canvas.h - OSK_H
    } else {
        canvas.h
    };
    let mut area = term_area(&font, canvas.w, term_h);
    eprintln!(
        "paperterm: starting, {}x{} cells, font {}x{}, key {}",
        area.cols, area.rows, font.width, font.height, key
    );

    // NOTE: server stalls this connection 1s.
    let _ = client.set_refresh_mode(settings::REFRESH_OPTIONS[settings.refresh].mode);

    // ---- terminal + shell session ----
    let size = GridSize {
        cols: area.cols,
        lines: area.rows,
    };
    let (event_tx, event_rx) = mpsc::channel();
    let mut term: Term<Listener> = Term::new(
        Config {
            scrolling_history: 5000,
            ..Config::default()
        },
        &size,
        Listener(event_tx),
    );
    let mut parser: Processor<StdSyncHandler> = Processor::new();

    let (session, replay) = Session::connect_or_start(window_size(&area))?;
    parser.advance(&mut term, &replay);
    // The shell already had the answers to any queries in the old output.
    while event_rx.try_recv().is_ok() {}
    let _ = session.resize(window_size(&area));

    // ---- polling: key 0 = session, key 2 = qtfb socket ----
    let poller = Arc::new(Poller::new()?);
    unsafe {
        poller.add_with_mode(
            &FdSource(session.raw_fd()),
            PollEvent::readable(0),
            PollMode::Level,
        )?;
        poller.add_with_mode(
            &FdSource(client.raw_fd()),
            PollEvent::readable(2),
            PollMode::Level,
        )?;
    }

    let sigterm = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::consts::SIGTERM, Arc::clone(&sigterm))?;
    signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&sigterm))?;

    let mut palette = palette::Palette::new(settings.dark);
    let mut kbd = osk::Osk::new(term_h, OSK_H, canvas.w);
    let mut physical = keys::PhysicalKeyboard::default();
    let mut ui = ui::Ui::new();

    // Initial paint: white screen, keyboard, full update.
    let (w, h) = (canvas.w, canvas.h);
    render::fill_rect(&mut canvas, 0, 0, w, h, render::WHITE);
    if osk_visible {
        kbd.draw_all(&mut canvas, &osk_font);
    }
    ui.draw_overlay(&mut canvas);
    canvas.present(rotation, (0, 0, canvas.w, canvas.h), client.framebuffer());
    client.update_all()?;

    let mut events = Events::with_capacity(NonZeroUsize::new(32).unwrap());
    let mut buf = [0u8; 65536];
    let mut activity = true; // force first render
    let mut dirty = DirtyRegion::default();
    // What the current press on the terminal area is doing.
    let mut gesture = Gesture::default();
    let mut clipboard = Clipboard::load();
    // The selection as last drawn.
    let mut shown_selection = None;
    let mut next_hwkbd_scan = Instant::now() + Duration::from_secs(2);
    // Redraw everything once the settings screen is out of the way.
    let mut repaint = false;
    let mut redraw_all_lines = true;
    let mut full_refresh_due = false;
    let mut updates_since_cleanup = 0u32;

    'main: loop {
        if sigterm.load(Ordering::Relaxed) {
            break 'main;
        }

        events.clear();
        let mut wait = Duration::from_millis(300);
        if let Some(deadline) = gesture.deadline() {
            wait = wait.min(deadline.saturating_duration_since(Instant::now()));
        }
        poller.wait(&mut events, Some(wait))?;
        if Instant::now() >= next_hwkbd_scan {
            next_hwkbd_scan = Instant::now() + Duration::from_secs(2);
            let has_hwkbd = hw_keyboard::present();
            if has_hwkbd == osk_visible {
                osk_visible = !has_hwkbd;
                if osk_visible {
                    term_h = canvas.h - OSK_H;
                    kbd = osk::Osk::new(term_h, OSK_H, canvas.w);
                } else {
                    term_h = canvas.h;
                    kbd.y0 = canvas.h;
                }
                area = term_area(&font, canvas.w, term_h);
                resize_term(&mut term, &session, &area);
                repaint = true;
            }
        }

        for ev in events.iter() {
            match ev.key {
                0 if ev.readable => loop {
                    match session.read(&mut buf) {
                        Ok(0) => break 'main, // the shell ended
                        Ok(n) => {
                            parser.advance(&mut term, &buf[..n]);
                            activity = true;
                        }
                        Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                        Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                        Err(_) => break 'main,
                    }
                },
                2 => {
                    let input = match client.drain_events() {
                        Ok(v) => v,
                        Err(_) => break 'main, // window closed
                    };
                    let turned = client.take_rotation().and_then(Rotation::from_wire);
                    if let Some(new) = turned.filter(|r| *r != rotation) {
                        rotation = new;
                        canvas = Canvas::new(rotation);
                        term_h = if osk_visible { canvas.h - OSK_H } else { canvas.h };
                        kbd = osk::Osk::new(term_h, OSK_H, canvas.w);
                        if !osk_visible {
                            kbd.y0 = canvas.h;
                        }
                        area = term_area(&font, canvas.w, term_h);
                        resize_term(&mut term, &session, &area);
                        gesture = Gesture::Idle;
                        repaint = true;
                        full_refresh_due = true;
                        if ui.settings_open() {
                            ui.draw_panel(&mut canvas, term_h);
                            dirty.add(0, 0, canvas.w, canvas.h);
                        }
                    }
                    for mut iev in input {
                        if matches!(
                            iev.input_type,
                            qtfb::INPUT_TOUCH_PRESS
                                | qtfb::INPUT_TOUCH_UPDATE
                                | qtfb::INPUT_TOUCH_RELEASE
                                | qtfb::INPUT_PEN_PRESS
                                | qtfb::INPUT_PEN_UPDATE
                                | qtfb::INPUT_PEN_RELEASE
                        ) {
                            (iev.x, iev.y) = rotation.to_canvas(iev.x, iev.y);
                        }
                        let command = match iev.input_type {
                            qtfb::INPUT_TOUCH_PRESS | qtfb::INPUT_PEN_PRESS => {
                                ui.on_press(iev.x, iev.y, &settings, (canvas.w, canvas.h))
                            }
                            // Typing doesn't reach the shell behind the settings screen.
                            qtfb::INPUT_BTN_PRESS if ui.settings_open() => {
                                match physical.press(iev.x) {
                                    Some((keys::KeyAction::Esc, _, _)) => ui.cancel_settings(),
                                    _ => ui::UiCommand::Nothing,
                                }
                            }
                            qtfb::INPUT_BTN_RELEASE if ui.settings_open() => {
                                physical.release(iev.x);
                                ui::UiCommand::Nothing
                            }
                            qtfb::INPUT_VKB_PRESS if ui.settings_open() => ui::UiCommand::Nothing,
                            _ => ui::UiCommand::Pass,
                        };
                        match command {
                            ui::UiCommand::Pass => handle_input(
                                iev,
                                &mut kbd,
                                &mut physical,
                                &mut canvas,
                                &mut client,
                                &mut term,
                                &session,
                                &osk_font,
                                &area,
                                &mut dirty,
                                &mut gesture,
                                &mut clipboard,
                                &mut activity,
                            )?,
                            ui::UiCommand::Nothing => {}
                            ui::UiCommand::MenuOpened => {
                                ui.draw_overlay(&mut canvas);
                                let (x, y, w, h) = ui.overlay_rect(canvas.w);
                                dirty.add(x, y, w, h);
                            }
                            ui::UiCommand::Closed => repaint = true,
                            ui::UiCommand::PanelChanged => {
                                ui.draw_panel(&mut canvas, term_h);
                                dirty.add(0, 0, canvas.w, canvas.h);
                            }
                            ui::UiCommand::RefreshScreen => {
                                repaint = true;
                                full_refresh_due = true;
                            }
                            ui::UiCommand::CleanNow => {
                                let _ = client.request_full_refresh();
                            }
                            ui::UiCommand::Apply(new) => {
                                if (new.font, new.size, new.crisp)
                                    != (settings.font, settings.size, settings.crisp)
                                {
                                    font = font::Font::load(new.font, new.size, new.crisp)
                                        .map_err(std::io::Error::other)?;
                                    area = term_area(&font, canvas.w, term_h);
                                    resize_term(&mut term, &session, &area);
                                }
                                if new.refresh != settings.refresh {
                                    let mode = settings::REFRESH_OPTIONS[new.refresh].mode;
                                    let _ = client.set_refresh_mode(mode);
                                }
                                if new.dark != settings.dark {
                                    palette = palette::Palette::new(new.dark);
                                }
                                settings = new;
                                if let Err(e) = settings.save() {
                                    eprintln!("paperterm: could not save settings: {e}");
                                }
                                updates_since_cleanup = 0;
                                repaint = true;
                                full_refresh_due = true;
                            }
                            ui::UiCommand::Copy => copy_selection(&mut term, &mut clipboard),
                            ui::UiCommand::Paste => {
                                paste(&mut term, &session, &clipboard)?;
                                repaint = true;
                            }
                            ui::UiCommand::Minimize => {
                                request_minimize();
                                repaint = true;
                            }
                            ui::UiCommand::Exit => break 'main,
                            ui::UiCommand::EndSession => {
                                let _ = session.end();
                                break 'main;
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        gesture.tick(Instant::now(), &mut term, &area);

        // Term-emitted events (query replies etc.).
        while let Ok(event) = event_rx.try_recv() {
            match event {
                Event::PtyWrite(text) => session.send_input(text.as_bytes())?,
                Event::Exit | Event::ChildExit(_) => break 'main,
                Event::Wakeup => activity = true,
                _ => {}
            }
        }

        if repaint && !ui.settings_open() {
            repaint = false;
            let (w, h) = (canvas.w, canvas.h);
            render::fill_rect(&mut canvas, 0, 0, w, h, render::WHITE);
            if osk_visible {
                kbd.draw_all(&mut canvas, &osk_font);
            }
            redraw_all_lines = true;
            activity = true;
            dirty.add(0, 0, w, h);
        }

        // Output arriving behind the settings screen is drawn when it closes.
        if activity && !ui.settings_open() {
            activity = false;
            let damage: Option<Vec<LineDamageBounds>> = match term.damage() {
                TermDamage::Full => None,
                TermDamage::Partial(_) if redraw_all_lines => None,
                TermDamage::Partial(it) => Some(it.collect()),
            };
            term.reset_damage();
            redraw_all_lines = false;

            let selected = term.selection.as_ref().and_then(|s| s.to_range(&term));
            let fb = &mut canvas;
            match damage {
                None => {
                    for row in 0..area.rows {
                        render::draw_line(fb, &font, &palette, &term, &area, row, 0, area.cols - 1, selected);
                    }
                    dirty.add(0, area.y0, fb.w, area.rows * area.ch);
                }
                Some(lines) => {
                    for b in lines {
                        if b.line >= area.rows || b.left > b.right {
                            continue;
                        }
                        let left = b.left.min(area.cols - 1);
                        let right = b.right.min(area.cols - 1);
                        render::draw_line(fb, &font, &palette, &term, &area, b.line, left, right, selected);
                        let (x, y, _, _) = area.cell_rect(left, b.line);
                        dirty.add(x, y, (right - left + 1) * area.cw, area.ch);
                    }
                }
            }
        }

        if !ui.settings_open() {
            // A selection changes without the terminal counting it as damage.
            let selected = term.selection.as_ref().and_then(|s| s.to_range(&term));
            let can_copy = selected.is_some() && !gesture.selecting();
            let mut rows = Vec::new();
            if selected != shown_selection {
                let offset = term.grid().display_offset();
                for range in [shown_selection, selected].into_iter().flatten() {
                    rows.extend(selection::visible_rows(range, offset, area.rows));
                }
                shown_selection = selected;
            }
            if can_copy != ui.can_copy {
                ui.can_copy = can_copy;
                let (x, y, w, h) = ui::copy_button(canvas.w);
                dirty.add(x, y, w, h);
                if !can_copy {
                    render::fill_rect(&mut canvas, x, y, w, h, render::WHITE);
                    rows.extend(0..=((y + h) / area.ch).min(area.rows - 1));
                }
            }
            rows.sort_unstable();
            rows.dedup();
            for row in rows {
                render::draw_line(&mut canvas, &font, &palette, &term, &area, row, 0, area.cols - 1, selected);
                dirty.add(0, area.y0 + row * area.ch, canvas.w, area.ch);
            }
        }

        if let Some(rect) = dirty.rect {
            if !ui.settings_open() && ui.overlay_touches(canvas.w, rect) {
                ui.draw_overlay(&mut canvas);
                let (x, y, w, h) = ui.overlay_rect(canvas.w);
                dirty.add(x, y, w, h);
            }
        }

        let dark = ui.dark(&settings);
        if dark != canvas.dark {
            canvas.dark = dark;
            dirty.add(0, 0, canvas.w, canvas.h);
        }

        if dirty.flush(&mut client, &canvas, rotation) && !ui.settings_open() {
            let cleanup = settings::CLEANUP_OPTIONS[settings.cleanup].1;
            updates_since_cleanup += 1;
            if cleanup > 0 && updates_since_cleanup >= cleanup {
                full_refresh_due = true;
            }
        }
        if full_refresh_due && !ui.settings_open() {
            full_refresh_due = false;
            updates_since_cleanup = 0;
            let _ = client.request_full_refresh();
        }
    }

    eprintln!("paperterm: exiting");
    client.terminate();
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn handle_input(
    ev: qtfb::InputEvent,
    kbd: &mut osk::Osk,
    physical: &mut keys::PhysicalKeyboard,
    canvas: &mut Canvas,
    client: &mut qtfb::QtfbClient,
    term: &mut Term<Listener>,
    session: &Session,
    font: &font::Font,
    area: &TermArea,
    dirty: &mut DirtyRegion,
    gesture: &mut Gesture,
    clipboard: &mut Clipboard,
    activity: &mut bool,
) -> std::io::Result<()> {
    match ev.input_type {
        // AppLoad system virtual keyboard (if the user opens it): press only.
        qtfb::INPUT_VKB_PRESS => {
            if let Some((action, ctrl, alt)) = keys::decode_vkb(ev.x) {
                let bytes = keys::encode(action, ctrl, alt, *term.mode());
                send_typed(&bytes, term, session, activity)?;
            }
        }
        // Physical keyboards arrive as bare Qt::Key values, focused window only.
        qtfb::INPUT_BTN_PRESS | qtfb::INPUT_BTN_RELEASE => {
            let typed = if ev.input_type == qtfb::INPUT_BTN_PRESS {
                physical.press(ev.x)
            } else {
                physical.release(ev.x)
            };
            if let Some((action, ctrl, alt)) = typed {
                if ctrl && !alt && physical.shift_held() {
                    match action {
                        keys::KeyAction::Char('c' | 'C') => {
                            copy_selection(term, clipboard);
                            return Ok(());
                        }
                        keys::KeyAction::Char('v' | 'V') => {
                            paste(term, session, clipboard)?;
                            *activity = true;
                            return Ok(());
                        }
                        _ => {}
                    }
                }
                let bytes = keys::encode(action, ctrl, alt, *term.mode());
                send_typed(&bytes, term, session, activity)?;
            }
        }
        qtfb::INPUT_TOUCH_PRESS | qtfb::INPUT_PEN_PRESS => {
            if ev.y as usize >= kbd.y0 {
                let osk_events = kbd.press(ev.dev_id, ev.x, ev.y, canvas, font);
                process_osk_events(osk_events, client, term, session, dirty, activity)?;
            } else {
                let pen = ev.input_type == qtfb::INPUT_PEN_PRESS;
                gesture.press(ev.dev_id, pen, ev.x, ev.y, Instant::now());
            }
        }
        qtfb::INPUT_TOUCH_UPDATE | qtfb::INPUT_PEN_UPDATE => {
            let pen = ev.input_type == qtfb::INPUT_PEN_UPDATE;
            if gesture.moved((ev.dev_id, pen), ev.x, ev.y, term, area) {
                *activity = true;
            }
        }
        qtfb::INPUT_TOUCH_RELEASE | qtfb::INPUT_PEN_RELEASE => {
            let pen = ev.input_type == qtfb::INPUT_PEN_RELEASE;
            gesture.release(ev.dev_id, pen, term);
            let osk_events = kbd.release(ev.dev_id, canvas, font);
            process_osk_events(osk_events, client, term, session, dirty, activity)?;
        }
        _ => {}
    }
    Ok(())
}

/// An app can't ask AppLoad for anything, so qmd/appload-minimize.qmd watches this
/// file for a new value.
fn request_minimize() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    if let Err(e) = std::fs::write("/tmp/paperterm-minimize", now.as_nanos().to_string()) {
        eprintln!("paperterm: could not ask to minimize: {e}");
    }
}

/// Typing clears a selection and returns to the live screen.
fn send_typed(
    bytes: &[u8],
    term: &mut Term<Listener>,
    session: &Session,
    activity: &mut bool,
) -> std::io::Result<()> {
    session.send_input(bytes)?;
    term.selection = None;
    if term.grid().display_offset() != 0 {
        term.scroll_display(Scroll::Bottom);
        *activity = true;
    }
    Ok(())
}

fn copy_selection(term: &mut Term<Listener>, clipboard: &mut Clipboard) {
    if let Some(text) = term.selection_to_string() {
        if let Err(e) = clipboard.set(text) {
            eprintln!("paperterm: could not save the clipboard: {e}");
        }
    }
    term.selection = None;
}

fn paste(term: &mut Term<Listener>, session: &Session, clipboard: &Clipboard) -> std::io::Result<()> {
    if let Some(text) = clipboard.text() {
        session.send_input(&selection::paste_bytes(text, *term.mode()))?;
        term.selection = None;
        term.scroll_display(Scroll::Bottom);
    }
    Ok(())
}

fn process_osk_events(
    events: Vec<osk::OskEvent>,
    client: &mut qtfb::QtfbClient,
    term: &mut Term<Listener>,
    session: &Session,
    dirty: &mut DirtyRegion,
    activity: &mut bool,
) -> std::io::Result<()> {
    for oev in events {
        match oev {
            osk::OskEvent::Send(action, ctrl, alt) => {
                let bytes = keys::encode(action, ctrl, alt, *term.mode());
                send_typed(&bytes, term, session, activity)?;
            }
            osk::OskEvent::FullRefresh => {
                let _ = client.request_full_refresh();
            }
            osk::OskEvent::Redraw(rects) => {
                for (x, y, w, h) in rects {
                    dirty.add(x, y, w, h);
                }
            }
        }
    }
    Ok(())
}

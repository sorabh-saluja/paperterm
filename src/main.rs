//! paperterm — terminal emulator for the reMarkable Paper Pro.
//! alacritty_terminal core + native qtfb client + PSF2 renderer + on-screen keyboard.

mod font;
mod hw_keyboard;
mod keys;
mod osk;
mod palette;
mod qtfb;
mod render;

use std::io::{ErrorKind, Read, Write};
use std::num::NonZeroUsize;
use std::os::fd::RawFd;
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event, EventListener, OnResize, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::term::{Config, LineDamageBounds, Term, TermDamage};
use alacritty_terminal::tty::{self, ChildEvent, EventedPty, EventedReadWrite, Options, Shell};
use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};
use polling::{Event as PollEvent, Events, PollMode, Poller};

use crate::render::{TermArea, SCREEN_W};

const SCREEN_H: usize = 2160;
const OSK_H: usize = 460;

const FONT_16X32: &[u8] = include_bytes!("../fonts/spleen-16x32.psfu");
const FONT_12X24: &[u8] = include_bytes!("../fonts/spleen-12x24.psfu");
const FONT_32X64: &[u8] = include_bytes!("../fonts/spleen-32x64.psfu");

#[derive(Clone)]
struct Listener(mpsc::Sender<Event>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let _ = self.0.send(event);
    }
}

#[derive(Copy, Clone)]
struct GridSize {
    cols: usize,
    lines: usize,
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

    fn flush(&mut self, client: &qtfb::QtfbClient) {
        if let Some((x, y, w, h)) = self.rect.take() {
            let _ = client.update_partial(x as i32, y as i32, w as i32, h as i32);
        }
    }
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
    if let Err(e) = run() {
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

    let font_data: &[u8] = match std::env::var("PAPERTERM_FONT").as_deref() {
        Ok("12x24") => FONT_12X24,
        Ok("32x64") => FONT_32X64,
        _ => FONT_16X32,
    };
    let font = font::Psf2Font::parse(font_data).map_err(std::io::Error::other)?;

    let mut osk_visible = !hw_keyboard::present();

    // Geometry: terminal on top, with the OSK band only when no hardware keyboard is present.
    let mut term_h = if osk_visible {
        SCREEN_H - OSK_H
    } else {
        SCREEN_H
    };
    let mut area = TermArea {
        cols: SCREEN_W / font.width,
        rows: term_h / font.height,
        cw: font.width,
        ch: font.height,
        x0: (SCREEN_W % font.width) / 2,
        y0: 0,
    };
    eprintln!(
        "paperterm: starting, {}x{} cells, font {}x{}, key {}",
        area.cols, area.rows, font.width, font.height, key
    );

    // ---- qtfb ----
    let mut client =
        qtfb::QtfbClient::connect(key, qtfb::FBFMT_RMPP_RGB565, SCREEN_W, SCREEN_H, 2)?;
    // Low-latency waveform for typing. NOTE: server stalls this connection 1s.
    let _ = client.set_refresh_mode(qtfb::REFRESH_MODE_FAST);

    // ---- terminal + pty ----
    tty::setup_env();
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

    let window_size = WindowSize {
        num_cols: area.cols as u16,
        num_lines: area.rows as u16,
        cell_width: font.width as u16,
        cell_height: font.height as u16,
    };
    let mut env = std::collections::HashMap::new();
    env.insert("TERM".into(), "xterm-256color".into());
    env.insert("HOME".into(), "/home/root".into());
    env.insert(
        "PATH".into(),
        "/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:/home/root:/home/root/node/bin".into(),
    );
    let shell_path = std::env::var("PAPERTERM_SHELL").unwrap_or_else(|_| "/usr/bin/bash".into());
    let pty_options = Options {
        shell: Some(Shell::new(shell_path, vec!["-l".into()])),
        working_directory: Some("/home/root".into()),
        drain_on_exit: false,
        env,
    };
    let mut pty = tty::new(&pty_options, window_size, 0)?;

    // ---- polling: key 0 = pty, key 1 = SIGCHLD pipe, key 2 = qtfb socket, 20+ = evdev keyboards ----
    let poller = Arc::new(Poller::new()?);
    unsafe {
        pty.register(&poller, PollEvent::readable(0), PollMode::Level)?;
        poller.add_with_mode(
            &FdSource(client.raw_fd()),
            PollEvent::readable(2),
            PollMode::Level,
        )?;
    }

    let sigterm = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::consts::SIGTERM, Arc::clone(&sigterm))?;
    signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&sigterm))?;

    let palette = palette::Palette::new();
    let mut kbd = osk::Osk::new(term_h, OSK_H);
    let mut physical = keys::PhysicalKeyboard::default();

    // Initial paint: white screen, keyboard, full update.
    render::fill_rect(
        client.framebuffer(),
        0,
        0,
        SCREEN_W,
        SCREEN_H,
        render::WHITE,
    );
    {
        let fb = client.framebuffer();
        if osk_visible {
            kbd.draw_all(fb, &font);
        }
    }
    client.update_all()?;

    let mut events = Events::with_capacity(NonZeroUsize::new(32).unwrap());
    let mut buf = [0u8; 65536];
    let mut activity = true; // force first render
    let mut dirty = DirtyRegion::default();
    // Touch-drag scrolling state on the terminal area.
    let mut drag: Option<(i32, i32)> = None; // (dev_id, last_y)
    let mut next_hwkbd_scan = Instant::now() + Duration::from_secs(2);

    'main: loop {
        if sigterm.load(Ordering::Relaxed) {
            break 'main;
        }

        events.clear();
        poller.wait(&mut events, Some(Duration::from_millis(300)))?;
        if Instant::now() >= next_hwkbd_scan {
            next_hwkbd_scan = Instant::now() + Duration::from_secs(2);
            let has_hwkbd = hw_keyboard::present();
            if has_hwkbd && osk_visible {
                osk_visible = false;
                kbd.y0 = SCREEN_H;
                term_h = SCREEN_H;
                area.rows = term_h / font.height;
                let new_size = GridSize {
                    cols: area.cols,
                    lines: area.rows,
                };
                term.resize(new_size);
                pty.on_resize(WindowSize {
                    num_cols: area.cols as u16,
                    num_lines: area.rows as u16,
                    cell_width: font.width as u16,
                    cell_height: font.height as u16,
                });
                term.scroll_display(Scroll::Bottom);
                render::fill_rect(
                    client.framebuffer(),
                    0,
                    0,
                    SCREEN_W,
                    SCREEN_H,
                    render::WHITE,
                );
                dirty.add(0, 0, SCREEN_W, SCREEN_H);
                activity = true;
            } else if !has_hwkbd && !osk_visible {
                osk_visible = true;
                term_h = SCREEN_H - OSK_H;
                area.rows = term_h / font.height;
                let new_size = GridSize {
                    cols: area.cols,
                    lines: area.rows,
                };
                term.resize(new_size);
                pty.on_resize(WindowSize {
                    num_cols: area.cols as u16,
                    num_lines: area.rows as u16,
                    cell_width: font.width as u16,
                    cell_height: font.height as u16,
                });
                kbd = osk::Osk::new(term_h, OSK_H);
                term.scroll_display(Scroll::Bottom);
                let fb = client.framebuffer();
                render::fill_rect(fb, 0, 0, SCREEN_W, SCREEN_H, render::WHITE);
                kbd.draw_all(fb, &font);
                dirty.add(0, 0, SCREEN_W, SCREEN_H);
                activity = true;
            }
        }

        for ev in events.iter() {
            match ev.key {
                1 => {
                    if let Some(ChildEvent::Exited(_)) = pty.next_child_event() {
                        break 'main;
                    }
                }
                0 if ev.readable => loop {
                    match pty.reader().read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            parser.advance(&mut term, &buf[..n]);
                            activity = true;
                        }
                        Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                        Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                        Err(_) => break, // EIO: child gone; Exited event follows
                    }
                },
                2 => {
                    let input = match client.drain_events() {
                        Ok(v) => v,
                        Err(_) => break 'main, // window closed
                    };
                    for iev in input {
                        handle_input(
                            iev,
                            &mut kbd,
                            &mut physical,
                            &mut client,
                            &mut term,
                            &mut pty,
                            &font,
                            &area,
                            &mut dirty,
                            &mut drag,
                            &mut activity,
                        )?;
                    }
                }
                _ => {}
            }
        }

        // Term-emitted events (query replies etc.).
        while let Ok(event) = event_rx.try_recv() {
            match event {
                Event::PtyWrite(text) => write_pty(&mut pty, text.as_bytes())?,
                Event::Exit | Event::ChildExit(_) => break 'main,
                Event::Wakeup => activity = true,
                _ => {}
            }
        }

        if activity {
            activity = false;
            let damage: Option<Vec<LineDamageBounds>> = match term.damage() {
                TermDamage::Full => None,
                TermDamage::Partial(it) => Some(it.collect()),
            };
            term.reset_damage();

            let fb = client.framebuffer();
            match damage {
                None => {
                    for row in 0..area.rows {
                        render::draw_line(fb, &font, &palette, &term, &area, row, 0, area.cols - 1);
                    }
                    dirty.add(0, area.y0, SCREEN_W, area.rows * area.ch);
                }
                Some(lines) => {
                    for b in lines {
                        if b.line >= area.rows || b.left > b.right {
                            continue;
                        }
                        let left = b.left.min(area.cols - 1);
                        let right = b.right.min(area.cols - 1);
                        render::draw_line(fb, &font, &palette, &term, &area, b.line, left, right);
                        let (x, y, _, _) = area.cell_rect(left, b.line);
                        dirty.add(x, y, (right - left + 1) * area.cw, area.ch);
                    }
                }
            }
        }

        dirty.flush(&client);
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
    client: &mut qtfb::QtfbClient,
    term: &mut Term<Listener>,
    pty: &mut tty::Pty,
    font: &font::Psf2Font,
    area: &TermArea,
    dirty: &mut DirtyRegion,
    drag: &mut Option<(i32, i32)>,
    activity: &mut bool,
) -> std::io::Result<()> {
    match ev.input_type {
        // AppLoad system virtual keyboard (if the user opens it): press only.
        qtfb::INPUT_VKB_PRESS => {
            if let Some((action, ctrl, alt)) = keys::decode_vkb(ev.x) {
                let bytes = keys::encode(action, ctrl, alt, *term.mode());
                write_pty(pty, &bytes)?;
                if term.grid().display_offset() != 0 {
                    term.scroll_display(Scroll::Bottom);
                    *activity = true;
                }
            }
        }
        // Physical keyboards arrive as bare Qt::Key values, focused window only.
        qtfb::INPUT_BTN_PRESS => {
            if let Some((action, ctrl, alt)) = physical.press(ev.x) {
                let bytes = keys::encode(action, ctrl, alt, *term.mode());
                write_pty(pty, &bytes)?;
                if term.grid().display_offset() != 0 {
                    term.scroll_display(Scroll::Bottom);
                    *activity = true;
                }
            }
        }
        qtfb::INPUT_BTN_RELEASE => physical.release(ev.x),
        qtfb::INPUT_TOUCH_PRESS | qtfb::INPUT_PEN_PRESS => {
            if ev.y as usize >= kbd.y0 {
                let osk_events = {
                    let fb = client.framebuffer();
                    kbd.press(ev.dev_id, ev.x, ev.y, fb, font)
                };
                process_osk_events(osk_events, client, term, pty, dirty, activity)?;
            } else {
                *drag = Some((ev.dev_id, ev.y));
            }
        }
        qtfb::INPUT_TOUCH_UPDATE => {
            if let Some((id, last_y)) = *drag {
                if id == ev.dev_id {
                    let dy = ev.y - last_y;
                    let lines = dy / area.ch as i32;
                    if lines != 0 {
                        // Drag down => view earlier history (positive Delta).
                        term.scroll_display(Scroll::Delta(lines));
                        *drag = Some((id, last_y + lines * area.ch as i32));
                        *activity = true;
                    }
                }
            }
        }
        qtfb::INPUT_TOUCH_RELEASE | qtfb::INPUT_PEN_RELEASE => {
            if drag.map(|(id, _)| id) == Some(ev.dev_id) {
                *drag = None;
            }
            let osk_events = {
                let fb = client.framebuffer();
                kbd.release(ev.dev_id, fb, font)
            };
            process_osk_events(osk_events, client, term, pty, dirty, activity)?;
        }
        _ => {}
    }
    Ok(())
}

fn process_osk_events(
    events: Vec<osk::OskEvent>,
    client: &mut qtfb::QtfbClient,
    term: &mut Term<Listener>,
    pty: &mut tty::Pty,
    dirty: &mut DirtyRegion,
    activity: &mut bool,
) -> std::io::Result<()> {
    for oev in events {
        match oev {
            osk::OskEvent::Send(action, ctrl, alt) => {
                let bytes = keys::encode(action, ctrl, alt, *term.mode());
                write_pty(pty, &bytes)?;
                if term.grid().display_offset() != 0 {
                    term.scroll_display(Scroll::Bottom);
                    *activity = true;
                }
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

fn write_pty(pty: &mut tty::Pty, bytes: &[u8]) -> std::io::Result<()> {
    let mut written = 0;
    while written < bytes.len() {
        match pty.writer().write(&bytes[written..]) {
            Ok(n) => written += n,
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

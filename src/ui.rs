//! The floating menu button, its drop-down, and the settings screen. All of it
//! is drawn into our own framebuffer, so it also works in AppLoad's full screen.

use crate::font::{Font, DEFAULT_SIZE, FONTS};
use crate::render::{self, BLACK, WHITE};
use crate::screen::{Canvas, Rect};
#[cfg(test)]
use crate::screen::{Rotation, PANEL_H, PANEL_W};
use crate::settings::{Settings, CLEANUP_OPTIONS, REFRESH_OPTIONS};

const MARGIN: usize = 48;
const GAP: usize = 20;
/// Width of the controls on a sideways screen; the preview takes the rest.
const LANDSCAPE_CONTROLS_W: usize = 1180;
const BUTTON_H: usize = 100;

const MENU_W: usize = 500;
const MENU_ROW_H: usize = 112;
const MENU_ITEMS: [(&str, MenuItem); 4] = [
    ("Settings", MenuItem::Settings),
    ("Refresh screen", MenuItem::Refresh),
    ("Exit PaperTerm", MenuItem::Exit),
    ("End session", MenuItem::EndSession),
];

fn pill(w: usize) -> Rect {
    (w - 196, 12, 180, 76)
}

fn menu(w: usize) -> Rect {
    let p = pill(w);
    (w - 16 - MENU_W, p.1 + p.3 + 8, MENU_W, MENU_ROW_H * MENU_ITEMS.len())
}

const PREVIEW_LINES: [&str; 7] = [
    "The quick brown fox jumps over the lazy dog",
    "0123456789 {}[]()<>=+-*/\\|~",
    "root@ferrari:~# ls -la /home/root",
    "┌────────┬───────┐",
    "│ paper  │  pro  │",
    "└────────┴───────┘",
    "Il1| O0 rn m ,.;:",
];

#[derive(Clone, Copy)]
enum MenuItem {
    Settings,
    Refresh,
    Exit,
    EndSession,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Hit {
    Cancel,
    Done,
    Font(usize),
    Smaller,
    Bigger,
    Text(bool),
    Refresh(usize),
    Cleanup(usize),
    CleanNow,
}

/// What the main loop must do after a press.
pub enum UiCommand {
    /// Not ours: the terminal or the on-screen keyboard handles it.
    Pass,
    Nothing,
    MenuOpened,
    /// The menu or settings closed: repaint the terminal.
    Closed,
    /// The settings screen opened or changed: redraw it.
    PanelChanged,
    /// Repaint, then run a full refresh.
    RefreshScreen,
    CleanNow,
    Apply(Settings),
    Exit,
    /// Close the shell too, instead of leaving it running for next time.
    EndSession,
}

struct Panel {
    draft: Settings,
    preview: Font,
}

enum Screen {
    Terminal,
    Menu,
    Settings(Box<Panel>),
}

pub struct Ui {
    screen: Screen,
    label: Font,
    title: Font,
    /// One per entry in FONTS, so each button shows its name in its own face.
    samples: Vec<Font>,
}

fn contains((x, y, w, h): Rect, px: i32, py: i32) -> bool {
    let (px, py) = (px as usize, py as usize);
    px >= x && px < x + w && py >= y && py < y + h
}

fn intersects(a: Rect, b: Rect) -> bool {
    a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3
}

fn draw_button(fb: &mut Canvas, font: &Font, rect: Rect, label: &str, selected: bool) {
    let (x, y, w, h) = rect;
    let (fg, bg) = if selected { (WHITE, BLACK) } else { (BLACK, WHITE) };
    render::fill_rect(fb, x, y, w, h, bg);
    let b = 3;
    render::fill_rect(fb, x, y, w, b, BLACK);
    render::fill_rect(fb, x, y + h - b, w, b, BLACK);
    render::fill_rect(fb, x, y, b, h, BLACK);
    render::fill_rect(fb, x + w - b, y, b, h, BLACK);
    let text_w = label.chars().count() * font.width;
    let tx = x + w.saturating_sub(text_w) / 2;
    let ty = y + h.saturating_sub(font.height) / 2;
    render::draw_text(fb, font, tx, ty, label, fg, bg);
}

fn controls_w(w: usize, h: usize) -> usize {
    if w > h { LANDSCAPE_CONTROLS_W } else { w - 2 * MARGIN }
}

/// `count` equal buttons side by side across the controls' width.
fn row(content_w: usize, y: usize, count: usize, i: usize) -> Rect {
    let w = (content_w - GAP * (count - 1)) / count;
    (MARGIN + i * (w + GAP), y, w, BUTTON_H)
}

// Vertical layout of the settings screen.
const HEADER_Y: usize = 36;
const FONT_LABEL_Y: usize = 160;
const FONT_Y: usize = 212;
const SIZE_LABEL_Y: usize = FONT_Y + 2 * BUTTON_H + GAP + 24;
const SIZE_Y: usize = SIZE_LABEL_Y + 52;
const TEXT_LABEL_Y: usize = SIZE_Y + BUTTON_H + 24;
const TEXT_Y: usize = TEXT_LABEL_Y + 52;
const REFRESH_LABEL_Y: usize = TEXT_Y + BUTTON_H + 24;
const REFRESH_Y: usize = REFRESH_LABEL_Y + 52;
const HINT_Y: usize = REFRESH_Y + BUTTON_H + 12;
const CLEANUP_LABEL_Y: usize = HINT_Y + 60;
const CLEANUP_Y: usize = CLEANUP_LABEL_Y + 52;
const PREVIEW_LABEL_Y: usize = CLEANUP_Y + BUTTON_H + 24;
const FONT_COLUMNS: usize = 3;

fn font_rect(content_w: usize, i: usize) -> Rect {
    let (x, _, w, h) = row(content_w, 0, FONT_COLUMNS, i % FONT_COLUMNS);
    (x, FONT_Y + (i / FONT_COLUMNS) * (h + GAP), w, h)
}

/// Where the "Preview" heading sits, and the box under it.
fn preview_area(w: usize, h: usize) -> (usize, usize, Rect) {
    if w > h {
        let x = MARGIN + LANDSCAPE_CONTROLS_W + GAP * 2;
        (x, FONT_LABEL_Y, (x, FONT_Y, w - MARGIN - x, h - 36 - FONT_Y))
    } else {
        let y = PREVIEW_LABEL_Y + 52;
        (MARGIN, PREVIEW_LABEL_Y, (MARGIN, y, w - 2 * MARGIN, h - 36 - y))
    }
}

fn panel_buttons(w: usize, h: usize) -> Vec<(Rect, Hit)> {
    let content = controls_w(w, h);
    let mut out = vec![
        ((w - MARGIN - 440 - GAP, HEADER_Y, 220, 92), Hit::Cancel),
        ((w - MARGIN - 220, HEADER_Y, 220, 92), Hit::Done),
        ((MARGIN, SIZE_Y, 140, BUTTON_H), Hit::Smaller),
        ((MARGIN + 340, SIZE_Y, 140, BUTTON_H), Hit::Bigger),
        (row(content, TEXT_Y, 2, 0), Hit::Text(false)),
        (row(content, TEXT_Y, 2, 1), Hit::Text(true)),
    ];
    for i in 0..FONTS.len() {
        out.push((font_rect(content, i), Hit::Font(i)));
    }
    for i in 0..REFRESH_OPTIONS.len() {
        out.push((row(content, REFRESH_Y, REFRESH_OPTIONS.len(), i), Hit::Refresh(i)));
    }
    let cleanup_columns = CLEANUP_OPTIONS.len() + 1;
    for i in 0..CLEANUP_OPTIONS.len() {
        out.push((row(content, CLEANUP_Y, cleanup_columns, i), Hit::Cleanup(i)));
    }
    out.push((row(content, CLEANUP_Y, cleanup_columns, cleanup_columns - 1), Hit::CleanNow));
    out
}

impl Panel {
    fn new(settings: &Settings) -> Self {
        Self {
            draft: settings.clone(),
            preview: load_preview(settings),
        }
    }

    fn step_size(&mut self, bigger: bool) {
        let sizes = FONTS[self.draft.font].sizes();
        let at = sizes.iter().position(|&s| s == self.draft.size).unwrap_or(0);
        let next = if bigger { (at + 1).min(sizes.len() - 1) } else { at.saturating_sub(1) };
        self.draft.size = sizes[next];
    }

    fn reload_preview(&mut self) {
        self.draft.size = FONTS[self.draft.font].nearest_size(self.draft.size);
        self.preview = load_preview(&self.draft);
    }
}

fn load_preview(s: &Settings) -> Font {
    Font::load(s.font, s.size, s.crisp).expect("bundled fonts parse")
}

impl Ui {
    pub fn new() -> Self {
        Self {
            screen: Screen::Terminal,
            label: Font::load(0, 40, false).expect("bundled fonts parse"),
            title: Font::load(0, DEFAULT_SIZE + 16, false).expect("bundled fonts parse"),
            samples: (0..FONTS.len())
                .map(|i| Font::load(i, FONTS[i].nearest_size(40), false).expect("bundled fonts parse"))
                .collect(),
        }
    }

    pub fn settings_open(&self) -> bool {
        matches!(self.screen, Screen::Settings(_))
    }

    /// Close the settings screen without keeping changes (Esc on the keyboard).
    pub fn cancel_settings(&mut self) -> UiCommand {
        self.screen = Screen::Terminal;
        UiCommand::RefreshScreen
    }

    /// `screen` is the canvas's width and height.
    pub fn on_press(&mut self, x: i32, y: i32, settings: &Settings, screen: (usize, usize)) -> UiCommand {
        let (w, h) = screen;
        let menu_box = menu(w);
        match &mut self.screen {
            Screen::Terminal => {
                if contains(pill(w), x, y) {
                    self.screen = Screen::Menu;
                    UiCommand::MenuOpened
                } else {
                    UiCommand::Pass
                }
            }
            Screen::Menu => {
                self.screen = Screen::Terminal;
                let item = MENU_ITEMS.iter().enumerate().find_map(|(i, (_, item))| {
                    let rect = (menu_box.0, menu_box.1 + i * MENU_ROW_H, MENU_W, MENU_ROW_H);
                    contains(rect, x, y).then_some(*item)
                });
                match item {
                    Some(MenuItem::Settings) => {
                        self.screen = Screen::Settings(Box::new(Panel::new(settings)));
                        UiCommand::PanelChanged
                    }
                    Some(MenuItem::Refresh) => UiCommand::RefreshScreen,
                    Some(MenuItem::Exit) => UiCommand::Exit,
                    Some(MenuItem::EndSession) => UiCommand::EndSession,
                    None => UiCommand::Closed,
                }
            }
            Screen::Settings(panel) => {
                let Some((_, hit)) = panel_buttons(w, h).into_iter().find(|(r, _)| contains(*r, x, y)) else {
                    return UiCommand::Nothing;
                };
                match hit {
                    Hit::Cancel => {
                        self.screen = Screen::Terminal;
                        return UiCommand::RefreshScreen;
                    }
                    Hit::Done => {
                        let draft = panel.draft.clone();
                        self.screen = Screen::Terminal;
                        return UiCommand::Apply(draft);
                    }
                    Hit::CleanNow => return UiCommand::CleanNow,
                    Hit::Font(i) => {
                        panel.draft.font = i;
                        panel.reload_preview();
                    }
                    Hit::Smaller | Hit::Bigger => {
                        panel.step_size(hit == Hit::Bigger);
                        panel.reload_preview();
                    }
                    Hit::Text(crisp) => {
                        panel.draft.crisp = crisp;
                        panel.reload_preview();
                    }
                    Hit::Refresh(i) => panel.draft.refresh = i,
                    Hit::Cleanup(i) => panel.draft.cleanup = i,
                }
                UiCommand::PanelChanged
            }
        }
    }

    /// The part of the screen the floating controls cover.
    pub fn overlay_rect(&self, w: usize) -> Rect {
        let p = pill(w);
        match self.screen {
            Screen::Menu => {
                let m = menu(w);
                (m.0, p.1, m.2, m.1 + m.3 - p.1)
            }
            _ => p,
        }
    }

    pub fn overlay_touches(&self, w: usize, dirty: Rect) -> bool {
        intersects(self.overlay_rect(w), dirty)
    }

    pub fn draw_overlay(&self, fb: &mut Canvas) {
        let open = matches!(self.screen, Screen::Menu);
        draw_button(fb, &self.label, pill(fb.w), "Menu", open);
        if open {
            let m = menu(fb.w);
            for (i, (label, _)) in MENU_ITEMS.iter().enumerate() {
                let rect = (m.0, m.1 + i * MENU_ROW_H, MENU_W, MENU_ROW_H);
                draw_button(fb, &self.label, rect, label, false);
            }
        }
    }

    /// Draw the settings screen. `term_h` is the height the terminal would have.
    pub fn draw_panel(&self, fb: &mut Canvas, term_h: usize) {
        let Screen::Settings(panel) = &self.screen else { return };
        let s = &panel.draft;
        let (w, h) = (fb.w, fb.h);
        render::fill_rect(fb, 0, 0, w, h, WHITE);

        let heading = |fb: &mut Canvas, y: usize, text: &str| {
            render::draw_text(fb, &self.label, MARGIN, y, text, BLACK, WHITE);
        };
        render::draw_text(fb, &self.title, MARGIN, HEADER_Y + 12, "Settings", BLACK, WHITE);
        heading(fb, FONT_LABEL_Y, "Font");
        heading(fb, SIZE_LABEL_Y, "Size");
        heading(fb, TEXT_LABEL_Y, "Text style");
        heading(fb, REFRESH_LABEL_Y, "Refresh mode");
        heading(fb, CLEANUP_LABEL_Y, "Automatic ghost cleanup");
        let (preview_x, preview_y, preview_box) = preview_area(w, h);
        render::draw_text(fb, &self.label, preview_x, preview_y, "Preview", BLACK, WHITE);

        for (rect, hit) in panel_buttons(w, h) {
            let (font, label, selected) = match hit {
                Hit::Cancel => (&self.label, "Cancel", false),
                Hit::Done => (&self.label, "Done", true),
                Hit::Font(i) => (&self.samples[i], FONTS[i].name, s.font == i),
                Hit::Smaller => (&self.title, "-", false),
                Hit::Bigger => (&self.title, "+", false),
                Hit::Text(false) => (&self.label, "Smooth", !s.crisp),
                Hit::Text(true) => (&self.label, "Crisp", s.crisp),
                Hit::Refresh(i) => (&self.label, REFRESH_OPTIONS[i].label, s.refresh == i),
                Hit::Cleanup(i) => (&self.label, CLEANUP_OPTIONS[i].0, s.cleanup == i),
                Hit::CleanNow => (&self.label, "Clean now", false),
            };
            draw_button(fb, font, rect, label, selected);
        }

        let size = s.size.to_string();
        let size_x = MARGIN + 140 + (200 - size.len() * self.title.width) / 2;
        render::draw_text(fb, &self.title, size_x, SIZE_Y + 18, &size, BLACK, WHITE);
        let grid = format!(
            "{} columns x {} rows",
            w / panel.preview.width,
            term_h / panel.preview.height
        );
        render::draw_text(fb, &self.label, MARGIN + 520, SIZE_Y + 30, &grid, BLACK, WHITE);
        render::draw_text(fb, &self.label, MARGIN, HINT_Y, REFRESH_OPTIONS[s.refresh].hint, BLACK, WHITE);

        draw_preview(fb, &panel.preview, preview_box);
    }
}

fn draw_preview(fb: &mut Canvas, font: &Font, area: Rect) {
    let (x, y, w, h) = area;
    draw_button(fb, font, area, "", false);
    let pad = 24;
    let cols = (w - 2 * pad) / font.width;
    let lines = (h - 2 * pad) / font.height;
    for (i, line) in PREVIEW_LINES.iter().take(lines).enumerate() {
        let text: String = line.chars().take(cols).collect();
        render::draw_text(fb, font, x + pad, y + pad + i * font.height, &text, BLACK, WHITE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREENS: [(usize, usize); 2] = [(PANEL_W, PANEL_H), (PANEL_H, PANEL_W)];

    #[test]
    fn every_button_is_on_screen_and_none_overlap() {
        for (w, h) in SCREENS {
            let buttons = panel_buttons(w, h);
            let preview = preview_area(w, h).2;
            assert!(preview.0 + preview.2 <= w && preview.1 + preview.3 <= h);
            for (i, (a, hit)) in buttons.iter().enumerate() {
                assert!(a.0 + a.2 <= w && a.1 + a.3 <= h, "{hit:?} off screen at {w}x{h}");
                assert!(!intersects(*a, preview), "{hit:?} over preview at {w}x{h}");
                for (b, other) in &buttons[i + 1..] {
                    assert!(!intersects(*a, *b), "{hit:?} overlaps {other:?} at {w}x{h}");
                }
            }
        }
    }

    #[test]
    fn menu_flow_opens_settings_and_cancel_returns() {
        for screen in SCREENS {
            let (w, h) = screen;
            let settings = Settings::default();
            let mut ui = Ui::new();
            assert!(matches!(ui.on_press(10, 1000, &settings, screen), UiCommand::Pass));
            let p = pill(w);
            assert!(matches!(ui.on_press(p.0 as i32 + 1, p.1 as i32 + 1, &settings, screen), UiCommand::MenuOpened));
            let m = menu(w);
            assert!(matches!(
                ui.on_press(m.0 as i32 + 1, m.1 as i32 + 1, &settings, screen),
                UiCommand::PanelChanged
            ));
            assert!(ui.settings_open());
            let cancel = panel_buttons(w, h).into_iter().find(|(_, x)| *x == Hit::Cancel).unwrap().0;
            assert!(matches!(
                ui.on_press(cancel.0 as i32 + 1, cancel.1 as i32 + 1, &settings, screen),
                UiCommand::RefreshScreen
            ));
            assert!(!ui.settings_open());
        }
    }

    #[test]
    fn size_buttons_change_the_draft_and_stop_at_the_ends() {
        let settings = Settings::default();
        let mut panel = Panel::new(&settings);
        panel.step_size(true);
        assert!(panel.draft.size > settings.size);
        for _ in 0..100 {
            panel.step_size(false);
        }
        assert_eq!(panel.draft.size, FONTS[0].sizes()[0]);
    }

    /// Writes raw RGB565 screens, as the panel receives them, for checking the
    /// layout by eye: PAPERTERM_DUMP_DIR=/tmp/out cargo test dump_screens
    #[test]
    fn dump_screens() {
        let Ok(dir) = std::env::var("PAPERTERM_DUMP_DIR") else { return };
        let settings = Settings::default();
        for (name, rotation) in [("portrait", Rotation::Deg0), ("landscape", Rotation::Deg90L)] {
            let mut fb = Canvas::new(rotation);
            let screen = (fb.w, fb.h);
            let mut ui = Ui::new();
            let term = Font::load(settings.font, settings.size, false).unwrap();
            for (i, line) in PREVIEW_LINES.iter().enumerate() {
                render::draw_text(&mut fb, &term, 0, i * term.height, line, BLACK, WHITE);
            }
            ui.draw_overlay(&mut fb);
            std::fs::write(format!("{dir}/{name}-terminal.raw"), &fb.px).unwrap();

            let p = pill(fb.w);
            ui.on_press(p.0 as i32 + 1, p.1 as i32 + 1, &settings, screen);
            ui.draw_overlay(&mut fb);
            std::fs::write(format!("{dir}/{name}-menu.raw"), &fb.px).unwrap();

            let m = menu(fb.w);
            ui.on_press(m.0 as i32 + 1, m.1 as i32 + 1, &settings, screen);
            ui.draw_panel(&mut fb, screen.1);
            std::fs::write(format!("{dir}/{name}-settings.raw"), &fb.px).unwrap();
        }
    }
}

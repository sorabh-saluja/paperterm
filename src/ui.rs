//! The floating menu button, its drop-down, and the settings screen. All of it
//! is drawn into our own framebuffer, so it also works in AppLoad's full screen.

use crate::font::{Font, DEFAULT_SIZE, FONTS};
use crate::render::{self, BLACK, SCREEN_W, WHITE};
use crate::settings::{Settings, CLEANUP_OPTIONS, REFRESH_OPTIONS};

pub type Rect = (usize, usize, usize, usize);

const SCREEN_H: usize = 2160;
const MARGIN: usize = 48;
const GAP: usize = 20;
const CONTENT_W: usize = SCREEN_W - 2 * MARGIN;
const BUTTON_H: usize = 100;

const PILL: Rect = (SCREEN_W - 196, 12, 180, 76);
const MENU_W: usize = 500;
const MENU_ROW_H: usize = 112;
const MENU_ITEMS: [(&str, MenuItem); 3] = [
    ("Settings", MenuItem::Settings),
    ("Refresh screen", MenuItem::Refresh),
    ("Exit PaperTerm", MenuItem::Exit),
];
const MENU: Rect = (
    SCREEN_W - 16 - MENU_W,
    PILL.1 + PILL.3 + 8,
    MENU_W,
    MENU_ROW_H * MENU_ITEMS.len(),
);

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

fn draw_button(fb: &mut [u8], font: &Font, rect: Rect, label: &str, selected: bool) {
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

/// `count` equal buttons side by side across the content width.
fn row(y: usize, count: usize, i: usize) -> Rect {
    let w = (CONTENT_W - GAP * (count - 1)) / count;
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
const PREVIEW: Rect = (
    MARGIN,
    PREVIEW_LABEL_Y + 52,
    CONTENT_W,
    SCREEN_H - 36 - (PREVIEW_LABEL_Y + 52),
);
const FONT_COLUMNS: usize = 3;

fn font_rect(i: usize) -> Rect {
    let (x, _, w, h) = row(0, FONT_COLUMNS, i % FONT_COLUMNS);
    (x, FONT_Y + (i / FONT_COLUMNS) * (h + GAP), w, h)
}

fn panel_buttons() -> Vec<(Rect, Hit)> {
    let mut out = vec![
        ((SCREEN_W - MARGIN - 440 - GAP, HEADER_Y, 220, 92), Hit::Cancel),
        ((SCREEN_W - MARGIN - 220, HEADER_Y, 220, 92), Hit::Done),
        ((MARGIN, SIZE_Y, 140, BUTTON_H), Hit::Smaller),
        ((MARGIN + 340, SIZE_Y, 140, BUTTON_H), Hit::Bigger),
        (row(TEXT_Y, 2, 0), Hit::Text(false)),
        (row(TEXT_Y, 2, 1), Hit::Text(true)),
    ];
    for i in 0..FONTS.len() {
        out.push((font_rect(i), Hit::Font(i)));
    }
    for i in 0..REFRESH_OPTIONS.len() {
        out.push((row(REFRESH_Y, REFRESH_OPTIONS.len(), i), Hit::Refresh(i)));
    }
    let cleanup_columns = CLEANUP_OPTIONS.len() + 1;
    for i in 0..CLEANUP_OPTIONS.len() {
        out.push((row(CLEANUP_Y, cleanup_columns, i), Hit::Cleanup(i)));
    }
    out.push((row(CLEANUP_Y, cleanup_columns, cleanup_columns - 1), Hit::CleanNow));
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

    pub fn on_press(&mut self, x: i32, y: i32, settings: &Settings) -> UiCommand {
        match &mut self.screen {
            Screen::Terminal => {
                if contains(PILL, x, y) {
                    self.screen = Screen::Menu;
                    UiCommand::MenuOpened
                } else {
                    UiCommand::Pass
                }
            }
            Screen::Menu => {
                self.screen = Screen::Terminal;
                let item = MENU_ITEMS.iter().enumerate().find_map(|(i, (_, item))| {
                    let rect = (MENU.0, MENU.1 + i * MENU_ROW_H, MENU_W, MENU_ROW_H);
                    contains(rect, x, y).then_some(*item)
                });
                match item {
                    Some(MenuItem::Settings) => {
                        self.screen = Screen::Settings(Box::new(Panel::new(settings)));
                        UiCommand::PanelChanged
                    }
                    Some(MenuItem::Refresh) => UiCommand::RefreshScreen,
                    Some(MenuItem::Exit) => UiCommand::Exit,
                    None => UiCommand::Closed,
                }
            }
            Screen::Settings(panel) => {
                let Some((_, hit)) = panel_buttons().into_iter().find(|(r, _)| contains(*r, x, y)) else {
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
    pub fn overlay_rect(&self) -> Rect {
        match self.screen {
            Screen::Menu => (MENU.0, PILL.1, MENU.2, MENU.1 + MENU.3 - PILL.1),
            _ => PILL,
        }
    }

    pub fn overlay_touches(&self, dirty: Rect) -> bool {
        intersects(self.overlay_rect(), dirty)
    }

    pub fn draw_overlay(&self, fb: &mut [u8]) {
        draw_button(fb, &self.label, PILL, "Menu", matches!(self.screen, Screen::Menu));
        if matches!(self.screen, Screen::Menu) {
            for (i, (label, _)) in MENU_ITEMS.iter().enumerate() {
                let rect = (MENU.0, MENU.1 + i * MENU_ROW_H, MENU_W, MENU_ROW_H);
                draw_button(fb, &self.label, rect, label, false);
            }
        }
    }

    /// Draw the settings screen. `term_h` is the height the terminal would have.
    pub fn draw_panel(&self, fb: &mut [u8], term_h: usize) {
        let Screen::Settings(panel) = &self.screen else { return };
        let s = &panel.draft;
        render::fill_rect(fb, 0, 0, SCREEN_W, SCREEN_H, WHITE);

        let heading = |fb: &mut [u8], y: usize, text: &str| {
            render::draw_text(fb, &self.label, MARGIN, y, text, BLACK, WHITE);
        };
        render::draw_text(fb, &self.title, MARGIN, HEADER_Y + 12, "Settings", BLACK, WHITE);
        heading(fb, FONT_LABEL_Y, "Font");
        heading(fb, SIZE_LABEL_Y, "Size");
        heading(fb, TEXT_LABEL_Y, "Text style");
        heading(fb, REFRESH_LABEL_Y, "Refresh mode");
        heading(fb, CLEANUP_LABEL_Y, "Automatic ghost cleanup");
        heading(fb, PREVIEW_LABEL_Y, "Preview");

        for (rect, hit) in panel_buttons() {
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
            SCREEN_W / panel.preview.width,
            term_h / panel.preview.height
        );
        render::draw_text(fb, &self.label, MARGIN + 520, SIZE_Y + 30, &grid, BLACK, WHITE);
        render::draw_text(fb, &self.label, MARGIN, HINT_Y, REFRESH_OPTIONS[s.refresh].hint, BLACK, WHITE);

        draw_preview(fb, &panel.preview);
    }
}

fn draw_preview(fb: &mut [u8], font: &Font) {
    let (x, y, w, h) = PREVIEW;
    draw_button(fb, font, PREVIEW, "", false);
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

    #[test]
    fn every_button_is_on_screen_and_none_overlap() {
        let buttons = panel_buttons();
        for (i, (a, hit)) in buttons.iter().enumerate() {
            assert!(a.0 + a.2 <= SCREEN_W && a.1 + a.3 <= SCREEN_H, "{hit:?} off screen");
            assert!(!intersects(*a, PREVIEW), "{hit:?} over preview");
            for (b, other) in &buttons[i + 1..] {
                assert!(!intersects(*a, *b), "{hit:?} overlaps {other:?}");
            }
        }
    }

    #[test]
    fn menu_flow_opens_settings_and_cancel_returns() {
        let settings = Settings::default();
        let mut ui = Ui::new();
        assert!(matches!(ui.on_press(10, 1000, &settings), UiCommand::Pass));
        assert!(matches!(ui.on_press(PILL.0 as i32 + 1, PILL.1 as i32 + 1, &settings), UiCommand::MenuOpened));
        assert!(matches!(
            ui.on_press(MENU.0 as i32 + 1, MENU.1 as i32 + 1, &settings),
            UiCommand::PanelChanged
        ));
        assert!(ui.settings_open());
        let cancel = panel_buttons().into_iter().find(|(_, h)| *h == Hit::Cancel).unwrap().0;
        assert!(matches!(
            ui.on_press(cancel.0 as i32 + 1, cancel.1 as i32 + 1, &settings),
            UiCommand::RefreshScreen
        ));
        assert!(!ui.settings_open());
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

    /// Writes raw RGB565 screens for checking the layout by eye:
    /// PAPERTERM_DUMP_DIR=/tmp/out cargo test dump_screens
    #[test]
    fn dump_screens() {
        let Ok(dir) = std::env::var("PAPERTERM_DUMP_DIR") else { return };
        let settings = Settings::default();
        let mut fb = vec![0xFFu8; SCREEN_W * SCREEN_H * 2];
        let mut ui = Ui::new();

        let term = Font::load(settings.font, settings.size, false).unwrap();
        for (i, line) in PREVIEW_LINES.iter().enumerate() {
            render::draw_text(&mut fb, &term, 0, i * term.height, line, BLACK, WHITE);
        }
        ui.draw_overlay(&mut fb);
        std::fs::write(format!("{dir}/terminal.rgb565"), &fb).unwrap();

        ui.on_press(PILL.0 as i32 + 1, PILL.1 as i32 + 1, &settings);
        ui.draw_overlay(&mut fb);
        std::fs::write(format!("{dir}/menu.rgb565"), &fb).unwrap();

        ui.on_press(MENU.0 as i32 + 1, MENU.1 as i32 + 1, &settings);
        ui.draw_panel(&mut fb, SCREEN_H);
        std::fs::write(format!("{dir}/settings.rgb565"), &fb).unwrap();

        let spleen = font_rect(4);
        ui.on_press(spleen.0 as i32 + 1, spleen.1 as i32 + 1, &settings);
        let crisp = row(TEXT_Y, 2, 1);
        ui.on_press(crisp.0 as i32 + 1, crisp.1 as i32 + 1, &settings);
        ui.draw_panel(&mut fb, SCREEN_H);
        std::fs::write(format!("{dir}/settings-spleen.rgb565"), &fb).unwrap();
    }
}

//! Choosing text on screen with a finger or the pen, and the clipboard it goes to.
//! A finger still scrolls when it moves straight away; holding it still selects
//! the word under it. The pen selects exactly what it is dragged across.

use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::index::{Column, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionRange, SelectionType};
use alacritty_terminal::term::{viewport_to_point, Term, TermMode};

use crate::render::TermArea;

const HOLD: Duration = Duration::from_millis(500);
/// Fingers and pens wobble: less movement than this is still holding or tapping.
const MOVE_SLOP: i32 = 24;

/// What the current press on the terminal area is doing.
#[derive(Default)]
pub enum Gesture {
    #[default]
    Idle,
    Pressed { id: i32, pen: bool, x: i32, y: i32, at: Instant },
    Scrolling { id: i32, last_y: i32 },
    Selecting { id: i32, pen: bool },
}

/// AppLoad doesn't promise the pen a steady device id, and there is only one pen.
fn same_device(id: i32, pen: bool, other_id: i32, other_pen: bool) -> bool {
    pen == other_pen && (pen || id == other_id)
}

impl Gesture {
    pub fn press(&mut self, id: i32, pen: bool, x: i32, y: i32, now: Instant) {
        *self = Gesture::Pressed { id, pen, x, y, at: now };
    }

    /// When the main loop must wake up to notice a press being held.
    pub fn deadline(&self) -> Option<Instant> {
        match self {
            Gesture::Pressed { at, .. } => Some(*at + HOLD),
            _ => None,
        }
    }

    pub fn selecting(&self) -> bool {
        matches!(self, Gesture::Selecting { .. })
    }

    /// Start selecting the word under a press that has been held long enough.
    pub fn tick<T: EventListener>(&mut self, now: Instant, term: &mut Term<T>, area: &TermArea) {
        if let Gesture::Pressed { id, pen, x, y, at } = *self {
            if now >= at + HOLD {
                let (point, side) = point_at(term, area, x, y);
                term.selection = Some(Selection::new(SelectionType::Semantic, point, side));
                *self = Gesture::Selecting { id, pen };
            }
        }
    }

    /// Returns whether the view scrolled.
    pub fn moved<T: EventListener>(
        &mut self,
        event: (i32, bool),
        x: i32,
        y: i32,
        term: &mut Term<T>,
        area: &TermArea,
    ) -> bool {
        let (event_id, event_pen) = event;
        match *self {
            Gesture::Pressed { id, pen, x: x0, y: y0, .. }
                if same_device(id, pen, event_id, event_pen) =>
            {
                if (x - x0).abs() < MOVE_SLOP && (y - y0).abs() < MOVE_SLOP {
                    return false;
                }
                if pen {
                    let (point, side) = point_at(term, area, x0, y0);
                    let mut selection = Selection::new(SelectionType::Simple, point, side);
                    let (point, side) = point_at(term, area, x, y);
                    selection.update(point, side);
                    term.selection = Some(selection);
                    *self = Gesture::Selecting { id, pen };
                    false
                } else {
                    *self = Gesture::Scrolling { id, last_y: y0 };
                    self.moved(event, x, y, term, area)
                }
            }
            Gesture::Scrolling { id, last_y } if !event_pen && id == event_id => {
                let lines = (y - last_y) / area.ch as i32;
                if lines == 0 {
                    return false;
                }
                // Drag down => view earlier history (positive Delta).
                term.scroll_display(Scroll::Delta(lines));
                *self = Gesture::Scrolling { id, last_y: last_y + lines * area.ch as i32 };
                true
            }
            Gesture::Selecting { id, pen } if same_device(id, pen, event_id, event_pen) => {
                let (point, side) = point_at(term, area, x, y);
                if let Some(selection) = term.selection.as_mut() {
                    selection.update(point, side);
                }
                false
            }
            _ => false,
        }
    }

    /// A tap on the terminal clears any selection.
    pub fn release<T: EventListener>(&mut self, id: i32, pen: bool, term: &mut Term<T>) {
        let ended = match *self {
            Gesture::Idle => false,
            Gesture::Pressed { id: i, pen: p, .. } => {
                let ended = same_device(i, p, id, pen);
                if ended {
                    term.selection = None;
                }
                ended
            }
            Gesture::Scrolling { id: i, .. } => !pen && i == id,
            Gesture::Selecting { id: i, pen: p } => same_device(i, p, id, pen),
        };
        if ended {
            *self = Gesture::Idle;
        }
    }
}

/// The text position under a screen position, kept inside the grid.
fn point_at<T: EventListener>(term: &Term<T>, area: &TermArea, x: i32, y: i32) -> (Point, Side) {
    let x = (x.max(0) as usize).saturating_sub(area.x0);
    let y = (y.max(0) as usize).saturating_sub(area.y0);
    let col = (x / area.cw).min(area.cols - 1);
    let row = (y / area.ch).min(area.rows - 1);
    let side = if x % area.cw < area.cw / 2 { Side::Left } else { Side::Right };
    let offset = term.grid().display_offset();
    (viewport_to_point(offset, Point::new(row, Column(col))), side)
}

/// The on-screen rows a selection covers.
pub fn visible_rows(range: SelectionRange, display_offset: usize, rows: usize) -> impl Iterator<Item = usize> {
    let row = |line: i32| line + display_offset as i32;
    let first = row(range.start.line.0).max(0);
    let last = row(range.end.line.0).min(rows as i32 - 1);
    (first..=last).map(|r| r as usize)
}

/// What to send the shell for pasted text. A program that asked for bracketed
/// paste gets it marked as such, so pasted lines don't run as they arrive.
pub fn paste_bytes(text: &str, mode: TermMode) -> Vec<u8> {
    if mode.contains(TermMode::BRACKETED_PASTE) {
        // Without these, pasted text could end the bracket early and run anyway.
        let safe = text.replace(['\x1b', '\u{9b}'], "");
        format!("\x1b[200~{safe}\x1b[201~").into_bytes()
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

/// Kept in a file on the tablet's RAM disk as well, so it outlasts closing
/// PaperTerm the way the shell does. A reboot clears it.
pub struct Clipboard {
    path: PathBuf,
    text: Option<String>,
}

impl Clipboard {
    pub fn load() -> Self {
        Self::at(PathBuf::from("/tmp/paperterm-clipboard"))
    }

    fn at(path: PathBuf) -> Self {
        let text = std::fs::read_to_string(&path).ok().filter(|t| !t.is_empty());
        Self { path, text }
    }

    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }

    pub fn set(&mut self, text: String) -> io::Result<()> {
        let saved = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&self.path)
            .and_then(|mut file| file.write_all(text.as_bytes()));
        self.text = Some(text);
        saved
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GridSize;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};

    const AREA: TermArea = TermArea { x0: 4, y0: 0, cols: 40, rows: 5, cw: 10, ch: 20 };
    const FINGER: (i32, bool) = (7, false);
    const PEN: (i32, bool) = (0, true);

    fn term_with(text: &str) -> Term<VoidListener> {
        let size = GridSize { cols: AREA.cols, lines: AREA.rows };
        let config = Config { scrolling_history: 100, ..Config::default() };
        let mut term = Term::new(config, &size, VoidListener);
        let mut parser: Processor<StdSyncHandler> = Processor::new();
        parser.advance(&mut term, text.as_bytes());
        term
    }

    /// The middle of a cell.
    fn at(col: i32, row: i32) -> (i32, i32) {
        (AREA.x0 as i32 + col * 10 + 2, row * 20 + 10)
    }

    fn hold_at(term: &mut Term<VoidListener>, col: i32, row: i32) -> Gesture {
        let (x, y) = at(col, row);
        let now = Instant::now();
        let mut gesture = Gesture::default();
        gesture.press(FINGER.0, false, x, y, now);
        gesture.tick(now + HOLD, term, &AREA);
        gesture
    }

    #[test]
    fn holding_a_finger_selects_the_word_under_it() {
        let mut term = term_with("cat /etc/hosts now");
        let mut gesture = hold_at(&mut term, 6, 0);
        assert!(gesture.selecting());
        assert_eq!(term.selection_to_string().as_deref(), Some("/etc/hosts"));
        gesture.release(FINGER.0, false, &mut term);
        assert!(!gesture.selecting());
        assert_eq!(term.selection_to_string().as_deref(), Some("/etc/hosts"));
    }

    #[test]
    fn dragging_after_holding_extends_by_words() {
        let mut term = term_with("one two three four");
        let mut gesture = hold_at(&mut term, 0, 0);
        let (x, y) = at(9, 0);
        gesture.moved(FINGER, x, y, &mut term, &AREA);
        assert_eq!(term.selection_to_string().as_deref(), Some("one two three"));
    }

    #[test]
    fn a_press_not_held_long_enough_selects_nothing() {
        let mut term = term_with("hello");
        let now = Instant::now();
        let mut gesture = Gesture::default();
        gesture.press(FINGER.0, false, 10, 10, now);
        gesture.tick(now + HOLD / 2, &mut term, &AREA);
        assert!(term.selection.is_none());
        assert_eq!(gesture.deadline(), Some(now + HOLD));
    }

    #[test]
    fn a_finger_that_moves_scrolls_instead_and_cannot_then_select() {
        let lines: String = (0..50).map(|i| format!("line {i}\r\n")).collect();
        let mut term = term_with(&lines);
        let now = Instant::now();
        let mut gesture = Gesture::default();
        gesture.press(FINGER.0, false, 50, 10, now);
        assert!(gesture.moved(FINGER, 50, 10 + 3 * 20, &mut term, &AREA));
        assert_eq!(term.grid().display_offset(), 3);
        gesture.tick(now + HOLD, &mut term, &AREA);
        assert!(term.selection.is_none());
    }

    #[test]
    fn a_small_wobble_is_still_a_hold() {
        let mut term = term_with("word");
        let now = Instant::now();
        let mut gesture = Gesture::default();
        gesture.press(FINGER.0, false, 20, 10, now);
        assert!(!gesture.moved(FINGER, 25, 13, &mut term, &AREA));
        gesture.tick(now + HOLD, &mut term, &AREA);
        assert_eq!(term.selection_to_string().as_deref(), Some("word"));
    }

    #[test]
    fn the_pen_selects_exactly_what_it_crosses() {
        let mut term = term_with("cat /etc/hosts now");
        let (x0, y) = (AREA.x0 as i32 + 4 * 10 + 1, 10);
        let mut gesture = Gesture::default();
        gesture.press(3, true, x0, y, Instant::now());
        // The second device id stands in for AppLoad reporting the pen differently.
        gesture.moved(PEN, AREA.x0 as i32 + 7 * 10 + 9, y, &mut term, &AREA);
        assert_eq!(term.selection_to_string().as_deref(), Some("/etc"));
        gesture.release(PEN.0, true, &mut term);
        assert!(!gesture.selecting());
    }

    #[test]
    fn a_selection_follows_lines_into_the_next_row() {
        let mut term = term_with("first line\r\nsecond line");
        let mut gesture = Gesture::default();
        gesture.press(0, true, AREA.x0 as i32 + 1, 10, Instant::now());
        gesture.moved(PEN, AREA.x0 as i32 + 5 * 10 + 9, 30, &mut term, &AREA);
        assert_eq!(term.selection_to_string().as_deref(), Some("first line\nsecond"));
    }

    #[test]
    fn a_tap_clears_the_selection_and_a_scroll_keeps_it() {
        let lines: String = (0..50).map(|i| format!("line {i}\r\n")).collect();
        let mut term = term_with(&lines);
        let mut gesture = hold_at(&mut term, 0, 1);
        gesture.release(FINGER.0, false, &mut term);
        assert!(term.selection.is_some());

        gesture.press(FINGER.0, false, 50, 10, Instant::now());
        gesture.moved(FINGER, 50, 70, &mut term, &AREA);
        gesture.release(FINGER.0, false, &mut term);
        assert!(term.selection.is_some());

        gesture.press(FINGER.0, false, 50, 10, Instant::now());
        gesture.release(FINGER.0, false, &mut term);
        assert!(term.selection.is_none());
    }

    #[test]
    fn dragging_off_the_grid_stays_inside_it() {
        let mut term = term_with("edge");
        let mut gesture = hold_at(&mut term, 0, 0);
        gesture.moved(FINGER, -500, 5000, &mut term, &AREA);
        gesture.moved(FINGER, 5000, -500, &mut term, &AREA);
        assert!(term.selection_to_string().is_some());
    }

    #[test]
    fn selected_cells_are_drawn_inverted() {
        use crate::font::Font;
        use crate::palette::Palette;
        use crate::render;
        use crate::screen::{Canvas, Rotation};

        let font = Font::load(0, 40, false).unwrap();
        let area = TermArea { x0: 0, y0: 0, cols: AREA.cols, rows: AREA.rows, cw: font.width, ch: font.height };
        let mut term = term_with("  word  ");
        let mut gesture = Gesture::default();
        gesture.press(FINGER.0, false, (2 * font.width + 1) as i32, 1, Instant::now());
        gesture.tick(Instant::now() + HOLD, &mut term, &area);
        let selected = term.selection.as_ref().and_then(|s| s.to_range(&term));
        assert!(selected.is_some());

        let mut fb = Canvas::new(Rotation::Deg0);
        render::draw_line(&mut fb, &font, &Palette::new(false), &term, &area, 0, 0, area.cols - 1, selected);
        // The top-left pixel of a cell is background, clear of the letter.
        let corner = |col: usize| fb.px[col * font.width * 2];
        assert_eq!(corner(1), 0xFF);
        assert_eq!(corner(2), 0x00);
        assert_eq!(corner(5), 0x00);
        assert_eq!(corner(6), 0xFF);
    }

    #[test]
    fn visible_rows_follow_scrolling_and_stop_at_the_screen() {
        use alacritty_terminal::index::Line;
        let range = SelectionRange::new(
            Point::new(Line(-2), Column(0)),
            Point::new(Line(9), Column(3)),
            false,
        );
        assert_eq!(visible_rows(range, 0, 5).collect::<Vec<_>>(), [0, 1, 2, 3, 4]);
        assert_eq!(visible_rows(range, 3, 5).collect::<Vec<_>>(), [1, 2, 3, 4]);
    }

    #[test]
    fn paste_turns_newlines_into_returns() {
        assert_eq!(paste_bytes("a\nb\r\nc", TermMode::empty()), b"a\rb\rc");
    }

    #[test]
    fn bracketed_paste_can_not_be_ended_from_inside() {
        let out = paste_bytes("a\nb\x1b[201~rm", TermMode::BRACKETED_PASTE);
        assert_eq!(out, b"\x1b[200~a\nb[201~rm\x1b[201~");
    }

    #[test]
    fn the_clipboard_survives_a_restart_and_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::temp_dir().join(format!("paperterm-clip-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut clipboard = Clipboard::at(path.clone());
        assert_eq!(clipboard.text(), None);
        clipboard.set("copied".to_string()).unwrap();
        assert_eq!(Clipboard::at(path.clone()).text(), Some("copied"));
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        std::fs::remove_file(&path).unwrap();
    }
}

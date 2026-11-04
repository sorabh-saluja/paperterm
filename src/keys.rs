//! Key actions -> PTY byte sequences, and decoding of AppLoad's built-in
//! virtual keyboard events (INPUT_VKB_*).

use alacritty_terminal::term::TermMode;
use std::collections::HashSet;

pub const MOD_SHIFT: i32 = 0x100000;
pub const MOD_CTRL: i32 = 0x200000;
pub const MOD_ALT: i32 = 0x400000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    Char(char),
    Enter,
    Backspace,
    Tab,
    BackTab,
    Esc,
    /// F1 to F12.
    F(u8),
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PgUp,
    PgDn,
    Delete,
}

/// Encode a key action (with modifier state) into the bytes to write to the
/// PTY. `mode` is consulted for DECCKM application cursor keys.
pub fn encode(action: KeyAction, ctrl: bool, alt: bool, mode: TermMode) -> Vec<u8> {
    let mut out = Vec::with_capacity(4);
    if alt {
        out.push(0x1b);
    }
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    match action {
        KeyAction::Char(c) => {
            if ctrl {
                let u = c.to_ascii_uppercase();
                match u {
                    'A'..='Z' => out.push((u as u8) & 0x1f),
                    ' ' | '@' => out.push(0),
                    '[' => out.push(0x1b),
                    '\\' => out.push(0x1c),
                    ']' => out.push(0x1d),
                    '^' => out.push(0x1e),
                    '_' | '/' => out.push(0x1f),
                    _ => {
                        let mut buf = [0u8; 4];
                        out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                    }
                }
            } else {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
        KeyAction::Enter => out.push(b'\r'),
        KeyAction::Backspace => out.push(0x7f),
        KeyAction::Tab => out.push(b'\t'),
        KeyAction::BackTab => out.extend_from_slice(b"\x1b[Z"),
        KeyAction::Esc => out.push(0x1b),
        KeyAction::F(n) => out.extend_from_slice(function_key(n)),
        KeyAction::Up | KeyAction::Down | KeyAction::Right | KeyAction::Left => {
            let ch = match action {
                KeyAction::Up => b'A',
                KeyAction::Down => b'B',
                KeyAction::Right => b'C',
                _ => b'D',
            };
            if ctrl {
                out.extend_from_slice(b"\x1b[1;5");
                out.push(ch);
            } else if app_cursor {
                out.extend_from_slice(b"\x1bO");
                out.push(ch);
            } else {
                out.extend_from_slice(b"\x1b[");
                out.push(ch);
            }
        }
        KeyAction::Home => out.extend_from_slice(if app_cursor { b"\x1bOH" } else { b"\x1b[H" }),
        KeyAction::End => out.extend_from_slice(if app_cursor { b"\x1bOF" } else { b"\x1b[F" }),
        KeyAction::PgUp => out.extend_from_slice(b"\x1b[5~"),
        KeyAction::PgDn => out.extend_from_slice(b"\x1b[6~"),
        KeyAction::Delete => out.extend_from_slice(b"\x1b[3~"),
    }
    out
}

/// The xterm sequence for F1 to F12.
fn function_key(n: u8) -> &'static [u8] {
    match n {
        1 => b"\x1bOP",
        2 => b"\x1bOQ",
        3 => b"\x1bOR",
        4 => b"\x1bOS",
        5 => b"\x1b[15~",
        6 => b"\x1b[17~",
        7 => b"\x1b[18~",
        8 => b"\x1b[19~",
        9 => b"\x1b[20~",
        10 => b"\x1b[21~",
        11 => b"\x1b[23~",
        12 => b"\x1b[24~",
        _ => b"",
    }
}

/// Decode an AppLoad virtual-keyboard press (`x` field of the event).
/// Codes per resources/keyboard/default.layout.json (which disagrees with
/// common.h by +1 for the named keys — the JSON is what's on the wire):
/// letters arrive as UPPERCASE ASCII with shift as a modifier bit.
pub fn decode_vkb(x: i32) -> Option<(KeyAction, bool, bool)> {
    let mods = x & (MOD_SHIFT | MOD_CTRL | MOD_ALT);
    let code = x & !(MOD_SHIFT | MOD_CTRL | MOD_ALT);
    let shift = mods & MOD_SHIFT != 0;
    let ctrl = mods & MOD_CTRL != 0;
    let alt = mods & MOD_ALT != 0;

    let action = match code {
        0 => return None, // bare modifier press
        9 => KeyAction::Tab,
        13 => KeyAction::Enter,
        27 => KeyAction::Esc,
        8 | 128 => KeyAction::Backspace,
        127 => KeyAction::Delete,
        129 => KeyAction::PgUp,
        130 => KeyAction::PgDn,
        131 => KeyAction::Down,
        132 => KeyAction::Up,
        133 => KeyAction::Left,
        134 => KeyAction::Right,
        135 => KeyAction::Home,
        136 => KeyAction::End,
        c @ 65..=90 => {
            let ch = c as u8 as char;
            KeyAction::Char(if shift { ch } else { ch.to_ascii_lowercase() })
        }
        c @ 32..=126 => KeyAction::Char(c as u8 as char),
        _ => return None,
    };
    Some((action, ctrl, alt))
}

// Qt::Key values for the non-printing keys AppLoad forwards from a physical keyboard.
const QT_KEY_ESCAPE: i32 = 0x0100_0000;
const QT_KEY_TAB: i32 = 0x0100_0001;
const QT_KEY_BACKTAB: i32 = 0x0100_0002;
const QT_KEY_BACKSPACE: i32 = 0x0100_0003;
const QT_KEY_RETURN: i32 = 0x0100_0004;
const QT_KEY_ENTER: i32 = 0x0100_0005;
const QT_KEY_DELETE: i32 = 0x0100_0007;
const QT_KEY_HOME: i32 = 0x0100_0010;
const QT_KEY_END: i32 = 0x0100_0011;
const QT_KEY_LEFT: i32 = 0x0100_0012;
const QT_KEY_UP: i32 = 0x0100_0013;
const QT_KEY_RIGHT: i32 = 0x0100_0014;
const QT_KEY_DOWN: i32 = 0x0100_0015;
const QT_KEY_PAGE_UP: i32 = 0x0100_0016;
const QT_KEY_PAGE_DOWN: i32 = 0x0100_0017;
const QT_KEY_SHIFT: i32 = 0x0100_0020;
const QT_KEY_CONTROL: i32 = 0x0100_0021;
const QT_KEY_META: i32 = 0x0100_0022;
const QT_KEY_ALT: i32 = 0x0100_0023;
const QT_KEY_CAPS_LOCK: i32 = 0x0100_0024;
const QT_KEY_F1: i32 = 0x0100_0030;
const QT_KEY_F12: i32 = 0x0100_003B;

/// Qt's dead (accent) keys, and the symbol each one types here. The Type Folio
/// has no bracket keys, but its two accent keys beside P sit where a US keyboard
/// has them, and a terminal has little use for the accents. With Alt (the "opt"
/// key) they type braces. Shift still gives the ` and ~ printed on them.
fn dead_key_symbol(key: i32, alt: bool) -> Option<char> {
    match key {
        0x0100_1250 => Some('`'),
        0x0100_1251 => Some(if alt { '{' } else { '[' }),
        0x0100_1252 => Some('^'),
        0x0100_1253 => Some('~'),
        0x0100_1257 => Some(if alt { '}' } else { ']' }),
        _ => None,
    }
}

/// The Type Folio has no function row, so Alt plus the number row stands in:
/// Alt+1 to Alt+9 are F1 to F9, then Alt+0 is F10, Alt+- F11 and Alt+= F12.
/// Qt's Alt is the key labelled "opt". The key labelled "alt" can't be a modifier:
/// Qt never reports it being pressed, only a Control release afterwards.
fn folio_function_key(key: i32) -> Option<u8> {
    match char::from_u32(key as u32)? {
        c @ '1'..='9' => Some(c as u8 - b'0'),
        '0' => Some(10),
        '-' => Some(11),
        '=' => Some(12),
        _ => None,
    }
}

/// Modifier state for physical keyboards. AppLoad sends only the bare Qt::Key
/// for each press and release, so modifiers have to be tracked here.
#[derive(Debug, Default)]
pub struct PhysicalKeyboard {
    shift: bool,
    ctrl: bool,
    alt: bool,
    caps: bool,
    /// Keys whose press we saw and whose release we haven't.
    down: HashSet<i32>,
}

impl PhysicalKeyboard {
    /// Asked directly because caps lock also gives an uppercase letter.
    pub fn shift_held(&self) -> bool {
        self.shift
    }

    pub fn press(&mut self, key: i32) -> Option<(KeyAction, bool, bool)> {
        self.down.insert(key);
        if let Some(symbol) = dead_key_symbol(key, self.alt) {
            return Some((KeyAction::Char(symbol), self.ctrl, false));
        }
        if self.alt {
            if let Some(n) = folio_function_key(key) {
                return Some((KeyAction::F(n), self.ctrl, false));
            }
        }
        let action = match key {
            QT_KEY_SHIFT => {
                self.shift = true;
                return None;
            }
            // Qt reports the Type Folio's Ctrl key as Meta.
            QT_KEY_CONTROL | QT_KEY_META => {
                self.ctrl = true;
                return None;
            }
            QT_KEY_ALT => {
                self.alt = true;
                return None;
            }
            QT_KEY_CAPS_LOCK => {
                self.caps = !self.caps;
                return None;
            }
            QT_KEY_ESCAPE => KeyAction::Esc,
            QT_KEY_BACKTAB => KeyAction::BackTab,
            QT_KEY_TAB if self.shift => KeyAction::BackTab,
            QT_KEY_TAB => KeyAction::Tab,
            QT_KEY_F1..=QT_KEY_F12 => KeyAction::F((key - QT_KEY_F1) as u8 + 1),
            QT_KEY_BACKSPACE => KeyAction::Backspace,
            QT_KEY_RETURN | QT_KEY_ENTER => KeyAction::Enter,
            QT_KEY_DELETE => KeyAction::Delete,
            QT_KEY_HOME => KeyAction::Home,
            QT_KEY_END => KeyAction::End,
            QT_KEY_LEFT => KeyAction::Left,
            QT_KEY_UP => KeyAction::Up,
            QT_KEY_RIGHT => KeyAction::Right,
            QT_KEY_DOWN => KeyAction::Down,
            QT_KEY_PAGE_UP => KeyAction::PgUp,
            QT_KEY_PAGE_DOWN => KeyAction::PgDn,
            // Qt reports letters in uppercase whatever the shift state, and
            // symbols already shifted ('!' rather than '1').
            0x20..=0x10_FFFF => {
                let c = char::from_u32(key as u32)?;
                if self.shift != self.caps {
                    KeyAction::Char(c.to_uppercase().next().unwrap_or(c))
                } else {
                    KeyAction::Char(c.to_lowercase().next().unwrap_or(c))
                }
            }
            _ => return None,
        };
        Some((action, self.ctrl, self.alt))
    }

    /// Qt's text composition swallows the press of an accent key and of the key
    /// after it, so a release with no press means that key was typed.
    pub fn release(&mut self, key: i32) -> Option<(KeyAction, bool, bool)> {
        match key {
            QT_KEY_SHIFT => self.shift = false,
            QT_KEY_CONTROL | QT_KEY_META => self.ctrl = false,
            QT_KEY_ALT => self.alt = false,
            _ => {}
        }
        let was_down = self.down.remove(&key);
        let printable = (0x20..=0x10_FFFF).contains(&key);
        if was_down || !(printable || dead_key_symbol(key, self.alt).is_some()) {
            return None;
        }
        let typed = self.press(key);
        self.down.remove(&key);
        typed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(c: char, ctrl: bool, alt: bool) -> Option<(KeyAction, bool, bool)> {
        Some((KeyAction::Char(c), ctrl, alt))
    }

    #[test]
    fn letters_follow_shift_and_caps_lock() {
        let mut k = PhysicalKeyboard::default();
        assert_eq!(k.press(0x41), ch('a', false, false));
        k.press(QT_KEY_SHIFT);
        assert_eq!(k.press(0x41), ch('A', false, false));
        k.release(QT_KEY_SHIFT);
        assert_eq!(k.press(0x41), ch('a', false, false));
        k.press(QT_KEY_CAPS_LOCK);
        k.release(QT_KEY_CAPS_LOCK);
        assert_eq!(k.press(0x41), ch('A', false, false));
        k.press(QT_KEY_SHIFT);
        assert_eq!(k.press(0x41), ch('a', false, false));
    }

    #[test]
    fn symbols_and_digits_pass_through() {
        let mut k = PhysicalKeyboard::default();
        assert_eq!(k.press(0x31), ch('1', false, false));
        k.press(QT_KEY_SHIFT);
        assert_eq!(k.press(0x21), ch('!', false, false));
        assert_eq!(k.press(0x20), ch(' ', false, false));
    }

    #[test]
    fn ctrl_c_and_alt_reach_the_pty() {
        let mut k = PhysicalKeyboard::default();
        k.press(QT_KEY_CONTROL);
        let (a, c, al) = k.press(0x43).unwrap();
        assert_eq!(encode(a, c, al, TermMode::empty()), vec![0x03]);
        k.release(QT_KEY_CONTROL);
        k.press(QT_KEY_ALT);
        let (a, c, al) = k.press(0x58).unwrap();
        assert_eq!(encode(a, c, al, TermMode::empty()), vec![0x1b, b'x']);
    }

    #[test]
    fn folio_ctrl_arrives_as_meta() {
        let mut k = PhysicalKeyboard::default();
        k.press(QT_KEY_META);
        let (a, c, al) = k.press(0x43).unwrap();
        assert_eq!(encode(a, c, al, TermMode::empty()), vec![0x03]);
        k.release(QT_KEY_META);
        assert_eq!(k.press(0x43), ch('c', false, false));
    }

    #[test]
    fn named_keys_encode_for_the_terminal() {
        let mut k = PhysicalKeyboard::default();
        for (key, bytes) in [
            (QT_KEY_RETURN, &b"\r"[..]),
            (QT_KEY_ENTER, b"\r"),
            (QT_KEY_BACKSPACE, b"\x7f"),
            (QT_KEY_TAB, b"\t"),
            (QT_KEY_ESCAPE, b"\x1b"),
            (QT_KEY_UP, b"\x1b[A"),
            (QT_KEY_DELETE, b"\x1b[3~"),
        ] {
            let (a, c, al) = k.press(key).unwrap();
            assert_eq!(encode(a, c, al, TermMode::empty()), bytes);
        }
    }

    fn bytes(k: &mut PhysicalKeyboard, key: i32) -> Vec<u8> {
        let (a, c, al) = k.press(key).unwrap();
        encode(a, c, al, TermMode::empty())
    }

    #[test]
    fn shift_tab_goes_backwards_however_qt_reports_it() {
        let mut k = PhysicalKeyboard::default();
        assert_eq!(bytes(&mut k, QT_KEY_BACKTAB), b"\x1b[Z");
        k.press(QT_KEY_SHIFT);
        assert_eq!(bytes(&mut k, QT_KEY_TAB), b"\x1b[Z");
        k.release(QT_KEY_SHIFT);
        assert_eq!(bytes(&mut k, QT_KEY_TAB), b"\t");
    }

    #[test]
    fn function_keys_from_a_keyboard_that_has_them() {
        let mut k = PhysicalKeyboard::default();
        assert_eq!(bytes(&mut k, QT_KEY_F1), b"\x1bOP");
        assert_eq!(bytes(&mut k, QT_KEY_F1 + 4), b"\x1b[15~");
        assert_eq!(bytes(&mut k, QT_KEY_F12), b"\x1b[24~");
    }

    #[test]
    fn alt_and_the_number_row_are_function_keys() {
        let mut k = PhysicalKeyboard::default();
        k.press(QT_KEY_ALT);
        assert_eq!(bytes(&mut k, '1' as i32), b"\x1bOP");
        assert_eq!(bytes(&mut k, '9' as i32), b"\x1b[20~");
        assert_eq!(bytes(&mut k, '0' as i32), b"\x1b[21~");
        assert_eq!(bytes(&mut k, '-' as i32), b"\x1b[23~");
        assert_eq!(bytes(&mut k, '=' as i32), b"\x1b[24~");
        // Letters still send Alt as usual.
        assert_eq!(bytes(&mut k, 0x58), vec![0x1b, b'x']);
        k.release(QT_KEY_ALT);
        assert_eq!(bytes(&mut k, '1' as i32), b"1");
    }

    const DEAD_TILDE: i32 = 0x0100_1253;

    #[test]
    fn accent_keys_type_on_release_when_qt_swallows_the_press() {
        // As captured from the Type Folio: Shift down, then only a release.
        let mut k = PhysicalKeyboard::default();
        k.press(QT_KEY_SHIFT);
        assert_eq!(k.release(DEAD_TILDE), ch('~', false, false));
        k.release(QT_KEY_SHIFT);
        assert_eq!(k.release(0x0100_1250), ch('`', false, false));
    }

    #[test]
    fn the_accent_keys_type_brackets_and_braces() {
        let mut k = PhysicalKeyboard::default();
        assert_eq!(k.release(0x0100_1251), ch('[', false, false));
        assert_eq!(k.release(0x0100_1257), ch(']', false, false));
        k.press(QT_KEY_ALT);
        assert_eq!(k.release(0x0100_1251), ch('{', false, false));
        assert_eq!(k.release(0x0100_1257), ch('}', false, false));
    }

    #[test]
    fn the_key_after_an_accent_key_still_types() {
        let mut k = PhysicalKeyboard::default();
        assert_eq!(k.release(DEAD_TILDE), ch('~', false, false));
        assert_eq!(k.release(0x43), ch('c', false, false));
    }

    #[test]
    fn a_normal_press_and_release_types_once() {
        let mut k = PhysicalKeyboard::default();
        assert_eq!(k.press(0x2d), ch('-', false, false));
        assert_eq!(k.release(0x2d), None);
        assert_eq!(k.press(DEAD_TILDE), ch('~', false, false));
        assert_eq!(k.release(DEAD_TILDE), None);
    }

    #[test]
    fn a_lone_modifier_or_special_release_types_nothing() {
        // The key labelled "alt" sends only this release.
        let mut k = PhysicalKeyboard::default();
        assert_eq!(k.release(QT_KEY_CONTROL), None);
        assert_eq!(k.release(QT_KEY_RETURN), None);
    }

    #[test]
    fn unknown_special_keys_are_ignored() {
        let mut k = PhysicalKeyboard::default();
        assert_eq!(k.press(0x0100_0053), None); // Super
        assert_eq!(k.press(QT_KEY_SHIFT), None);
    }
}

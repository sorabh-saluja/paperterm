//! Key actions -> PTY byte sequences, and decoding of AppLoad's built-in
//! virtual keyboard events (INPUT_VKB_*).

use alacritty_terminal::term::TermMode;

pub const MOD_SHIFT: i32 = 0x100000;
pub const MOD_CTRL: i32 = 0x200000;
pub const MOD_ALT: i32 = 0x400000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    Char(char),
    Enter,
    Backspace,
    Tab,
    Esc,
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
        KeyAction::Esc => out.push(0x1b),
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

/// Modifier state for physical keyboards. AppLoad sends only the bare Qt::Key
/// for each press and release, so modifiers have to be tracked here.
#[derive(Debug, Default)]
pub struct PhysicalKeyboard {
    shift: bool,
    ctrl: bool,
    alt: bool,
    caps: bool,
}

impl PhysicalKeyboard {
    pub fn press(&mut self, key: i32) -> Option<(KeyAction, bool, bool)> {
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
            QT_KEY_TAB | QT_KEY_BACKTAB => KeyAction::Tab,
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

    pub fn release(&mut self, key: i32) {
        match key {
            QT_KEY_SHIFT => self.shift = false,
            QT_KEY_CONTROL | QT_KEY_META => self.ctrl = false,
            QT_KEY_ALT => self.alt = false,
            _ => {}
        }
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

    #[test]
    fn unknown_special_keys_are_ignored() {
        let mut k = PhysicalKeyboard::default();
        assert_eq!(k.press(0x0100_0030), None); // F1
        assert_eq!(k.press(QT_KEY_SHIFT), None);
    }
}

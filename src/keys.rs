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

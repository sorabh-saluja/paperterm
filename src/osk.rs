//! On-screen keyboard drawn inside our framebuffer (bottom band), driven by
//! qtfb touch events. Always visible — unlike the system overlay keyboard it
//! never covers the shell prompt, and key feedback is a tiny partial update.

use crate::font::Font;
use crate::keys::KeyAction;
use crate::render::{self, SCREEN_W};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ModState {
    Off,
    Oneshot,
    Locked,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mod {
    Shift,
    Ctrl,
    Alt,
}

#[derive(Clone, Copy)]
pub enum KeyDef {
    /// normal char, shifted char
    Char(char, char),
    Named(KeyAction, &'static str),
    Modifier(Mod, &'static str),
    FullRefresh,
}

struct Key {
    def: KeyDef,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}

pub enum OskEvent {
    Send(KeyAction, bool, bool), // action, ctrl, alt
    FullRefresh,
    Redraw(Vec<(usize, usize, usize, usize)>), // dirty rects (may accompany Send)
}

pub struct Osk {
    keys: Vec<Key>,
    pub y0: usize,
    pub height: usize,
    shift: ModState,
    ctrl: ModState,
    alt: ModState,
    /// touch dev_id -> pressed key index
    pressed: HashMap<i32, usize>,
}

// Layout rows: (unit width, def). Units are relative per-row.
fn layout() -> Vec<Vec<(f32, KeyDef)>> {
    use KeyAction::*;
    use KeyDef::{Char, Modifier, Named};
    vec![
        vec![
            (1.3, Named(Esc, "esc")),
            (1.0, Char('`', '~')),
            (1.0, Char('1', '!')),
            (1.0, Char('2', '@')),
            (1.0, Char('3', '#')),
            (1.0, Char('4', '$')),
            (1.0, Char('5', '%')),
            (1.0, Char('6', '^')),
            (1.0, Char('7', '&')),
            (1.0, Char('8', '*')),
            (1.0, Char('9', '(')),
            (1.0, Char('0', ')')),
            (1.0, Char('-', '_')),
            (1.0, Char('=', '+')),
            (1.7, Named(Backspace, "bksp")),
        ],
        vec![
            (1.5, Named(Tab, "tab")),
            (1.0, Char('q', 'Q')),
            (1.0, Char('w', 'W')),
            (1.0, Char('e', 'E')),
            (1.0, Char('r', 'R')),
            (1.0, Char('t', 'T')),
            (1.0, Char('y', 'Y')),
            (1.0, Char('u', 'U')),
            (1.0, Char('i', 'I')),
            (1.0, Char('o', 'O')),
            (1.0, Char('p', 'P')),
            (1.0, Char('[', '{')),
            (1.0, Char(']', '}')),
            (1.5, Char('\\', '|')),
        ],
        vec![
            (1.8, Modifier(Mod::Ctrl, "ctrl")),
            (1.0, Char('a', 'A')),
            (1.0, Char('s', 'S')),
            (1.0, Char('d', 'D')),
            (1.0, Char('f', 'F')),
            (1.0, Char('g', 'G')),
            (1.0, Char('h', 'H')),
            (1.0, Char('j', 'J')),
            (1.0, Char('k', 'K')),
            (1.0, Char('l', 'L')),
            (1.0, Char(';', ':')),
            (1.0, Char('\'', '"')),
            (1.9, Named(Enter, "enter")),
        ],
        vec![
            (2.0, Modifier(Mod::Shift, "shift")),
            (1.0, Char('z', 'Z')),
            (1.0, Char('x', 'X')),
            (1.0, Char('c', 'C')),
            (1.0, Char('v', 'V')),
            (1.0, Char('b', 'B')),
            (1.0, Char('n', 'N')),
            (1.0, Char('m', 'M')),
            (1.0, Char(',', '<')),
            (1.0, Char('.', '>')),
            (1.0, Char('/', '?')),
            (1.2, Named(Up, "^")),
            (1.5, Named(Delete, "del")),
        ],
        vec![
            (1.6, Modifier(Mod::Alt, "alt")),
            (1.2, Named(Home, "home")),
            (1.2, Named(End, "end")),
            (5.0, Char(' ', ' ')),
            (1.2, Named(PgUp, "pgup")),
            (1.2, Named(PgDn, "pgdn")),
            (1.2, Named(Left, "<")),
            (1.2, Named(Down, "v")),
            (1.2, Named(Right, ">")),
            (1.2, KeyDef::FullRefresh),
        ],
    ]
}

impl Osk {
    pub fn new(y0: usize, height: usize) -> Self {
        let rows = layout();
        let row_h = height / rows.len();
        let mut keys = Vec::new();
        for (ri, row) in rows.iter().enumerate() {
            let total: f32 = row.iter().map(|(u, _)| u).sum();
            let unit = SCREEN_W as f32 / total;
            let mut x = 0f32;
            for (u, def) in row {
                let w = u * unit;
                keys.push(Key {
                    def: *def,
                    x: x.round() as usize,
                    y: y0 + ri * row_h,
                    w: (w.round() as usize).min(SCREEN_W - x.round() as usize),
                    h: row_h,
                });
                x += w;
            }
        }
        Self {
            keys,
            y0,
            height,
            shift: ModState::Off,
            ctrl: ModState::Off,
            alt: ModState::Off,
            pressed: HashMap::new(),
        }
    }

    fn hit(&self, x: i32, y: i32) -> Option<usize> {
        let (x, y) = (x as usize, y as usize);
        self.keys
            .iter()
            .position(|k| x >= k.x && x < k.x + k.w && y >= k.y && y < k.y + k.h)
    }

    fn mod_state(&self, m: Mod) -> ModState {
        match m {
            Mod::Shift => self.shift,
            Mod::Ctrl => self.ctrl,
            Mod::Alt => self.alt,
        }
    }

    fn set_mod(&mut self, m: Mod, s: ModState) {
        match m {
            Mod::Shift => self.shift = s,
            Mod::Ctrl => self.ctrl = s,
            Mod::Alt => self.alt = s,
        }
    }

    /// Touch press at (x, y): returns events to act on.
    pub fn press(&mut self, dev_id: i32, x: i32, y: i32, fb: &mut [u8], font: &Font) -> Vec<OskEvent> {
        let Some(idx) = self.hit(x, y) else { return vec![] };
        self.pressed.insert(dev_id, idx);
        let mut dirty = vec![];
        let mut out = vec![];

        match self.keys[idx].def {
            KeyDef::Modifier(m, _) => {
                let next = match self.mod_state(m) {
                    ModState::Off => ModState::Oneshot,
                    ModState::Oneshot => ModState::Locked,
                    ModState::Locked => ModState::Off,
                };
                self.set_mod(m, next);
                if m == Mod::Shift {
                    // Shift changes every key label: redraw the whole band.
                    self.draw_all(fb, font);
                    dirty.push((0, self.y0, SCREEN_W, self.height));
                } else {
                    self.draw_key(fb, font, idx, true);
                    dirty.push(self.key_rect(idx));
                }
            }
            KeyDef::FullRefresh => {
                self.draw_key(fb, font, idx, true);
                dirty.push(self.key_rect(idx));
                out.push(OskEvent::FullRefresh);
            }
            KeyDef::Char(c, shifted) => {
                let shift_on = self.shift != ModState::Off;
                let ch = if shift_on { shifted } else { c };
                let ctrl = self.ctrl != ModState::Off;
                let alt = self.alt != ModState::Off;
                out.push(OskEvent::Send(KeyAction::Char(ch), ctrl, alt));
                self.draw_key(fb, font, idx, true);
                dirty.push(self.key_rect(idx));
                dirty.extend(self.consume_oneshots(fb, font));
            }
            KeyDef::Named(action, _) => {
                let ctrl = self.ctrl != ModState::Off;
                let alt = self.alt != ModState::Off;
                out.push(OskEvent::Send(action, ctrl, alt));
                self.draw_key(fb, font, idx, true);
                dirty.push(self.key_rect(idx));
                dirty.extend(self.consume_oneshots(fb, font));
            }
        }
        out.push(OskEvent::Redraw(dirty));
        out
    }

    pub fn release(&mut self, dev_id: i32, fb: &mut [u8], font: &Font) -> Vec<OskEvent> {
        let Some(idx) = self.pressed.remove(&dev_id) else { return vec![] };
        self.draw_key(fb, font, idx, false);
        vec![OskEvent::Redraw(vec![self.key_rect(idx)])]
    }

    fn consume_oneshots(&mut self, fb: &mut [u8], font: &Font) -> Vec<(usize, usize, usize, usize)> {
        let mut dirty = vec![];
        let mut shift_changed = false;
        for m in [Mod::Shift, Mod::Ctrl, Mod::Alt] {
            if self.mod_state(m) == ModState::Oneshot {
                self.set_mod(m, ModState::Off);
                if m == Mod::Shift {
                    shift_changed = true;
                    continue;
                }
                for (i, k) in self.keys.iter().enumerate() {
                    if let KeyDef::Modifier(km, _) = k.def {
                        if km == m {
                            dirty.push((k.x, k.y, k.w, k.h));
                            let pressed = self.pressed.values().any(|&p| p == i);
                            self.draw_key_i(fb, font, i, pressed);
                        }
                    }
                }
            }
        }
        if shift_changed {
            // Labels revert to unshifted: redraw the whole band.
            self.draw_all(fb, font);
            dirty.push((0, self.y0, SCREEN_W, self.height));
        }
        dirty
    }

    fn key_rect(&self, idx: usize) -> (usize, usize, usize, usize) {
        let k = &self.keys[idx];
        (k.x, k.y, k.w, k.h)
    }

    pub fn draw_all(&self, fb: &mut [u8], font: &Font) {
        render::fill_rect(fb, 0, self.y0, SCREEN_W, self.height, render::WHITE);
        for i in 0..self.keys.len() {
            let pressed = self.pressed.values().any(|&p| p == i);
            self.draw_key_i(fb, font, i, pressed);
        }
    }

    fn draw_key(&self, fb: &mut [u8], font: &Font, idx: usize, pressed: bool) {
        self.draw_key_i(fb, font, idx, pressed);
    }

    fn draw_key_i(&self, fb: &mut [u8], font: &Font, idx: usize, pressed: bool) {
        let k = &self.keys[idx];
        let (mut fg, mut bg) = (render::BLACK, render::WHITE);
        let latched = match k.def {
            KeyDef::Modifier(m, _) => self.mod_state(m),
            _ => ModState::Off,
        };
        if latched == ModState::Locked || pressed {
            std::mem::swap(&mut fg, &mut bg);
        }
        // Key body + border.
        render::fill_rect(fb, k.x, k.y, k.w, k.h, bg);
        let bw = 2;
        render::fill_rect(fb, k.x, k.y, k.w, bw, fg);
        render::fill_rect(fb, k.x, k.y + k.h - bw, k.w, bw, fg);
        render::fill_rect(fb, k.x, k.y, bw, k.h, fg);
        render::fill_rect(fb, k.x + k.w - bw, k.y, bw, k.h, fg);
        // One-shot latch marker: thick inner top bar.
        if latched == ModState::Oneshot {
            render::fill_rect(fb, k.x + bw, k.y + bw, k.w - 2 * bw, 8, fg);
        }
        // Label: char keys show their shifted glyph while shift is engaged.
        let label: String = match k.def {
            KeyDef::Char(c, shifted) => {
                if self.shift != ModState::Off { shifted.to_string() } else { c.to_string() }
            }
            KeyDef::Named(_, l) | KeyDef::Modifier(_, l) => l.to_string(),
            KeyDef::FullRefresh => "rfsh".to_string(),
        };
        let tw = label.chars().count() * font.width;
        let tx = k.x + k.w.saturating_sub(tw) / 2;
        let ty = k.y + k.h.saturating_sub(font.height) / 2;
        render::draw_text(fb, font, tx, ty, &label, fg, bg);
    }
}

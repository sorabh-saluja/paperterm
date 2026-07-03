//! Renderer-owned base color palette. alacritty_terminal's Term only stores
//! runtime overrides (OSC 4/10/11...); the embedder supplies the defaults.
//! Tuned for e-ink: default fg black, bg white.

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};

pub struct Palette {
    base: [Rgb; 269],
}

impl Palette {
    pub fn new() -> Self {
        let mut base = [Rgb { r: 0, g: 0, b: 0 }; 269];
        const ANSI: [(u8, u8, u8); 16] = [
            (0, 0, 0),
            (170, 0, 0),
            (0, 130, 0),
            (150, 90, 0),
            (0, 0, 170),
            (150, 0, 150),
            (0, 140, 140),
            (190, 190, 190),
            (85, 85, 85),
            (255, 60, 60),
            (0, 180, 0),
            (200, 130, 0),
            (70, 70, 255),
            (220, 80, 220),
            (0, 190, 190),
            (255, 255, 255),
        ];
        for (i, (r, g, b)) in ANSI.iter().enumerate() {
            base[i] = Rgb { r: *r, g: *g, b: *b };
        }
        for i in 0..216 {
            let (r, g, b) = (i / 36, (i / 6) % 6, i % 6);
            let c = |v: usize| if v == 0 { 0 } else { (v * 40 + 55) as u8 };
            base[16 + i] = Rgb { r: c(r), g: c(g), b: c(b) };
        }
        for i in 0..24 {
            let v = (i * 10 + 8) as u8;
            base[232 + i] = Rgb { r: v, g: v, b: v };
        }
        base[NamedColor::Foreground as usize] = Rgb { r: 0, g: 0, b: 0 };
        base[NamedColor::Background as usize] = Rgb { r: 255, g: 255, b: 255 };
        base[NamedColor::Cursor as usize] = Rgb { r: 0, g: 0, b: 0 };
        base[NamedColor::BrightForeground as usize] = Rgb { r: 0, g: 0, b: 0 };
        base[NamedColor::DimForeground as usize] = Rgb { r: 96, g: 96, b: 96 };
        for i in 0..8 {
            let n = base[i];
            base[NamedColor::DimBlack as usize + i] = Rgb {
                r: (n.r as u16 * 3 / 4) as u8,
                g: (n.g as u16 * 3 / 4) as u8,
                b: (n.b as u16 * 3 / 4) as u8,
            };
        }
        Self { base }
    }

    pub fn resolve(&self, color: Color, overrides: &Colors) -> Rgb {
        match color {
            Color::Spec(rgb) => rgb,
            Color::Named(named) => overrides[named as usize].unwrap_or(self.base[named as usize]),
            Color::Indexed(idx) => {
                overrides[idx as usize].unwrap_or(self.base[idx as usize])
            }
        }
    }
}

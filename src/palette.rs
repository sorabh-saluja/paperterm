//! Renderer-owned base color palette. alacritty_terminal's Term only stores
//! runtime overrides (OSC 4/10/11...); the embedder supplies the defaults.
//! Tuned for e-ink: default fg black, bg white, or the reverse in dark mode. The
//! canvas is inverted on its way to the panel in dark mode, so colours are
//! inverted here too and reach the panel as written.

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};

const ANSI_DARK: [(u8, u8, u8); 16] = [
    (0, 0, 0),
    (230, 70, 70),
    (60, 190, 60),
    (220, 170, 40),
    (90, 130, 255),
    (210, 90, 210),
    (60, 190, 190),
    (200, 200, 200),
    (130, 130, 130),
    (255, 110, 110),
    (110, 230, 110),
    (250, 210, 90),
    (130, 165, 255),
    (240, 130, 240),
    (110, 230, 230),
    (255, 255, 255),
];

pub struct Palette {
    base: [Rgb; 269],
    dark: bool,
}

impl Palette {
    pub fn new(dark: bool) -> Self {
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
        let ansi = if dark { &ANSI_DARK } else { &ANSI };
        for (i, (r, g, b)) in ansi.iter().enumerate() {
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
        let (ink, paper, dim) = if dark { (255, 0, 170) } else { (0, 255, 96) };
        let grey = |v: u8| Rgb { r: v, g: v, b: v };
        base[NamedColor::Foreground as usize] = grey(ink);
        base[NamedColor::Background as usize] = grey(paper);
        base[NamedColor::Cursor as usize] = grey(ink);
        base[NamedColor::BrightForeground as usize] = grey(ink);
        base[NamedColor::DimForeground as usize] = grey(dim);
        for i in 0..8 {
            let n = base[i];
            base[NamedColor::DimBlack as usize + i] = Rgb {
                r: (n.r as u16 * 3 / 4) as u8,
                g: (n.g as u16 * 3 / 4) as u8,
                b: (n.b as u16 * 3 / 4) as u8,
            };
        }
        Self { base, dark }
    }

    pub fn resolve(&self, color: Color, overrides: &Colors) -> Rgb {
        let rgb = match color {
            Color::Spec(rgb) => rgb,
            Color::Named(named) => overrides[named as usize].unwrap_or(self.base[named as usize]),
            Color::Indexed(idx) => {
                overrides[idx as usize].unwrap_or(self.base[idx as usize])
            }
        };
        if self.dark {
            Rgb { r: !rgb.r, g: !rgb.g, b: !rgb.b }
        } else {
            rgb
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_mode_puts_light_text_on_black_at_the_panel() {
        let palette = Palette::new(true);
        let colors = Colors::default();
        let at_panel = |c: Color| {
            let rgb = palette.resolve(c, &colors);
            (!rgb.r, !rgb.g, !rgb.b)
        };
        assert_eq!(at_panel(Color::Named(NamedColor::Foreground)), (255, 255, 255));
        assert_eq!(at_panel(Color::Named(NamedColor::Background)), (0, 0, 0));
        assert_eq!(at_panel(Color::Spec(Rgb { r: 10, g: 20, b: 30 })), (10, 20, 30));
    }

    #[test]
    fn light_mode_passes_colours_through() {
        let palette = Palette::new(false);
        let colors = Colors::default();
        let text = palette.resolve(Color::Named(NamedColor::Foreground), &colors);
        assert_eq!((text.r, text.g, text.b), (0, 0, 0));
        let spec = palette.resolve(Color::Spec(Rgb { r: 10, g: 20, b: 30 }), &colors);
        assert_eq!((spec.r, spec.g, spec.b), (10, 20, 30));
    }
}

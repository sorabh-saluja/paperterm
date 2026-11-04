//! Pixel-level drawing into the RGB565 shared-memory framebuffer.

use crate::font::Font;
use crate::palette::Palette;
use alacritty_terminal::event::EventListener;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::Rgb;

pub const SCREEN_W: usize = 1620;

#[inline]
pub fn rgb565(c: Rgb) -> u16 {
    (((c.r as u16) >> 3) << 11) | (((c.g as u16) >> 2) << 5) | ((c.b as u16) >> 3)
}

pub const WHITE: u16 = 0xFFFF;
pub const BLACK: u16 = 0x0000;

#[inline]
fn put_px(fb: &mut [u8], x: usize, y: usize, c: u16) {
    let i = (y * SCREEN_W + x) * 2;
    fb[i] = (c & 0xff) as u8;
    fb[i + 1] = (c >> 8) as u8;
}

pub fn fill_rect(fb: &mut [u8], x: usize, y: usize, w: usize, h: usize, c: u16) {
    let lo = (c & 0xff) as u8;
    let hi = (c >> 8) as u8;
    for row in y..(y + h).min(2160) {
        let start = (row * SCREEN_W + x) * 2;
        let end = start + w.min(SCREEN_W - x) * 2;
        let slice = &mut fb[start..end];
        for px in slice.chunks_exact_mut(2) {
            px[0] = lo;
            px[1] = hi;
        }
    }
}

pub fn invert_rect(fb: &mut [u8], x: usize, y: usize, w: usize, h: usize) {
    for row in y..(y + h).min(2160) {
        let start = (row * SCREEN_W + x) * 2;
        let end = start + w.min(SCREEN_W - x) * 2;
        for b in &mut fb[start..end] {
            *b = !*b;
        }
    }
}

/// Draw one glyph cell. `bold` uses classic double-strike.
pub fn draw_glyph(
    fb: &mut [u8],
    font: &Font,
    x: usize,
    y: usize,
    c: char,
    fg: u16,
    bg: u16,
    bold: bool,
) {
    let glyph = font.glyph(c);
    for gy in 0..font.height {
        let row = &glyph[gy * font.width..(gy + 1) * font.width];
        let py = y + gy;
        let mut prev = 0;
        for (gx, &coverage) in row.iter().enumerate() {
            let ink = if bold { coverage.max(prev) } else { coverage };
            put_px(fb, x + gx, py, blend(fg, bg, ink));
            prev = coverage;
        }
    }
}

/// Mix two RGB565 colours: `ink` 0 gives `bg`, 255 gives `fg`.
#[inline]
fn blend(fg: u16, bg: u16, ink: u8) -> u16 {
    match ink {
        0 => bg,
        255 => fg,
        _ => {
            let a = ink as u32;
            let mix = |shift: u32, mask: u32| {
                let f = (fg as u32 >> shift) & mask;
                let b = (bg as u32 >> shift) & mask;
                (((f * a + b * (255 - a)) / 255) << shift) as u16
            };
            mix(11, 0x1F) | mix(5, 0x3F) | mix(0, 0x1F)
        }
    }
}

/// Draw a text label (no wrapping) — used by the on-screen keyboard.
pub fn draw_text(fb: &mut [u8], font: &Font, x: usize, y: usize, s: &str, fg: u16, bg: u16) {
    let mut cx = x;
    for c in s.chars() {
        if cx + font.width > SCREEN_W {
            break;
        }
        draw_glyph(fb, font, cx, y, c, fg, bg, false);
        cx += font.width;
    }
}

/// Terminal grid geometry on screen.
pub struct TermArea {
    pub x0: usize,
    pub y0: usize,
    pub cols: usize,
    pub rows: usize,
    pub cw: usize,
    pub ch: usize,
}

impl TermArea {
    pub fn cell_rect(&self, col: usize, row: usize) -> (usize, usize, usize, usize) {
        (self.x0 + col * self.cw, self.y0 + row * self.ch, self.cw, self.ch)
    }
}

/// Render one damaged viewport line from the grid into the framebuffer.
pub fn draw_line<T: EventListener>(
    fb: &mut [u8],
    font: &Font,
    palette: &Palette,
    term: &Term<T>,
    area: &TermArea,
    row: usize,
    left: usize,
    right: usize,
) {
    let display_offset = term.grid().display_offset();
    let line = Line(row as i32 - display_offset as i32);
    let colors = term.colors();
    let right = right.min(area.cols - 1);

    for col in left..=right {
        let cell = &term.grid()[line][Column(col)];
        let (px, py, _, _) = area.cell_rect(col, row);
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            let bg = rgb565(palette.resolve(cell.bg, colors));
            fill_rect(fb, px, py, area.cw, area.ch, bg);
            continue;
        }
        let mut fg = palette.resolve(cell.fg, colors);
        let mut bg = palette.resolve(cell.bg, colors);
        if cell.flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        let fg565 = rgb565(fg);
        let bg565 = rgb565(bg);
        let glyph_char = if cell.flags.contains(Flags::HIDDEN) { ' ' } else { cell.c };
        let bold = cell.flags.intersects(Flags::BOLD | Flags::DIM_BOLD);
        draw_glyph(fb, font, px, py, glyph_char, fg565, bg565, bold);
        if cell.flags.intersects(Flags::ALL_UNDERLINES) {
            fill_rect(fb, px, py + area.ch - 2, area.cw, 2, fg565);
        }
        if cell.flags.contains(Flags::STRIKEOUT) {
            fill_rect(fb, px, py + area.ch / 2, area.cw, 1, fg565);
        }
    }

    // Cursor: block-invert its cell when it sits on this row.
    if term.mode().contains(TermMode::SHOW_CURSOR) && display_offset == 0 {
        let cursor: Point = term.grid().cursor.point;
        let crow = cursor.line.0 as usize;
        let ccol = cursor.column.0;
        if crow == row && ccol >= left && ccol <= right {
            let (px, py, w, h) = area.cell_rect(ccol, row);
            invert_rect(fb, px, py, w, h);
        }
    }
}

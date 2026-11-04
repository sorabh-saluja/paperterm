//! Minimal PSF2 bitmap font parser (format verified against the vendored
//! Spleen .psfu files: 32-byte LE header, MSB-first row bitmaps, UTF-8
//! unicode table terminated per-glyph by 0xFF, 0xFE starting ignorable
//! combining sequences).

use ab_glyph::{point, Font as _, FontRef, PxScale, ScaleFont};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

pub const DEFAULT_SIZE: usize = 48;
const OUTLINE_SIZES: [usize; 14] = [28, 32, 36, 40, 44, 48, 52, 56, 60, 64, 68, 72, 76, 80];

const JETBRAINS: &[u8] = include_bytes!("../fonts/JetBrainsMono-Medium.ttf");
const FIRA: &[u8] = include_bytes!("../fonts/FiraCode-Medium.ttf");
const HACK: &[u8] = include_bytes!("../fonts/Hack-Regular.ttf");
const DEJAVU: &[u8] = include_bytes!("../fonts/DejaVuSansMono.ttf");
const SPLEEN: &[(usize, &[u8])] = &[
    (24, include_bytes!("../fonts/spleen-12x24.psfu")),
    (32, include_bytes!("../fonts/spleen-16x32.psfu")),
    (64, include_bytes!("../fonts/spleen-32x64.psfu")),
];

enum Source {
    Outline(&'static [u8]),
    Bitmap(&'static [(usize, &'static [u8])]),
}

pub struct FontChoice {
    pub name: &'static str,
    source: Source,
}

pub const FONTS: &[FontChoice] = &[
    FontChoice { name: "JetBrains Mono", source: Source::Outline(JETBRAINS) },
    FontChoice { name: "Fira Code", source: Source::Outline(FIRA) },
    FontChoice { name: "Hack", source: Source::Outline(HACK) },
    FontChoice { name: "DejaVu Sans Mono", source: Source::Outline(DEJAVU) },
    FontChoice { name: "Spleen", source: Source::Bitmap(SPLEEN) },
];

impl FontChoice {
    /// The sizes this font offers, smallest first. Size is the line height in pixels.
    pub fn sizes(&self) -> Vec<usize> {
        match self.source {
            Source::Outline(_) => OUTLINE_SIZES.to_vec(),
            Source::Bitmap(faces) => faces.iter().map(|f| f.0).collect(),
        }
    }

    pub fn nearest_size(&self, size: usize) -> usize {
        self.sizes()
            .into_iter()
            .min_by_key(|s| s.abs_diff(size))
            .unwrap_or(size)
    }
}

enum Raster {
    Outline {
        face: FontRef<'static>,
        /// Covers symbols the chosen face lacks.
        fallback: FontRef<'static>,
        px: f32,
        baseline: f32,
    },
    Bitmap(Psf2Font),
}

/// A monospace font at one size. Every glyph becomes one cell of coverage bytes
/// (0 = background, 255 = ink), whatever kind of font it came from.
pub struct Font {
    pub width: usize,
    pub height: usize,
    crisp: bool,
    raster: Raster,
    cache: RefCell<HashMap<char, Rc<[u8]>>>,
}

impl Font {
    /// `index` is a position in FONTS. `crisp` draws pure black and white, with
    /// no grey edges.
    pub fn load(index: usize, size: usize, crisp: bool) -> Result<Self, String> {
        let raster = match FONTS[index].source {
            Source::Outline(data) => {
                let face = FontRef::try_from_slice(data).map_err(|e| e.to_string())?;
                let fallback = FontRef::try_from_slice(DEJAVU).map_err(|e| e.to_string())?;
                let scaled = face.as_scaled(PxScale::from(size as f32));
                Raster::Outline {
                    baseline: scaled.ascent().round(),
                    px: size as f32,
                    face,
                    fallback,
                }
            }
            Source::Bitmap(faces) => {
                let data = faces
                    .iter()
                    .find(|f| f.0 == size)
                    .or(faces.first())
                    .ok_or("bitmap font has no sizes")?
                    .1;
                Raster::Bitmap(Psf2Font::parse(data)?)
            }
        };
        let (width, height) = match &raster {
            Raster::Outline { face, px, .. } => {
                let scaled = face.as_scaled(PxScale::from(*px));
                let advance = scaled.h_advance(face.glyph_id('M'));
                let height = scaled.ascent() - scaled.descent();
                (advance.round() as usize, height.ceil() as usize)
            }
            Raster::Bitmap(psf) => (psf.width, psf.height),
        };
        Ok(Self {
            width,
            height,
            crisp,
            raster,
            cache: RefCell::new(HashMap::new()),
        })
    }

    /// `width * height` coverage bytes for `c`, top row first.
    pub fn glyph(&self, c: char) -> Rc<[u8]> {
        if let Some(cell) = self.cache.borrow().get(&c) {
            return Rc::clone(cell);
        }
        let cell: Rc<[u8]> = self.rasterize(c).into();
        self.cache.borrow_mut().insert(c, Rc::clone(&cell));
        cell
    }

    fn rasterize(&self, c: char) -> Vec<u8> {
        let mut cell = vec![0u8; self.width * self.height];
        match &self.raster {
            Raster::Bitmap(psf) => {
                let bits = psf.glyph(c);
                let bpr = psf.bytes_per_row();
                for y in 0..self.height {
                    for x in 0..self.width {
                        if bits[y * bpr + x / 8] & (0x80 >> (x % 8)) != 0 {
                            cell[y * self.width + x] = 255;
                        }
                    }
                }
            }
            Raster::Outline { face, fallback, px, baseline } => {
                let source = if face.glyph_id(c).0 != 0 { face } else { fallback };
                let id = source.glyph_id(c);
                // A fallback glyph can be wider than our cell: shrink it to fit.
                let advance = source.as_scaled(PxScale::from(*px)).h_advance(id);
                let fit = (self.width as f32 / advance.max(1.0)).min(1.0);
                let glyph = id.with_scale_and_position(*px * fit, point(0.0, *baseline));
                if let Some(outline) = source.outline_glyph(glyph) {
                    let bounds = outline.px_bounds();
                    outline.draw(|gx, gy, coverage| {
                        let x = bounds.min.x as i32 + gx as i32;
                        let y = bounds.min.y as i32 + gy as i32;
                        if x >= 0 && y >= 0 && (x as usize) < self.width && (y as usize) < self.height {
                            let i = y as usize * self.width + x as usize;
                            cell[i] = cell[i].max(self.shade(coverage));
                        }
                    });
                }
            }
        }
        cell
    }

    fn shade(&self, coverage: f32) -> u8 {
        if self.crisp {
            return if coverage >= 0.5 { 255 } else { 0 };
        }
        // Thin grey edges wash out on e-ink, so lean towards ink.
        (coverage.clamp(0.0, 1.0).powf(0.7) * 255.0) as u8
    }
}

pub struct Psf2Font {
    pub width: usize,
    pub height: usize,
    charsize: usize,
    bytes_per_row: usize,
    glyphs: &'static [u8],
    map: HashMap<char, usize>,
    fallback: usize,
}

impl Psf2Font {
    pub fn parse(data: &'static [u8]) -> Result<Self, String> {
        if data.len() < 32 || data[0..4] != [0x72, 0xB5, 0x4A, 0x86] {
            return Err("not a PSF2 font".into());
        }
        let u32le = |off: usize| -> usize {
            u32::from_le_bytes(data[off..off + 4].try_into().unwrap()) as usize
        };
        let headersize = u32le(8);
        let flags = u32le(12);
        let length = u32le(16);
        let charsize = u32le(20);
        let height = u32le(24);
        let width = u32le(28);
        let bytes_per_row = width.div_ceil(8);
        if charsize != bytes_per_row * height {
            return Err(format!("charsize mismatch: {} != {}", charsize, bytes_per_row * height));
        }
        let bitmaps_end = headersize + length * charsize;
        if data.len() < bitmaps_end {
            return Err("truncated glyph table".into());
        }

        let mut map = HashMap::new();
        if flags & 1 != 0 {
            let mut pos = bitmaps_end;
            let mut glyph = 0usize;
            while pos < data.len() && glyph < length {
                match data[pos] {
                    0xFF => {
                        glyph += 1;
                        pos += 1;
                    }
                    0xFE => {
                        // Combining sequences: skip to this glyph's terminator.
                        while pos < data.len() && data[pos] != 0xFF {
                            pos += 1;
                        }
                    }
                    _ => {
                        // One UTF-8 encoded codepoint mapping to `glyph`.
                        let len = utf8_len(data[pos]);
                        if pos + len <= data.len() {
                            if let Ok(s) = std::str::from_utf8(&data[pos..pos + len]) {
                                if let Some(c) = s.chars().next() {
                                    map.entry(c).or_insert(glyph);
                                }
                            }
                        }
                        pos += len;
                    }
                }
            }
        } else {
            // No unicode table: identity map for latin-1.
            for i in 0..length.min(256) {
                map.insert(char::from_u32(i as u32).unwrap(), i);
            }
        }

        let fallback = *map.get(&'?').unwrap_or(&0);
        Ok(Self {
            width,
            height,
            charsize,
            bytes_per_row,
            glyphs: &data[headersize..bitmaps_end],
            map,
            fallback,
        })
    }

    /// Glyph bitmap: `height` rows of `bytes_per_row` bytes, MSB = leftmost.
    pub fn glyph(&self, c: char) -> &[u8] {
        let idx = *self.map.get(&c).unwrap_or(&self.fallback);
        &self.glyphs[idx * self.charsize..(idx + 1) * self.charsize]
    }

    pub fn bytes_per_row(&self) -> usize {
        self.bytes_per_row
    }
}

fn utf8_len(first: u8) -> usize {
    match first {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ink(font: &Font, c: char) -> usize {
        font.glyph(c).iter().filter(|&&a| a > 0).count()
    }

    #[test]
    fn every_font_loads_at_every_size_with_ink_and_blank_space() {
        for (i, choice) in FONTS.iter().enumerate() {
            for size in choice.sizes() {
                let font = Font::load(i, size, false).unwrap();
                assert!(font.width > 0 && font.height > 0, "{}", choice.name);
                assert_eq!(font.glyph('A').len(), font.width * font.height);
                assert!(ink(&font, 'A') > 0, "{} {} has no ink", choice.name, size);
                assert_eq!(ink(&font, ' '), 0, "{} {} space has ink", choice.name, size);
            }
        }
    }

    #[test]
    fn crisp_text_has_no_grey() {
        let font = Font::load(0, DEFAULT_SIZE, true).unwrap();
        assert!(font.glyph('g').iter().all(|&a| a == 0 || a == 255));
        let smooth = Font::load(0, DEFAULT_SIZE, false).unwrap();
        assert!(smooth.glyph('g').iter().any(|&a| a != 0 && a != 255));
    }

    #[test]
    fn missing_symbols_fall_back_to_dejavu() {
        // Braille is in DejaVu Sans Mono but not in Hack.
        let font = Font::load(2, DEFAULT_SIZE, false).unwrap();
        assert!(ink(&font, '\u{28FF}') > 0);
    }
}

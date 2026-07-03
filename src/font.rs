//! Minimal PSF2 bitmap font parser (format verified against the vendored
//! Spleen .psfu files: 32-byte LE header, MSB-first row bitmaps, UTF-8
//! unicode table terminated per-glyph by 0xFF, 0xFE starting ignorable
//! combining sequences).

use std::collections::HashMap;

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

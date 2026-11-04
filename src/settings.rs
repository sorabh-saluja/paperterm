//! User settings, stored as `key=value` lines so a bad or old file can never
//! stop the terminal from starting: anything unrecognised keeps its default.

use crate::font::{self, FONTS};
use crate::qtfb;
use std::path::PathBuf;

pub struct RefreshOption {
    pub key: &'static str,
    pub label: &'static str,
    pub hint: &'static str,
    pub mode: i32,
}

pub const REFRESH_OPTIONS: &[RefreshOption] = &[
    RefreshOption {
        key: "fastest",
        label: "Fastest",
        hint: "Quickest response, most ghosting",
        mode: qtfb::REFRESH_MODE_UFAST,
    },
    RefreshOption {
        key: "fast",
        label: "Fast",
        hint: "Quick typing, some ghosting",
        mode: qtfb::REFRESH_MODE_FAST,
    },
    RefreshOption {
        key: "animate",
        label: "Animate",
        hint: "Meant for scrolling and moving content",
        mode: qtfb::REFRESH_MODE_ANIMATE,
    },
    RefreshOption {
        key: "balanced",
        label: "Balanced",
        hint: "Slower, cleaner text",
        mode: qtfb::REFRESH_MODE_CONTENT,
    },
    RefreshOption {
        key: "sharpest",
        label: "Sharpest",
        hint: "Slowest, best quality",
        mode: qtfb::REFRESH_MODE_UI,
    },
];
const DEFAULT_REFRESH: usize = 1;

/// (label, screen updates between automatic full refreshes; 0 = never)
pub const CLEANUP_OPTIONS: &[(&str, u32)] = &[("Off", 0), ("Every 500", 500), ("Every 100", 100)];

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub font: usize,
    pub size: usize,
    pub crisp: bool,
    pub refresh: usize,
    pub cleanup: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            font: 0,
            size: font::DEFAULT_SIZE,
            crisp: false,
            refresh: DEFAULT_REFRESH,
            cleanup: 0,
        }
    }
}

impl Settings {
    pub fn parse(text: &str) -> Self {
        let mut s = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            let value = value.trim();
            match key.trim() {
                "font" => {
                    if let Some(i) = FONTS.iter().position(|f| f.name == value) {
                        s.font = i;
                    }
                }
                "size" => s.size = value.parse().unwrap_or(s.size),
                "text" => s.crisp = value == "crisp",
                "refresh" => {
                    if let Some(i) = REFRESH_OPTIONS.iter().position(|o| o.key == value) {
                        s.refresh = i;
                    }
                }
                "cleanup" => {
                    let updates = value.parse().unwrap_or(0);
                    if let Some(i) = CLEANUP_OPTIONS.iter().position(|o| o.1 == updates) {
                        s.cleanup = i;
                    }
                }
                _ => {}
            }
        }
        s.size = FONTS[s.font].nearest_size(s.size);
        s
    }

    pub fn to_text(&self) -> String {
        format!(
            "font={}\nsize={}\ntext={}\nrefresh={}\ncleanup={}\n",
            FONTS[self.font].name,
            self.size,
            if self.crisp { "crisp" } else { "smooth" },
            REFRESH_OPTIONS[self.refresh].key,
            CLEANUP_OPTIONS[self.cleanup].1,
        )
    }

    pub fn load() -> Self {
        std::fs::read_to_string(path())
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, self.to_text())?;
        std::fs::rename(&tmp, &path)
    }
}

fn path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/root".into());
    PathBuf::from(home).join(".config/paperterm/settings.conf")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let s = Settings {
            font: 2,
            size: 40,
            crisp: true,
            refresh: 3,
            cleanup: 2,
        };
        assert_eq!(Settings::parse(&s.to_text()), s);
    }

    #[test]
    fn bad_values_keep_defaults() {
        let s = Settings::parse("font=Nope\nsize=abc\nrefresh=warp\ncleanup=7\njunk\n");
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn size_snaps_to_what_the_font_offers() {
        let s = Settings::parse("font=Spleen\nsize=48\n");
        assert_eq!(s.size, 32);
    }
}

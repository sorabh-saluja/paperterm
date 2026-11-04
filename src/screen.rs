//! Everything draws upright into a `Canvas`. AppLoad keeps its framebuffer in
//! the panel's native 1620x2160 layout, so when the tablet is turned, pixels
//! and touches are converted here (matching rm-appload's FBController).

pub const PANEL_W: usize = 1620;
pub const PANEL_H: usize = 2160;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rotation {
    Deg0,
    Deg90L,
    Deg90R,
    Deg180,
}

impl Rotation {
    /// The value in AppLoad's rotation message.
    pub fn from_wire(value: i32) -> Option<Self> {
        match value {
            0 => Some(Self::Deg0),
            1 => Some(Self::Deg90L),
            2 => Some(Self::Deg90R),
            3 => Some(Self::Deg180),
            _ => None,
        }
    }

    pub fn is_landscape(self) -> bool {
        matches!(self, Self::Deg90L | Self::Deg90R)
    }

    /// Canvas position to framebuffer position.
    #[inline]
    fn to_panel(self, x: usize, y: usize) -> (usize, usize) {
        match self {
            Self::Deg0 => (x, y),
            Self::Deg90L => (PANEL_W - 1 - y, x),
            Self::Deg90R => (y, PANEL_H - 1 - x),
            Self::Deg180 => (PANEL_W - 1 - x, PANEL_H - 1 - y),
        }
    }

    /// Framebuffer position, as touches report it, to canvas position.
    pub fn to_canvas(self, x: i32, y: i32) -> (i32, i32) {
        let (w, h) = (PANEL_W as i32, PANEL_H as i32);
        match self {
            Self::Deg0 => (x, y),
            Self::Deg90L => (y, w - 1 - x),
            Self::Deg90R => (h - 1 - y, x),
            Self::Deg180 => (w - 1 - x, h - 1 - y),
        }
    }

    fn rect_to_panel(self, (x, y, w, h): Rect) -> Rect {
        match self {
            Self::Deg0 => (x, y, w, h),
            Self::Deg90L => (PANEL_W - y - h, x, h, w),
            Self::Deg90R => (y, PANEL_H - x - w, h, w),
            Self::Deg180 => (PANEL_W - x - w, PANEL_H - y - h, w, h),
        }
    }
}

pub type Rect = (usize, usize, usize, usize);

/// An upright RGB565 picture of the screen. Everything draws as if the theme were
/// light; in dark mode the picture is inverted on its way to the panel.
pub struct Canvas {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u8>,
    pub dark: bool,
}

impl Canvas {
    pub fn new(rotation: Rotation) -> Self {
        let (w, h) = if rotation.is_landscape() {
            (PANEL_H, PANEL_W)
        } else {
            (PANEL_W, PANEL_H)
        };
        Self {
            w,
            h,
            px: vec![0xFF; w * h * 2],
            dark: false,
        }
    }

    /// Copy `rect` into the framebuffer. Returns the framebuffer area it touched.
    pub fn present(&self, rotation: Rotation, rect: Rect, panel: &mut [u8]) -> Rect {
        let (x, y) = (rect.0.min(self.w), rect.1.min(self.h));
        let (w, h) = (rect.2.min(self.w - x), rect.3.min(self.h - y));
        let flip = if self.dark { 0xFF } else { 0 };
        for row in y..y + h {
            let line = &self.px[(row * self.w + x) * 2..(row * self.w + x + w) * 2];
            if rotation == Rotation::Deg0 {
                let out = &mut panel[(row * PANEL_W + x) * 2..(row * PANEL_W + x + w) * 2];
                for (to, from) in out.iter_mut().zip(line) {
                    *to = from ^ flip;
                }
                continue;
            }
            for (i, px) in line.chunks_exact(2).enumerate() {
                let (u, v) = rotation.to_panel(x + i, row);
                let at = (v * PANEL_W + u) * 2;
                panel[at] = px[0] ^ flip;
                panel[at + 1] = px[1] ^ flip;
            }
        }
        rotation.rect_to_panel((x, y, w, h))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_mode_inverts_what_reaches_the_panel() {
        for rotation in [Rotation::Deg0, Rotation::Deg90L] {
            let mut canvas = Canvas::new(rotation);
            canvas.dark = true;
            let mut panel = vec![0x55u8; PANEL_W * PANEL_H * 2];
            canvas.present(rotation, (0, 0, canvas.w, canvas.h), &mut panel);
            assert!(panel.iter().all(|b| *b == 0));
            assert!(canvas.px.iter().all(|b| *b == 0xFF));
        }
    }

    const ALL: [Rotation; 4] = [Rotation::Deg0, Rotation::Deg90L, Rotation::Deg90R, Rotation::Deg180];

    #[test]
    fn corners_match_appload_touch_mapping() {
        // The canvas's top-left and top-right corners, from rm-appload's
        // convertPointToQTFBPixels corner lists.
        assert_eq!(Rotation::Deg90L.to_panel(0, 0), (1619, 0));
        assert_eq!(Rotation::Deg90L.to_panel(2159, 0), (1619, 2159));
        assert_eq!(Rotation::Deg90R.to_panel(0, 0), (0, 2159));
        assert_eq!(Rotation::Deg90R.to_panel(2159, 0), (0, 0));
        assert_eq!(Rotation::Deg180.to_panel(0, 0), (1619, 2159));
    }

    #[test]
    fn touches_undo_the_pixel_mapping() {
        for r in ALL {
            for (x, y) in [(0, 0), (10, 20), (1000, 1500), (1619, 1619)] {
                let (u, v) = r.to_panel(x, y);
                assert_eq!(r.to_canvas(u as i32, v as i32), (x as i32, y as i32), "{r:?}");
            }
        }
    }

    #[test]
    fn present_writes_only_inside_the_rect_it_reports() {
        for r in ALL {
            let mut canvas = Canvas::new(r);
            let rect = (100, 40, 30, 20);
            for y in rect.1..rect.1 + rect.3 {
                for x in rect.0..rect.0 + rect.2 {
                    let i = (y * canvas.w + x) * 2;
                    canvas.px[i] = 0;
                    canvas.px[i + 1] = 0;
                }
            }
            let mut panel = vec![0xFFu8; PANEL_W * PANEL_H * 2];
            let (x, y, w, h) = canvas.present(r, rect, &mut panel);
            let mut black = 0;
            for v in 0..PANEL_H {
                for u in 0..PANEL_W {
                    if panel[(v * PANEL_W + u) * 2] == 0 {
                        black += 1;
                        assert!(u >= x && u < x + w && v >= y && v < y + h, "{r:?}");
                    }
                }
            }
            assert_eq!(black, rect.2 * rect.3, "{r:?}");
        }
    }

    #[test]
    fn a_rect_past_the_edge_is_clipped() {
        let canvas = Canvas::new(Rotation::Deg90L);
        let mut panel = vec![0u8; PANEL_W * PANEL_H * 2];
        let touched = canvas.present(Rotation::Deg90L, (2100, 1600, 500, 500), &mut panel);
        assert_eq!((touched.2, touched.3), (20, 60));
    }
}

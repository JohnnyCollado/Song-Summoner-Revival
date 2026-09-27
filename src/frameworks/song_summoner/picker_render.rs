/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Host-side drawing for the picker: a small canvas with the handful of
//! operations the design needs, and the layout constants.
//!
//! The picker is drawn entirely in Rust into one 480×320 bitmap, then handed
//! to UIKit in one `CGContextDrawImage`. Doing it here rather than through
//! guest-visible CoreGraphics calls gives us gradients, rounded corners,
//! tinting and alpha that touchHLE's CG doesn't have, and it's fast enough
//! to redraw every frame while scrolling.
//!
//! The constants mirror `inspect/picker_mockup.py` (same names where there
//! is one). Change the mockup first, agree on the look, then copy the
//! numbers here. Game art is always drawn 1:1 at integer positions.

use crate::font::{Font, TextAlignment};
use crate::media::artwork::Bitmap;
use std::cell::OnceCell;

pub const W: i32 = 480;
pub const H: i32 = 320;
/// Navigation bar height.
pub const NAV_H: i32 = 44;
/// Tab bar height: fits the native 40×40 tab icons and a label.
pub const TAB_H: i32 = 56;
/// List area height: exactly 4 rows.
pub const LIST_H: i32 = H - NAV_H - TAB_H;
/// `cellbg.png` / `touchBG.png` / `list_fighter*.png` height.
pub const ROW_H: i32 = 55;
/// Section header height.
pub const HDR_H: i32 = 22;
/// The game's `MusicLibraryCell` draws its text at x=15.
pub const TEXT_X: i32 = 15;
/// Text x when a 50×50 artwork is shown at (4, 2).
pub const ART_TEXT_X: i32 = 62;
/// Titles and subtitles end here, clear of the fighter portrait's face.
pub const TEXT_MAX_X: i32 = 292;
/// 200-wide portrait, clear of the index strip.
pub const FIGHTER_X: i32 = W - 200 - 18;
pub const TAB_W: i32 = W / 4;
/// The A–Z index strip.
pub const INDEX_X: i32 = W - 16;
pub const INDEX_W: i32 = 14;
pub const INDEX_LETTERS: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ#";
/// Length of the push/pop slide.
pub const SLIDE_SECONDS: f32 = 0.25;

pub type Rgb = (u8, u8, u8);
pub const CYAN: Rgb = (90, 200, 255);
pub const WHITE: Rgb = (255, 255, 255);
pub const GREY: Rgb = (150, 150, 150);
pub const BACKGROUND: Rgb = (22, 22, 24);
pub const NAV_TOP: Rgb = (70, 70, 74);
pub const NAV_BOTTOM: Rgb = (8, 8, 10);
pub const NAV_RULE: Rgb = (40, 150, 220);
pub const BUTTON_FILL: Rgb = (28, 28, 32);
pub const BUTTON_STROKE: Rgb = (95, 95, 105);
pub const TAB_TOP: Rgb = (38, 38, 42);
pub const TAB_RULE: Rgb = (20, 20, 20);
pub const TAB_PLATE: Rgb = (60, 60, 66);
pub const HEADER_TOP: Rgb = (58, 64, 74);
pub const HEADER_BOTTOM: Rgb = (36, 40, 48);
pub const INDEX_TEXT: Rgb = (200, 200, 200);
pub const LOADING_COUNT: Rgb = (170, 170, 170);

/// Font sizes in the design are Pillow's (pixels per em); touchHLE's font
/// code scales sizes up by 1.125 to match iPhone OS, so undo that.
pub fn font_px(size: f32) -> f32 {
    size / 1.125
}

/// LiberationSans (touchHLE's UIKit font), with Noto Sans JP loaded only
/// if a Japanese title turns up.
pub struct Fonts {
    regular: Font,
    bold: Font,
    regular_ja: OnceCell<Font>,
    bold_ja: OnceCell<Font>,
}

impl Fonts {
    pub fn load() -> Fonts {
        Fonts {
            regular: Font::sans_regular(),
            bold: Font::sans_bold(),
            regular_ja: OnceCell::new(),
            bold_ja: OnceCell::new(),
        }
    }

    fn pick(&self, bold: bool, text: &str) -> &Font {
        // Liberation covers Latin, Greek and Cyrillic; anything from the CJK
        // blocks onwards needs Noto.
        let needs_ja = text.chars().any(|c| c as u32 >= 0x2E80);
        match (bold, needs_ja) {
            (false, false) => &self.regular,
            (true, false) => &self.bold,
            (false, true) => self.regular_ja.get_or_init(Font::sans_regular_ja),
            (true, true) => self.bold_ja.get_or_init(Font::sans_bold_ja),
        }
    }

    pub fn width(&self, bold: bool, size: f32, text: &str) -> f32 {
        self.pick(bold, text)
            .calculate_text_size(font_px(size), text, None)
            .0
    }

    /// `text`, cut down with "…" to fit `max_width`.
    pub fn ellipsize(&self, bold: bool, size: f32, text: &str, max_width: f32) -> String {
        if self.width(bold, size, text) <= max_width {
            return text.to_string();
        }
        let mut chars: Vec<char> = text.chars().collect();
        while !chars.is_empty() {
            chars.pop();
            let candidate: String = chars.iter().collect::<String>() + "…";
            if self.width(bold, size, &candidate) <= max_width {
                return candidate;
            }
        }
        "…".to_string()
    }
}

/// Line breaks and tabs in tags would otherwise draw as boxes.
pub fn single_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// A bitmap being drawn into, with a clip rectangle.
pub struct Canvas {
    pub bitmap: Bitmap,
    clip: (i32, i32, i32, i32),
}

impl Canvas {
    pub fn new(width: i32, height: i32) -> Canvas {
        Canvas {
            bitmap: Bitmap::new(width as u32, height as u32),
            clip: (0, 0, width, height),
        }
    }

    pub fn width(&self) -> i32 {
        self.bitmap.width as i32
    }
    pub fn height(&self) -> i32 {
        self.bitmap.height as i32
    }

    /// Restrict drawing to a rectangle (intersected with the canvas).
    pub fn set_clip(&mut self, x: i32, y: i32, w: i32, h: i32) {
        self.clip = (
            x.max(0),
            y.max(0),
            (x + w).min(self.width()),
            (y + h).min(self.height()),
        );
    }
    pub fn reset_clip(&mut self) {
        self.clip = (0, 0, self.width(), self.height());
    }

    /// Composite one premultiplied pixel (0.0–1.0 channels) over the canvas.
    fn blend(&mut self, x: i32, y: i32, src: [f32; 4]) {
        let (x0, y0, x1, y1) = self.clip;
        if x < x0 || y < y0 || x >= x1 || y >= y1 || src[3] <= 0.0 {
            return;
        }
        let i = (y as usize * self.bitmap.width as usize + x as usize) * 4;
        let px = &mut self.bitmap.pixels[i..i + 4];
        let keep = 1.0 - src[3].min(1.0);
        for c in 0..4 {
            let value = src[c] * 255.0 + f32::from(px[c]) * keep;
            px[c] = value.round().clamp(0.0, 255.0) as u8;
        }
    }

    fn color(rgb: Rgb, alpha: f32) -> [f32; 4] {
        let a = alpha.clamp(0.0, 1.0);
        [
            f32::from(rgb.0) / 255.0 * a,
            f32::from(rgb.1) / 255.0 * a,
            f32::from(rgb.2) / 255.0 * a,
            a,
        ]
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, rgb: Rgb, alpha: f32) {
        let src = Self::color(rgb, alpha);
        for yy in y..y + h {
            for xx in x..x + w {
                self.blend(xx, yy, src);
            }
        }
    }

    /// A vertical gradient, top colour on the first row, bottom on the last.
    pub fn vgradient(&mut self, x: i32, y: i32, w: i32, h: i32, top: Rgb, bottom: Rgb) {
        for row in 0..h {
            let t = row as f32 / (h - 1).max(1) as f32;
            let mix = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
            let rgb = (mix(top.0, bottom.0), mix(top.1, bottom.1), mix(top.2, bottom.2));
            self.fill_rect(x, y + row, w, 1, rgb, 1.0);
        }
    }

    /// How much of pixel (px, py) lies inside a rounded rectangle, sampled
    /// on a 4×4 grid.
    fn rounded_coverage(px: i32, py: i32, x: f32, y: f32, w: f32, h: f32, r: f32) -> f32 {
        let mut inside = 0;
        for sy in 0..4 {
            for sx in 0..4 {
                let fx = px as f32 + (sx as f32 + 0.5) / 4.0;
                let fy = py as f32 + (sy as f32 + 0.5) / 4.0;
                if fx < x || fy < y || fx > x + w || fy > y + h {
                    continue;
                }
                let cx = fx.clamp(x + r, x + w - r);
                let cy = fy.clamp(y + r, y + h - r);
                if (fx - cx).powi(2) + (fy - cy).powi(2) <= r * r {
                    inside += 1;
                }
            }
        }
        inside as f32 / 16.0
    }

    /// A rounded rectangle, optionally filled and/or with a 1px outline.
    pub fn rounded_rect(
        &mut self,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        radius: f32,
        fill: Option<(Rgb, f32)>,
        stroke: Option<Rgb>,
    ) {
        let (fx, fy, fw, fh) = (x as f32, y as f32, w as f32, h as f32);
        for py in y - 1..=y + h {
            for px in x - 1..=x + w {
                let outer = Self::rounded_coverage(px, py, fx, fy, fw, fh, radius);
                if outer <= 0.0 {
                    continue;
                }
                if let Some((rgb, alpha)) = fill {
                    self.blend(px, py, Self::color(rgb, alpha * outer));
                }
                if let Some(rgb) = stroke {
                    let inner = Self::rounded_coverage(
                        px,
                        py,
                        fx + 1.0,
                        fy + 1.0,
                        fw - 2.0,
                        fh - 2.0,
                        (radius - 1.0).max(0.0),
                    );
                    let ring = (outer - inner).max(0.0);
                    self.blend(px, py, Self::color(rgb, ring));
                }
            }
        }
    }

    /// Draw a bitmap 1:1 at (x, y), faded by `alpha`.
    pub fn blit(&mut self, src: &Bitmap, x: i32, y: i32, alpha: f32) {
        for sy in 0..src.height as i32 {
            for sx in 0..src.width as i32 {
                let p = src.pixel(sx as u32, sy as u32);
                if p[3] == 0 {
                    continue;
                }
                let px = [
                    f32::from(p[0]) / 255.0 * alpha,
                    f32::from(p[1]) / 255.0 * alpha,
                    f32::from(p[2]) / 255.0 * alpha,
                    f32::from(p[3]) / 255.0 * alpha,
                ];
                self.blend(x + sx, y + sy, px);
            }
        }
    }

    /// Draw a bitmap's shape in a flat colour (for the selected tab icon).
    pub fn blit_tinted(&mut self, src: &Bitmap, x: i32, y: i32, rgb: Rgb) {
        for sy in 0..src.height as i32 {
            for sx in 0..src.width as i32 {
                let a = f32::from(src.pixel(sx as u32, sy as u32)[3]) / 255.0;
                if a > 0.0 {
                    self.blend(x + sx, y + sy, Self::color(rgb, a));
                }
            }
        }
    }

    /// Draw one line of text with its top at `y`. Returns its width.
    pub fn text(
        &mut self,
        fonts: &Fonts,
        bold: bool,
        size: f32,
        text: &str,
        x: f32,
        y: f32,
        rgb: Rgb,
    ) -> f32 {
        let font = fonts.pick(bold, text);
        let color = Self::color(rgb, 1.0);
        let mut glyphs: Vec<(i32, i32, i32, i32, Vec<f32>)> = Vec::new();
        font.draw(
            font_px(size),
            text,
            (x, y),
            None,
            TextAlignment::Left,
            |glyph| {
                let (gx, gy) = glyph.origin();
                let (gw, gh) = glyph.dimensions();
                let mut coverage = Vec::with_capacity((gw * gh).max(0) as usize);
                for yy in 0..gh {
                    for xx in 0..gw {
                        coverage.push(glyph.pixel_at((xx, yy)));
                    }
                }
                glyphs.push((gx.round() as i32, gy.round() as i32, gw, gh, coverage));
            },
        );
        for (gx, gy, gw, gh, coverage) in glyphs {
            for yy in 0..gh {
                for xx in 0..gw {
                    let c = coverage[(yy * gw + xx) as usize];
                    if c > 0.0 {
                        let src = [color[0] * c, color[1] * c, color[2] * c, color[3] * c];
                        self.blend(gx + xx, gy + yy, src);
                    }
                }
            }
        }
        fonts.width(bold, size, text)
    }

    /// Text centred on `center_x`.
    pub fn text_centered(
        &mut self,
        fonts: &Fonts,
        bold: bool,
        size: f32,
        text: &str,
        center_x: f32,
        y: f32,
        rgb: Rgb,
    ) {
        let width = fonts.width(bold, size, text);
        self.text(fonts, bold, size, text, (center_x - width / 2.0).round(), y, rgb);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_and_blend() {
        let mut canvas = Canvas::new(4, 4);
        canvas.fill_rect(0, 0, 4, 4, (255, 0, 0), 1.0);
        assert_eq!(canvas.bitmap.pixel(2, 2), [255, 0, 0, 255]);
        // Half-transparent white over red.
        canvas.fill_rect(1, 1, 1, 1, (255, 255, 255), 0.5);
        assert_eq!(canvas.bitmap.pixel(1, 1), [255, 128, 128, 255]);
        // Clipping keeps pixels outside untouched.
        canvas.set_clip(0, 0, 2, 2);
        canvas.fill_rect(0, 0, 4, 4, (0, 0, 255), 1.0);
        assert_eq!(canvas.bitmap.pixel(1, 1), [0, 0, 255, 255]);
        assert_eq!(canvas.bitmap.pixel(3, 3), [255, 0, 0, 255]);
    }

    #[test]
    fn blit_is_one_to_one() {
        let mut src = Bitmap::new(2, 1);
        src.pixels = vec![10, 20, 30, 255, 0, 0, 0, 0];
        let mut canvas = Canvas::new(4, 2);
        canvas.fill_rect(0, 0, 4, 2, (1, 1, 1), 1.0);
        canvas.blit(&src, 1, 1, 1.0);
        assert_eq!(canvas.bitmap.pixel(1, 1), [10, 20, 30, 255]);
        // Transparent source pixels leave the canvas alone.
        assert_eq!(canvas.bitmap.pixel(2, 1), [1, 1, 1, 255]);
        // Drawing past the edge is clipped, not a panic.
        canvas.blit(&src, 3, 1, 1.0);
        canvas.blit(&src, -1, -1, 1.0);
    }

    #[test]
    fn tint_uses_only_alpha() {
        let mut src = Bitmap::new(1, 1);
        src.pixels = vec![255, 0, 0, 255];
        let mut canvas = Canvas::new(1, 1);
        canvas.blit_tinted(&src, 0, 0, CYAN);
        assert_eq!(canvas.bitmap.pixel(0, 0), [90, 200, 255, 255]);
    }

    #[test]
    fn rounded_rect_corners_are_soft() {
        let mut canvas = Canvas::new(20, 20);
        canvas.rounded_rect(0, 0, 20, 20, 7.0, Some(((255, 255, 255), 1.0)), None);
        assert_eq!(canvas.bitmap.pixel(10, 10)[3], 255);
        assert_eq!(canvas.bitmap.pixel(0, 0)[3], 0);
        // The arc is antialiased: some pixel in the corner is partly
        // covered. (Not one fixed pixel: the diagonal happens to step
        // from fully outside to fully inside for this radius.)
        let soft = (0..7)
            .flat_map(|y| (0..7).map(move |x| (x, y)))
            .map(|(x, y)| canvas.bitmap.pixel(x, y)[3])
            .any(|a| a > 0 && a < 255);
        assert!(soft, "no partly covered pixel in the corner");
    }

    #[test]
    fn layout_matches_the_mockup() {
        assert_eq!(LIST_H, 4 * ROW_H);
        assert_eq!(FIGHTER_X, 262);
        assert_eq!(INDEX_LETTERS.chars().count(), 27);
    }

    #[test]
    fn single_line_strips_control_characters() {
        assert_eq!(single_line("Line\nBreak\tTab"), "Line Break Tab");
    }
}

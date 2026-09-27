/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Cover art: the `library/art/<id>.png` cache and a small bitmap type.
//!
//! The scanners store each song's embedded picture once, shrunk so its
//! longest side is at most [MAX_ART_SIDE]. This is the user's own art, so
//! resizing it is fine (unlike the game's art, which the picker always draws
//! at its native size).

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub const MAX_ART_SIDE: u32 = 256;

/// An RGBA8 image with premultiplied alpha, the same layout
/// [crate::image::Image] uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Bitmap {
    /// A fully transparent bitmap.
    pub fn new(width: u32, height: u32) -> Bitmap {
        Bitmap {
            width,
            height,
            pixels: vec![0; width as usize * height as usize * 4],
        }
    }

    pub fn from_image(image: &crate::image::Image) -> Bitmap {
        let (width, height) = image.dimensions();
        Bitmap {
            width,
            height,
            pixels: image.pixels().to_vec(),
        }
    }

    /// Decode a PNG or JPEG.
    pub fn decode(bytes: &[u8]) -> Option<Bitmap> {
        crate::image::Image::from_bytes(bytes)
            .ok()
            .map(|image| Bitmap::from_image(&image))
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        [
            self.pixels[i],
            self.pixels[i + 1],
            self.pixels[i + 2],
            self.pixels[i + 3],
        ]
    }

    /// Resize to exactly `width`×`height` by averaging the source pixels
    /// each output pixel covers. Meant for shrinking.
    pub fn scaled(&self, width: u32, height: u32) -> Bitmap {
        if width == self.width && height == self.height {
            return self.clone();
        }
        let mut out = Bitmap::new(width, height);
        if self.width == 0 || self.height == 0 {
            return out;
        }
        let sx = self.width as f64 / width as f64;
        let sy = self.height as f64 / height as f64;
        for y in 0..height {
            let y0 = (y as f64 * sy).floor() as u32;
            let y1 = (((y + 1) as f64 * sy).ceil() as u32).clamp(y0 + 1, self.height);
            for x in 0..width {
                let x0 = (x as f64 * sx).floor() as u32;
                let x1 = (((x + 1) as f64 * sx).ceil() as u32).clamp(x0 + 1, self.width);
                let mut sum = [0u32; 4];
                for yy in y0..y1 {
                    for xx in x0..x1 {
                        let p = self.pixel(xx, yy);
                        for c in 0..4 {
                            sum[c] += u32::from(p[c]);
                        }
                    }
                }
                let n = (y1 - y0) * (x1 - x0);
                let i = (y as usize * width as usize + x as usize) * 4;
                for c in 0..4 {
                    out.pixels[i + c] = ((sum[c] + n / 2) / n) as u8;
                }
            }
        }
        out
    }

    /// Shrink (never enlarge) so the longest side is at most `max_side`.
    pub fn scaled_to_fit(&self, max_side: u32) -> Bitmap {
        let longest = self.width.max(self.height);
        if longest <= max_side || longest == 0 {
            return self.clone();
        }
        let w = ((self.width as u64 * max_side as u64 + longest as u64 / 2) / longest as u64)
            .max(1) as u32;
        let h = ((self.height as u64 * max_side as u64 + longest as u64 / 2) / longest as u64)
            .max(1) as u32;
        self.scaled(w, h)
    }
}

fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut table = [0u32; 256];
        for (n, entry) in table.iter_mut().enumerate() {
            let mut c = n as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xedb8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *entry = c;
        }
        table
    });
    let mut crc = 0xffff_ffffu32;
    for &b in data {
        crc = table[((crc ^ u32::from(b)) & 0xff) as usize] ^ (crc >> 8);
    }
    crc ^ 0xffff_ffff
}

fn png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// Encode as an 8-bit RGBA PNG (PNG alpha is straight, so the pixels are
/// un-premultiplied first).
pub fn encode_png(bitmap: &Bitmap) -> Vec<u8> {
    let mut raw = Vec::with_capacity((bitmap.width as usize * 4 + 1) * bitmap.height as usize);
    for y in 0..bitmap.height {
        raw.push(0); // filter type: none
        for x in 0..bitmap.width {
            let [r, g, b, a] = bitmap.pixel(x, y);
            if a == 0 {
                raw.extend_from_slice(&[0, 0, 0, 0]);
            } else {
                let un = |c: u8| ((u32::from(c) * 255 + u32::from(a) / 2) / u32::from(a)).min(255) as u8;
                raw.extend_from_slice(&[un(r), un(g), un(b), a]);
            }
        }
    }
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&raw).unwrap();
    let compressed = encoder.finish().unwrap();

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&bitmap.width.to_be_bytes());
    ihdr.extend_from_slice(&bitmap.height.to_be_bytes());
    // 8 bits per channel, colour type 6 (RGBA), default compression and
    // filtering, no interlacing.
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);

    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    png_chunk(&mut out, b"IHDR", &ihdr);
    png_chunk(&mut out, b"IDAT", &compressed);
    png_chunk(&mut out, b"IEND", &[]);
    out
}

pub fn art_path(id: u64) -> PathBuf {
    super::art_dir().join(format!("{id}.png"))
}

/// The raw PNG for a song, for handing to `UIImage`.
pub fn load_png_bytes(id: u64) -> Option<Vec<u8>> {
    std::fs::read(art_path(id)).ok()
}

/// Recently decoded art. The picker asks for the same few dozen songs over
/// and over while scrolling, so decoding once each is enough.
static CACHE: Mutex<Vec<(u64, Option<Arc<Bitmap>>)>> = Mutex::new(Vec::new());
const CACHE_SIZE: usize = 48;

/// A song's decoded cover art, or `None` if it has none.
pub fn load(id: u64) -> Option<Arc<Bitmap>> {
    {
        let mut cache = CACHE.lock().unwrap();
        if let Some(pos) = cache.iter().position(|&(cached, _)| cached == id) {
            let entry = cache.remove(pos);
            let bitmap = entry.1.clone();
            cache.push(entry);
            return bitmap;
        }
    }
    let bitmap = load_png_bytes(id)
        .and_then(|bytes| Bitmap::decode(&bytes))
        .map(Arc::new);
    let mut cache = CACHE.lock().unwrap();
    if cache.len() >= CACHE_SIZE {
        cache.remove(0);
    }
    cache.push((id, bitmap.clone()));
    bitmap
}

/// Forget cached art, after a rescan may have replaced the files.
pub fn clear_cache() {
    CACHE.lock().unwrap().clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, rgba: [u8; 4]) -> Bitmap {
        Bitmap {
            width,
            height,
            pixels: rgba.repeat((width * height) as usize),
        }
    }

    #[test]
    fn crc32_matches_reference() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32(b"IEND"), 0xae42_6082);
    }

    #[test]
    fn png_round_trips_through_the_engine_decoder() {
        let mut bitmap = Bitmap::new(3, 2);
        let colours: [[u8; 4]; 6] = [
            [255, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 255, 255],
            [0, 0, 0, 0],
            [10, 20, 30, 255],
            [255, 255, 255, 255],
        ];
        for (i, c) in colours.iter().enumerate() {
            bitmap.pixels[i * 4..i * 4 + 4].copy_from_slice(c);
        }
        let png = encode_png(&bitmap);
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let decoded = Bitmap::decode(&png).expect("stb_image should read our PNG");
        assert_eq!(decoded, bitmap);
    }

    #[test]
    fn shrinks_to_fit() {
        let big = solid(300, 150, [200, 100, 50, 255]);
        let small = big.scaled_to_fit(MAX_ART_SIDE);
        assert_eq!((small.width, small.height), (256, 128));
        assert_eq!(small.pixel(17, 99), [200, 100, 50, 255]);
        // Already small enough: untouched, never enlarged.
        let tiny = solid(40, 60, [1, 2, 3, 255]);
        assert_eq!(tiny.scaled_to_fit(MAX_ART_SIDE), tiny);
    }

    #[test]
    fn scaling_averages() {
        let mut checker = Bitmap::new(2, 2);
        checker.pixels = vec![
            255, 255, 255, 255, 0, 0, 0, 255, //
            0, 0, 0, 255, 255, 255, 255, 255,
        ];
        assert_eq!(checker.scaled(1, 1).pixel(0, 0), [128, 128, 128, 255]);
        let square = solid(100, 100, [9, 9, 9, 255]).scaled(50, 50);
        assert_eq!((square.width, square.height), (50, 50));
        assert_eq!(square.pixel(49, 49), [9, 9, 9, 255]);
    }
}

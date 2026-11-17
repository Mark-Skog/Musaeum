//! Sauvola's adaptive threshold: a pixel is ink when it is darker than the local mean by a margin
//! that grows with the local contrast, so paper grain and uneven lighting stay paper and faint
//! strokes in a dark corner are still ink.
//!
//! The threshold is worked out on a grid of blocks, not per pixel: it changes over the scale of
//! the window, which is many blocks wide, and the grid makes the pass over a page of 1.6 million
//! pixels a few milliseconds.

use crate::gray::{Gray, Integral};

/// Ink or paper, one byte per pixel: 1 is ink.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub bits: Vec<u8>,
}

impl Bitmap {
    /// Erases the long thin strokes across the page: ink whose run down its column is `thin` rows
    /// or fewer, in a row of such ink `long` pixels or more. An underline, a strike-through, a
    /// pencil line drawn under a word: no letter has a stroke across that long, and the foot of a
    /// line found along one would follow it off the letters. Where it crosses a letter's stem it
    /// is not thin, and the stem stays.
    pub fn erase_underlines(&mut self, thin: usize, long: usize) {
        let (w, h) = (self.width as usize, self.height as usize);
        if w == 0 || h == 0 || long == 0 {
            return;
        }
        let mut flat = vec![0u8; w * h];
        let mut any = vec![false; h];
        let mut run = vec![0u16; w];
        for y in 0..=h {
            for x in 0..w {
                if y < h && self.bits[y * w + x] != 0 {
                    run[x] = run[x].saturating_add(1);
                    continue;
                }
                let length = usize::from(run[x]);
                if length > 0 && length <= thin {
                    for row in y - length..y {
                        flat[row * w + x] = 1;
                        any[row] = true;
                    }
                }
                run[x] = 0;
            }
        }
        for y in (0..h).filter(|&y| any[y]) {
            let row = &flat[y * w..(y + 1) * w];
            let mut x = 0;
            while x < w {
                if row[x] == 0 {
                    x += 1;
                    continue;
                }
                let start = x;
                while x < w && row[x] != 0 {
                    x += 1;
                }
                if x - start >= long {
                    self.bits[y * w + start..y * w + x].fill(0);
                }
            }
        }
    }

    pub fn blank(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            bits: vec![0; width as usize * height as usize],
        }
    }

    #[inline]
    pub fn is_ink(&self, x: usize, y: usize) -> bool {
        self.bits[y * self.width as usize + x] != 0
    }

    /// How many pixels are ink.
    pub fn ink_count(&self) -> usize {
        self.bits.iter().filter(|&&bit| bit != 0).count()
    }
}

/// Sauvola's parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SauvolaOptions {
    /// The window's side, in pixels; made odd.
    pub window: u32,
    /// How much the threshold drops where the contrast is low: 0.2 to 0.5 in the literature.
    pub k: f32,
    /// The dynamic range of the standard deviation, 128 for 8-bit pictures.
    pub r: f32,
    /// The side of the blocks the threshold is computed on.
    pub block: u32,
}

impl Default for SauvolaOptions {
    fn default() -> Self {
        Self {
            window: 31,
            k: 0.3,
            r: 128.0,
            block: 4,
        }
    }
}

/// Binarizes `gray`.
pub fn binarize(gray: &Gray, options: &SauvolaOptions) -> Bitmap {
    let width = gray.width as usize;
    let height = gray.height as usize;
    let mut out = Bitmap::blank(gray.width, gray.height);
    if width == 0 || height == 0 {
        return out;
    }
    let integral = Integral::of(gray);
    let half = (options.window.max(3) / 2) as usize;
    let block = options.block.max(1) as usize;
    let blocks_x = width.div_ceil(block);
    let blocks_y = height.div_ceil(block);
    let mut thresholds = vec![0u8; blocks_x * blocks_y];
    for by in 0..blocks_y {
        let cy = (by * block + block / 2).min(height - 1);
        let y0 = cy.saturating_sub(half);
        let y1 = (cy + half + 1).min(height);
        for bx in 0..blocks_x {
            let cx = (bx * block + block / 2).min(width - 1);
            let x0 = cx.saturating_sub(half);
            let x1 = (cx + half + 1).min(width);
            let (mean, deviation) = integral.stats(x0, y0, x1, y1);
            let threshold = mean * (1.0 + options.k * (deviation / options.r - 1.0));
            thresholds[by * blocks_x + bx] = threshold.clamp(0.0, 255.0) as u8;
        }
    }
    for y in 0..height {
        let row = &gray.pixels[y * width..(y + 1) * width];
        let bits = &mut out.bits[y * width..(y + 1) * width];
        let block_row = &thresholds[(y / block) * blocks_x..];
        for (x, (&value, bit)) in row.iter().zip(bits.iter_mut()).enumerate() {
            *bit = u8::from(value < block_row[x / block]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grain_stays_paper_and_strokes_become_ink_even_in_shade() {
        let mut gray = Gray::filled(200, 100, 235);
        let mut seed = 7u32;
        for (index, pixel) in gray.pixels.iter_mut().enumerate() {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            *pixel = 228 + (seed >> 29) as u8;
            // The right half is in shade: everything 60 darker.
            if index % 200 >= 100 {
                *pixel -= 60;
            }
        }
        // A stroke on each side, dark relative to its surroundings.
        for y in 40..60 {
            for x in 20..24 {
                gray.pixels[y * 200 + x] = 40;
                gray.pixels[y * 200 + x + 120] = 40;
            }
        }
        let bits = binarize(&gray, &SauvolaOptions::default());
        assert_eq!(bits.ink_count(), 2 * 20 * 4, "only the strokes");
        assert!(bits.is_ink(21, 50) && bits.is_ink(141, 50));
        assert!(!bits.is_ink(150, 10));
    }

    #[test]
    fn an_underline_goes_and_the_letters_over_it_stay() {
        let (w, h) = (200usize, 60usize);
        let mut mask = Bitmap::blank(w as u32, h as u32);
        let mut ink = |x: usize, y: usize| mask.bits[y * w + x] = 1;
        // Stems of letters, 3 wide and 20 high, and the bar of a T on one of them.
        for stem in 0..8 {
            for y in 20..40 {
                for x in 0..3 {
                    ink(20 + stem * 20 + x, y);
                }
            }
        }
        for x in 16..28 {
            ink(x, 20);
            ink(x, 21);
        }
        // An underline two rows thick touching the stems' feet, from x 15 to 180.
        for x in 15..180 {
            ink(x, 40);
            ink(x, 41);
        }
        mask.erase_underlines(3, 15);
        // Between the stems the underline is gone; under them, part of a stem, it stays.
        assert!(!mask.is_ink(30, 40) && !mask.is_ink(170, 41));
        assert!(mask.is_ink(21, 41) && mask.is_ink(41, 40));
        assert!((20..40).all(|y| mask.is_ink(61, y)));
        // The bar of the T is a letter's: shorter than an underline.
        assert!(mask.is_ink(17, 20) && mask.is_ink(26, 21));
    }
}

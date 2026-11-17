//! A perceptual hash of a region of a page: 64 bits that say what the picture looks like, close
//! for the same picture scanned or compressed again, far for another one. A difference hash: the
//! region shrunk to 9 × 8 gray pixels, one bit per pair of neighbours saying which is brighter.

use crate::gray::Gray;

/// The hash of the region `x0..x1` × `y0..y1` of `gray`, in pixels. A region smaller than a few
/// pixels hashes the same as its neighbourhood, so callers should not ask about specks.
pub fn dhash(gray: &Gray, x0: i64, y0: i64, x1: i64, y1: i64) -> u64 {
    let width = (x1 - x0).max(1) as u32;
    let height = (y1 - y0).max(1) as u32;
    let small = gray.crop(x0, y0, width, height).resample_area(9, 8);
    let mut bits = 0u64;
    for row in 0..8usize {
        for column in 0..8usize {
            let left = small.pixels[row * 9 + column];
            let right = small.pixels[row * 9 + column + 1];
            bits = (bits << 1) | u64::from(left > right);
        }
    }
    bits
}

/// How many bits differ between two hashes: under about 10 is the same picture.
pub fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(seed: u32, grain: u8) -> Gray {
        let mut gray = Gray::filled(300, 200, 230);
        let mut state = seed;
        for (index, pixel) in gray.pixels.iter_mut().enumerate() {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            *pixel = 225 + (state >> 28) as u8 % grain.max(1);
            let (x, y) = (index % 300, index / 300);
            // Some "text": dark bands.
            if (y / 12) % 2 == 0 && (x / 7) % 3 != 2 && y > 20 && x > 30 && x < 270 {
                *pixel = 40 + (state >> 26) as u8;
            }
        }
        gray
    }

    #[test]
    fn the_same_picture_hashes_near_and_another_far() {
        let a = page(1, 8);
        let b = page(99, 12);
        let ha = dhash(&a, 20, 10, 280, 190);
        let hb = dhash(&b, 20, 10, 280, 190);
        assert!(hamming(ha, hb) <= 6, "{}", hamming(ha, hb));
        let mut other = a.clone();
        for (index, pixel) in other.pixels.iter_mut().enumerate() {
            let (x, y) = (index % 300, index / 300);
            *pixel = if (x / 9 + y / 15) % 2 == 0 { 30 } else { 230 };
        }
        let hc = dhash(&other, 20, 10, 280, 190);
        assert!(hamming(ha, hc) >= 16, "{}", hamming(ha, hc));
    }

    /// The compositor hashes the region under a mark with its own copy of this (`dhashRgba` in
    /// `compositor/lines.ts`), whose test pins the same picture to the same bits.
    #[test]
    fn the_compositor_hashes_the_same_picture_to_the_same_bits() {
        let (width, height) = (37u32, 23u32);
        let mut rgba = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let v = (x * 7 + y * 13 + (x * y) % 17 * 5) % 256;
                rgba.extend_from_slice(&[v as u8, ((v * 3) % 256) as u8, 255 - v as u8, 255]);
            }
        }
        let gray = Gray::from_rgba(width, height, &rgba).unwrap();
        let hash = dhash(&gray, 0, 0, i64::from(width), i64::from(height));
        assert_eq!(format!("{hash:016x}"), "268e1db4d2979746");
    }
}

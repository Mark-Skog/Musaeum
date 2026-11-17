//! A gray picture of a page, and the integral images that make window statistics cost the same
//! whatever the window's size.

/// A gray picture: one byte per pixel, rows packed, 0 black and 255 white.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gray {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Gray {
    /// A picture of one color.
    pub fn filled(width: u32, height: u32, value: u8) -> Self {
        Self {
            width,
            height,
            pixels: vec![value; width as usize * height as usize],
        }
    }

    /// From RGBA rows, as the render workers write them: the luminance of each pixel.
    pub fn from_rgba(width: u32, height: u32, rgba: &[u8]) -> Option<Self> {
        let count = width as usize * height as usize;
        if rgba.len() < count * 4 {
            return None;
        }
        let pixels = rgba[..count * 4]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|px| {
                // Rec. 601 weights, in integers: the sum of the weights is 256.
                ((77 * u32::from(px[0]) + 150 * u32::from(px[1]) + 29 * u32::from(px[2])) >> 8)
                    as u8
            })
            .collect();
        Some(Self {
            width,
            height,
            pixels,
        })
    }

    /// The pixel at (x, y); out of the picture, white.
    #[inline]
    pub fn at(&self, x: i64, y: i64) -> u8 {
        if x < 0 || y < 0 || x >= i64::from(self.width) || y >= i64::from(self.height) {
            255
        } else {
            self.pixels[y as usize * self.width as usize + x as usize]
        }
    }

    /// The picture turned `quarters` quarter turns clockwise. A point of this picture lands at
    /// [`Gray::quarter_point`] of the turned one.
    pub fn quarter_turned(&self, quarters: u8) -> Gray {
        let (w, h) = (self.width as usize, self.height as usize);
        let quarters = quarters % 4;
        let (tw, th) = if quarters % 2 == 1 { (h, w) } else { (w, h) };
        let mut pixels = vec![255u8; w * h];
        for y in 0..th {
            let row = &mut pixels[y * tw..(y + 1) * tw];
            for (x, out) in row.iter_mut().enumerate() {
                let (sx, sy) = match quarters {
                    1 => (y, h - 1 - x),
                    2 => (w - 1 - x, h - 1 - y),
                    3 => (w - 1 - y, x),
                    _ => (x, y),
                };
                *out = self.pixels[sy * w + sx];
            }
        }
        Gray {
            width: tw as u32,
            height: th as u32,
            pixels,
        }
    }

    /// Where a point of a picture of `width` × `height`, in pixels from its top-left corner, lands
    /// once the picture is turned `quarters` quarter turns clockwise, as
    /// [`Gray::quarter_turned`] turns it.
    pub fn quarter_point(point: [f32; 2], quarters: u8, width: u32, height: u32) -> [f32; 2] {
        let ([x, y], w, h) = (point, width as f32, height as f32);
        match quarters % 4 {
            1 => [h - y, x],
            2 => [w - x, h - y],
            3 => [y, w - x],
            _ => [x, y],
        }
    }

    /// The pixels of `width` × `height` from (x, y), white where the picture ends.
    pub fn crop(&self, x: i64, y: i64, width: u32, height: u32) -> Gray {
        let mut out = Gray::filled(width, height, 255);
        for row in 0..height {
            for column in 0..width {
                out.pixels[row as usize * width as usize + column as usize] =
                    self.at(x + i64::from(column), y + i64::from(row));
            }
        }
        out
    }

    /// Shrinks to `width` × `height` by averaging the pixels that fall into each new one, which
    /// is what a perceptual hash needs: no aliasing, whatever the ratio.
    pub fn resample_area(&self, width: u32, height: u32) -> Gray {
        let mut out = Gray::filled(width, height, 255);
        if self.width == 0 || self.height == 0 || width == 0 || height == 0 {
            return out;
        }
        let sx = f64::from(self.width) / f64::from(width);
        let sy = f64::from(self.height) / f64::from(height);
        for row in 0..height {
            let y0 = (f64::from(row) * sy).floor() as usize;
            let y1 =
                ((f64::from(row + 1) * sy).ceil() as usize).clamp(y0 + 1, self.height as usize);
            for column in 0..width {
                let x0 = (f64::from(column) * sx).floor() as usize;
                let x1 = ((f64::from(column + 1) * sx).ceil() as usize)
                    .clamp(x0 + 1, self.width as usize);
                let mut sum = 0u64;
                for y in y0..y1 {
                    let line = &self.pixels[y * self.width as usize..];
                    for &value in &line[x0..x1] {
                        sum += u64::from(value);
                    }
                }
                out.pixels[row as usize * width as usize + column as usize] =
                    (sum / ((y1 - y0) * (x1 - x0)) as u64) as u8;
            }
        }
        out
    }
}

/// Sums of the pixels and of their squares over every rectangle from the origin, so the mean and
/// the variance of any window are four lookups each.
pub struct Integral {
    width: usize,
    sum: Vec<u32>,
    square: Vec<u64>,
}

impl Integral {
    pub fn of(gray: &Gray) -> Self {
        let width = gray.width as usize + 1;
        let height = gray.height as usize + 1;
        let mut sum = vec![0u32; width * height];
        let mut square = vec![0u64; width * height];
        for y in 1..height {
            let mut row_sum = 0u32;
            let mut row_square = 0u64;
            let source = &gray.pixels[(y - 1) * gray.width as usize..y * gray.width as usize];
            for (x, &value) in (1..width).zip(source) {
                row_sum += u32::from(value);
                row_square += u64::from(value) * u64::from(value);
                sum[y * width + x] = sum[(y - 1) * width + x] + row_sum;
                square[y * width + x] = square[(y - 1) * width + x] + row_square;
            }
        }
        Self { width, sum, square }
    }

    /// Mean and standard deviation of the window `x0..x1` × `y0..y1`, both in pixels of the
    /// picture (exclusive ends, clamped by the caller).
    #[inline]
    pub fn stats(&self, x0: usize, y0: usize, x1: usize, y1: usize) -> (f32, f32) {
        let w = self.width;
        let count = ((x1 - x0) * (y1 - y0)) as f64;
        if count <= 0.0 {
            return (255.0, 0.0);
        }
        let s = f64::from(self.sum[y1 * w + x1])
            - f64::from(self.sum[y0 * w + x1])
            - f64::from(self.sum[y1 * w + x0])
            + f64::from(self.sum[y0 * w + x0]);
        let q = self.square[y1 * w + x1] as f64
            - self.square[y0 * w + x1] as f64
            - self.square[y1 * w + x0] as f64
            + self.square[y0 * w + x0] as f64;
        let mean = s / count;
        let variance = (q / count - mean * mean).max(0.0);
        (mean as f32, variance.sqrt() as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn luminance_and_window_statistics() {
        let rgba: Vec<u8> = [
            [255, 255, 255, 255],
            [0, 0, 0, 255],
            [255, 0, 0, 255],
            [0, 255, 0, 255],
        ]
        .concat();
        let gray = Gray::from_rgba(2, 2, &rgba).unwrap();
        assert_eq!(gray.pixels, vec![255, 0, 76, 149]);
        let integral = Integral::of(&gray);
        let (mean, deviation) = integral.stats(0, 0, 2, 2);
        assert!((mean - 120.0).abs() < 0.01, "{mean}");
        assert!((deviation - 94.08).abs() < 0.1, "{deviation}");
        assert_eq!(integral.stats(1, 1, 2, 2), (149.0, 0.0));
        assert_eq!(gray.at(-1, 0), 255);
        assert_eq!(gray.crop(1, 1, 2, 2).pixels, vec![149, 255, 255, 255]);
    }

    #[test]
    fn area_resampling_averages() {
        let mut gray = Gray::filled(4, 2, 0);
        gray.pixels[4..].fill(200);
        let small = gray.resample_area(2, 1);
        assert_eq!(small.pixels, vec![100, 100]);
    }

    #[test]
    fn a_quarter_turn_moves_pixels_and_points_alike() {
        // A 3 × 2 picture with one dark pixel, the top-right one.
        let mut gray = Gray::filled(3, 2, 255);
        gray.pixels[2] = 0;
        for quarters in 0..4u8 {
            let turned = gray.quarter_turned(quarters);
            let [x, y] = Gray::quarter_point([2.5, 0.5], quarters, 3, 2);
            assert_eq!(
                turned.at(x.floor() as i64, y.floor() as i64),
                0,
                "{quarters}"
            );
            assert_eq!(turned.pixels.iter().filter(|&&v| v == 0).count(), 1);
            // Turning back the rest of the way lands on the point again.
            let back = Gray::quarter_point([x, y], (4 - quarters) % 4, turned.width, turned.height);
            assert!((back[0] - 2.5).abs() < 1e-5 && (back[1] - 0.5).abs() < 1e-5);
        }
        // A clockwise quarter: the top-right corner goes to the bottom-right.
        assert_eq!(gray.quarter_turned(1).at(1, 2), 0);
    }
}

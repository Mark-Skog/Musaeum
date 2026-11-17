//! A line of text straightened out of a page's picture for the OCR, and the way back.
//!
//! The line detector gives a line as its baseline, a polyline that may bend toward a book's spine or
//! run at any angle, with how far its ink reaches above and below. Tesseract reads one level line
//! best. So the picture is sampled along the baseline: column `c` of the strip is the point at arc
//! length `c` along the baseline (less a margin before its start), and row `r` is that point moved
//! across the line, upward for the rows above the baseline. A curved line comes out straight, a line
//! at 90° comes out level, and the columns of the strip are distances along the line: a word read
//! between columns `a` and `b` is the stretch of the line between those arc lengths.

/// A gray picture: one byte a pixel, 0 black, rows packed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gray {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Gray {
    /// Bilinear, with white paper outside.
    pub fn sample(&self, x: f32, y: f32) -> f32 {
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let at = |px: f32, py: f32| -> f32 {
            if px < 0.0 || py < 0.0 || px >= self.width as f32 || py >= self.height as f32 {
                255.0
            } else {
                f32::from(self.pixels[py as usize * self.width as usize + px as usize])
            }
        };
        at(x0, y0) * (1.0 - fx) * (1.0 - fy)
            + at(x0 + 1.0, y0) * fx * (1.0 - fy)
            + at(x0, y0 + 1.0) * (1.0 - fx) * fy
            + at(x0 + 1.0, y0 + 1.0) * fx * fy
    }
}

/// A line as the strip is cut along it, in pixels of the picture.
#[derive(Clone, Debug, PartialEq)]
pub struct LinePath {
    /// The baseline, from the line's start to its end in reading order.
    pub baseline: Vec<[f32; 2]>,
    /// How far the ink reaches above the baseline and below it, across the line, as the detector
    /// measured it, and the height of the small letters.
    pub ascent: f32,
    pub descent: f32,
    pub x_height: f32,
}

impl LinePath {
    /// The length of the baseline.
    pub fn length(&self) -> f32 {
        self.baseline
            .windows(2)
            .map(|pair| (pair[1][0] - pair[0][0]).hypot(pair[1][1] - pair[0][1]))
            .sum()
    }

    /// The point at arc length `s` along the baseline and the unit direction there; before the
    /// start and past the end, the first and last segments go on straight.
    pub fn at(&self, s: f32) -> ([f32; 2], [f32; 2]) {
        let points = &self.baseline;
        let direction = |a: [f32; 2], b: [f32; 2]| -> [f32; 2] {
            let length = (b[0] - a[0]).hypot(b[1] - a[1]);
            if length > 1e-6 {
                [(b[0] - a[0]) / length, (b[1] - a[1]) / length]
            } else {
                [1.0, 0.0]
            }
        };
        if points.len() < 2 {
            return (points.first().copied().unwrap_or([0.0, 0.0]), [1.0, 0.0]);
        }
        let mut walked = 0.0f32;
        for (index, pair) in points.windows(2).enumerate() {
            let length = (pair[1][0] - pair[0][0]).hypot(pair[1][1] - pair[0][1]);
            let last = index + 2 == points.len();
            if s <= walked + length || last {
                let d = direction(pair[0], pair[1]);
                let t = s - walked;
                return ([pair[0][0] + d[0] * t, pair[0][1] + d[1] * t], d);
            }
            walked += length;
        }
        unreachable!("the last segment returns")
    }
}

/// A straightened line and what it takes to go back.
#[derive(Clone, Debug, PartialEq)]
pub struct Strip {
    pub picture: Gray,
    /// Columns before arc length 0.
    pub lead: f32,
    /// The length of the line's baseline.
    pub length: f32,
}

impl Strip {
    /// A stretch of strip columns, as fractions of the line from its start to its end.
    pub fn fractions(&self, left: f32, right: f32) -> (f32, f32) {
        let length = self.length.max(1e-3);
        ((left - self.lead) / length, (right - self.lead) / length)
    }
}

/// How far the strip reaches, in heights of the small letters: above the baseline, where capitals,
/// ascenders and the accents over them end, and below it, where descenders end; the detector's own
/// ascent and descent leave thin strokes and accents out. And how much it takes before and after
/// the line, as fractions of the ink's height.
const ABOVE: f32 = 1.8;
const BELOW: f32 = 0.75;
const ENDS: f32 = 0.6;
/// The same for a strip cut tight to the line's own letters, for lines set so close that the
/// margins above reach into the line before and those below into the line after: a list of names
/// under a signature, a line of capitals over one of italics. Capitals and ascenders reach a
/// little above the small letters' height; the detector's descent says how far its line goes
/// below.
const TIGHT_ABOVE: f32 = 1.15;
const TIGHT_BELOW: f32 = 0.3;

/// The strip of `line` out of `picture`. Rows above the baseline are sampled upward across the
/// line: for a line read left to right, toward the top of the picture.
pub fn straighten(picture: &Gray, line: &LinePath) -> Strip {
    straighten_as(picture, line, ABOVE, BELOW)
}

/// The strip of `line` cut tight to its letters: no more above the baseline than its capitals
/// reach, nor below it than its own descent, so the lines before and after it stay out.
pub fn straighten_tight(picture: &Gray, line: &LinePath) -> Strip {
    straighten_as(picture, line, TIGHT_ABOVE, TIGHT_BELOW)
}

/// The strip of `line` cut as if its letters stood upside down on it, to be read turned half a
/// turn: what reaches furthest from the baseline, capitals and accents, is then below it, so the
/// larger margin goes there. Its columns are those of [`straighten`].
pub fn straighten_upside_down(picture: &Gray, line: &LinePath) -> Strip {
    straighten_as(picture, line, BELOW, ABOVE)
}

/// The strip of `line`, `up` and `down` small letters' heights above and below its baseline at
/// least.
fn straighten_as(picture: &Gray, line: &LinePath, up: f32, down: f32) -> Strip {
    let x_height = line.x_height.max(1.0);
    let above = line.ascent.max(x_height * up);
    let below = line.descent.max(x_height * down);
    let height_of_ink = above + below;
    let lead = (height_of_ink * ENDS).ceil();
    let length = line.length();
    let width = (length + 2.0 * lead).ceil().max(1.0) as u32;
    let rows_above = above.ceil() as u32;
    let height = (rows_above + below.ceil() as u32).max(1);
    let mut pixels = vec![255u8; width as usize * height as usize];
    for column in 0..width {
        let s = column as f32 + 0.5 - lead;
        let (point, d) = line.at(s);
        // Upward across a line going right, with y growing downward: (d.y, -d.x).
        let up = [d[1], -d[0]];
        for row in 0..height {
            let rise = rows_above as f32 - (row as f32 + 0.5);
            let x = point[0] + up[0] * rise;
            let y = point[1] + up[1] * rise;
            pixels[(row * width + column) as usize] =
                picture.sample(x - 0.5, y - 0.5).round().clamp(0.0, 255.0) as u8;
        }
    }
    Strip {
        picture: Gray {
            width,
            height,
            pixels,
        },
        lead,
        length,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A white picture with a black bar along a given direction from (40, 60).
    fn bar(angle_degrees: f32) -> (Gray, LinePath) {
        let (w, h) = (200u32, 160u32);
        let mut pixels = vec![255u8; (w * h) as usize];
        let (sin, cos) = angle_degrees.to_radians().sin_cos();
        // As seen, counterclockwise: y goes up as the line goes on.
        let d = [cos, -sin];
        let up = [d[1], -d[0]];
        for step in 0..800 {
            let s = step as f32 * 0.125;
            for rise in 0..12 {
                let x = 40.0 + d[0] * s + up[0] * rise as f32 * 0.5;
                let y = 100.0 + d[1] * s + up[1] * rise as f32 * 0.5;
                if x >= 0.0 && y >= 0.0 && (x as u32) < w && (y as u32) < h {
                    pixels[(y as u32 * w + x as u32) as usize] = 0;
                }
            }
        }
        let line = LinePath {
            baseline: vec![[40.0, 100.0], [40.0 + d[0] * 100.0, 100.0 + d[1] * 100.0]],
            ascent: 6.0,
            descent: 2.0,
            x_height: 5.0,
        };
        (
            Gray {
                width: w,
                height: h,
                pixels,
            },
            line,
        )
    }

    #[test]
    fn a_turned_line_comes_out_level() {
        for angle in [0.0, 45.0, 90.0, -30.0] {
            let (picture, line) = bar(angle);
            let strip = straighten(&picture, &line);
            assert!((strip.length - 100.0).abs() < 0.01);
            let w = strip.picture.width as usize;
            let rows_above = (5.0f32 * ABOVE).max(6.0).ceil() as usize;
            // Just above the baseline, the bar is dark along the whole line; well above it, paper.
            let dark_row = rows_above - 3;
            let paper_row = 0;
            let from = strip.lead as usize + 5;
            let to = strip.lead as usize + 95;
            let dark = (from..to)
                .filter(|&x| strip.picture.pixels[dark_row * w + x] < 128)
                .count();
            let paper = (from..to)
                .filter(|&x| strip.picture.pixels[paper_row * w + x] > 200)
                .count();
            assert!(dark >= 85, "{angle}°: {dark} dark");
            assert!(paper >= 85, "{angle}°: {paper} paper");
        }
    }

    #[test]
    fn columns_are_fractions_of_the_line() {
        let (picture, line) = bar(0.0);
        let strip = straighten(&picture, &line);
        let (a, b) = strip.fractions(strip.lead + 25.0, strip.lead + 75.0);
        assert!((a - 0.25).abs() < 1e-4 && (b - 0.75).abs() < 1e-4);
    }

    #[test]
    fn past_its_ends_a_line_goes_on_straight() {
        let line = LinePath {
            baseline: vec![[0.0, 0.0], [10.0, 0.0], [20.0, 10.0]],
            ascent: 1.0,
            descent: 1.0,
            x_height: 1.0,
        };
        let (before, _) = line.at(-5.0);
        assert!((before[0] + 5.0).abs() < 1e-4 && before[1].abs() < 1e-4);
        let (after, d) = line.at(line.length() + 2.0_f32.sqrt());
        assert!((after[0] - 21.0).abs() < 1e-3 && (after[1] - 11.0).abs() < 1e-3);
        assert!((d[0] - d[1]).abs() < 1e-4);
    }

    #[test]
    fn a_tight_strip_keeps_to_the_line_s_own_letters() {
        let (picture, mut line) = bar(0.0);
        line.x_height = 10.0;
        line.ascent = 12.0;
        line.descent = 2.0;
        let usual = straighten(&picture, &line);
        let tight = straighten_tight(&picture, &line);
        // Above: 1.8 small letters, or its own ascent if more; tight, 1.15 of them, or the ascent.
        // Below: 0.75 small letters; tight, 0.3 of them, or its own descent if more.
        assert_eq!(usual.picture.height, 18 + 8);
        assert_eq!(tight.picture.height, 12 + 3);
        // Its columns are still fractions of the same line.
        let (from, to) = tight.fractions(tight.lead, tight.lead + tight.length);
        assert!(from.abs() < 1e-6 && (to - 1.0).abs() < 1e-6);
    }
}

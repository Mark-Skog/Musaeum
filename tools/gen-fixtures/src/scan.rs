//! Scanned twins: pages drawn from a PDF, made to look scanned, and written back as a PDF of
//! pictures only, with no text layer. `visual-check scan` draws the pages with PDFium; this turns the
//! pixels into what a scanner would have given and writes the file.
//!
//! What a scan does to a page, each optional: turns it by a small angle; bends its lines toward the
//! spine, the way a book pressed on the glass curls near its binding, darker there; tints and
//! grains the paper; and softens the ink. The same seed gives the same twin.

use std::io;

use crate::{CATALOG_ID, PAGES_ID, PdfWriter, XorShift, deflate};

/// What the scanner does to a page.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Scanning {
    /// Degrees, counterclockwise, about the page's centre.
    pub skew_degrees: f32,
    /// How far lines sink toward the spine at the page's top and bottom edges, as a fraction of
    /// the page's height; the spine is on the right of even pages and the left of odd ones.
    pub curl: f32,
    /// Paper grain and specks, 0 for none.
    pub noise: f32,
    pub seed: u64,
}

/// A gray page: one byte a pixel, 0 black.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrayPage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl GrayPage {
    /// Gray from RGBA rows, with Rec. 601 weights.
    pub fn from_rgba(width: u32, height: u32, rgba: &[u8]) -> Self {
        let pixels = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|px| {
                ((77 * u32::from(px[0]) + 150 * u32::from(px[1]) + 29 * u32::from(px[2])) >> 8)
                    as u8
            })
            .collect();
        Self {
            width,
            height,
            pixels,
        }
    }

    fn sample(&self, x: f32, y: f32) -> f32 {
        // Bilinear, with paper outside the page.
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

/// Where a point of the flat page lands on the scan, for the curl and the skew of `scanning`, the
/// spine on the right when `spine_right`. Points in pixels.
pub fn warp(
    scanning: &Scanning,
    width: f32,
    height: f32,
    spine_right: bool,
    x: f32,
    y: f32,
) -> [f32; 2] {
    let (mut x, mut y) = (x, y);
    if scanning.curl > 0.0 {
        let from = if spine_right {
            width * 0.62
        } else {
            width * 0.38
        };
        let t = if spine_right {
            (x - from) / (width - from)
        } else {
            (from - x) / from
        }
        .clamp(0.0, 1.0);
        if t > 0.0 {
            let middle = (y - height / 2.0) / (height / 2.0);
            y += scanning.curl * height * t * t * (0.3 + 0.7 * middle);
            let squeeze = 0.35 * (width - from).min(from) * t * t * t / 3.0;
            x += if spine_right { -squeeze } else { squeeze };
        }
    }
    if scanning.skew_degrees != 0.0 {
        // Counterclockwise as seen, with y down.
        let (sin, cos) = (-scanning.skew_degrees.to_radians()).sin_cos();
        let (cx, cy) = (width / 2.0, height / 2.0);
        let (dx, dy) = (x - cx, y - cy);
        x = cx + dx * cos - dy * sin;
        y = cy + dx * sin + dy * cos;
    }
    [x, y]
}

/// The page as `scanning` scans it. `page` is its index in the book, which side the spine is on.
pub fn scan(flat: &GrayPage, scanning: &Scanning, page: u32) -> GrayPage {
    let (w, h) = (flat.width as f32, flat.height as f32);
    let spine_right = page.is_multiple_of(2);
    // The warp goes from the flat page to the scan; drawing the scan needs the other way, which is
    // found by a few steps of fixed-point iteration from the point itself.
    let inverse = |sx: f32, sy: f32| -> [f32; 2] {
        let (mut x, mut y) = (sx, sy);
        for _ in 0..6 {
            let [wx, wy] = warp(scanning, w, h, spine_right, x, y);
            x += sx - wx;
            y += sy - wy;
        }
        [x, y]
    };
    let mut rng = XorShift::new(scanning.seed ^ u64::from(page) << 20);
    let mut pixels = vec![255u8; flat.pixels.len()];
    let warped = scanning.curl > 0.0 || scanning.skew_degrees != 0.0;
    for sy in 0..flat.height {
        for sx in 0..flat.width {
            let value = if warped {
                let [x, y] = inverse(sx as f32 + 0.5, sy as f32 + 0.5);
                flat.sample(x - 0.5, y - 0.5)
            } else {
                f32::from(flat.pixels[(sy * flat.width + sx) as usize])
            };
            // Paper: a little darker than white, more so toward the spine of a curled page.
            let shade = if scanning.curl > 0.0 {
                let t = if spine_right {
                    sx as f32 / w
                } else {
                    1.0 - sx as f32 / w
                };
                1.0 - 0.25 * ((t - 0.75) / 0.25).clamp(0.0, 1.0).powf(1.5)
            } else {
                1.0
            };
            let paper = 236.0 * shade;
            let ink = value / 255.0;
            let mut out = 30.0 + (paper - 30.0) * ink;
            if scanning.noise > 0.0 {
                out += (rng.below(17) as f32 - 8.0) * scanning.noise;
            }
            pixels[(sy * flat.width + sx) as usize] = out.clamp(0.0, 255.0) as u8;
        }
    }
    if scanning.noise > 0.0 {
        let specks = (flat.width as f32 * flat.height as f32 / 40_000.0 * scanning.noise) as u32;
        for _ in 0..specks {
            let (x, y) = (rng.below(flat.width), rng.below(flat.height));
            pixels[(y * flat.width + x) as usize] = 40 + rng.below(60) as u8;
        }
    }
    GrayPage {
        width: flat.width,
        height: flat.height,
        pixels,
    }
}

/// A PDF of pictures only: each page `size_points` big, drawn entirely by its picture.
pub fn image_pdf(pages: &[GrayPage], size_points: [f32; 2]) -> io::Result<Vec<u8>> {
    let mut pdf = PdfWriter {
        out: Vec::new(),
        position: 0,
        offsets: vec![0],
    };
    pdf.bytes(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n")?;
    let first = 3usize;
    pdf.object(
        CATALOG_ID,
        &format!("<< /Type /Catalog /Pages {PAGES_ID} 0 R >>"),
    )?;
    let kids: String = (0..pages.len())
        .map(|index| format!("{} 0 R ", first + index * 3))
        .collect();
    pdf.object(
        PAGES_ID,
        &format!("<< /Type /Pages /Count {} /Kids [ {kids}] >>", pages.len()),
    )?;
    let [width, height] = size_points;
    for (index, page) in pages.iter().enumerate() {
        let id = first + index * 3;
        pdf.object(
            id,
            &format!(
                "<< /Type /Page /Parent {PAGES_ID} 0 R /MediaBox [0 0 {width} {height}] \
                 /Resources << /XObject << /Im0 {} 0 R >> >> /Contents {} 0 R >>",
                id + 1,
                id + 2
            ),
        )?;
        pdf.stream(
            id + 1,
            &format!(
                "/Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceGray \
                 /BitsPerComponent 8 /Filter /FlateDecode",
                page.width, page.height
            ),
            &deflate(&page.pixels)?,
        )?;
        let content = format!("q {width} 0 0 {height} 0 0 cm /Im0 Do Q");
        pdf.stream(
            id + 2,
            "/Filter /FlateDecode",
            &deflate(content.as_bytes())?,
        )?;
    }
    pdf.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flat_scan_keeps_the_ink_where_it_was() {
        let mut flat = GrayPage {
            width: 40,
            height: 30,
            pixels: vec![255; 1200],
        };
        flat.pixels[10 * 40 + 20] = 0;
        let scanned = scan(&flat, &Scanning::default(), 0);
        assert!(scanned.pixels[10 * 40 + 20] < 60);
        assert!(scanned.pixels[0] > 200);
        let turned = scan(
            &flat,
            &Scanning {
                skew_degrees: 2.0,
                ..Scanning::default()
            },
            0,
        );
        assert_eq!(turned.pixels.len(), 1200);
        let pdf = image_pdf(&[scanned], [40.0, 30.0]).unwrap();
        assert!(pdf.starts_with(b"%PDF-1.7"));
    }

    #[test]
    fn the_warp_turns_the_way_it_says() {
        let scanning = Scanning {
            skew_degrees: 90.0,
            ..Scanning::default()
        };
        // A point right of the centre goes up, counterclockwise as seen.
        let [x, y] = warp(&scanning, 100.0, 100.0, true, 60.0, 50.0);
        assert!(
            (x - 50.0).abs() < 1e-3 && (y - 40.0).abs() < 1e-3,
            "{x} {y}"
        );
    }
}

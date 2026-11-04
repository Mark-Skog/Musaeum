//! A page of color patches, for checking what PDFium makes of RGB and CMYK, and the CIEDE2000
//! difference to judge it by.
//!
//! RGB has one right answer: the same numbers. CMYK has none without a profile, and PDFium has no
//! color management; it converts DeviceCMYK with a fixed table. The references for the process
//! primaries are what the U.S. Web Coated (SWOP) v2 profile gives in sRGB, as Little CMS computes it
//! (<https://polotno.com/tools/cmyk-to-rgb>), which is what most of the printing world means by
//! "cyan". Black and the mixtures depend on the rendering intent, so they are measured, not judged.

use std::fmt::Write as _;
use std::fs::File;
use std::io::{self, BufWriter};
use std::path::Path;

use crate::{CATALOG_ID, PAGES_ID, PdfWriter, deflate};

/// What a patch is checked against.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Expected {
    /// These exact sRGB values, give or take `tolerance` per channel.
    Exact([u8; 3], u8),
    /// The sRGB the SWOP profile gives, within this CIEDE2000 difference.
    Profile([u8; 3], f64),
    /// Nothing to judge it by: reported as measured.
    Measured,
}

/// One patch: where it is on the page, from the top left in points, and how it is painted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorPatch {
    /// What it is.
    pub name: &'static str,
    /// Column and row in the grid.
    pub cell: (u32, u32),
    /// The content-stream operators that set its color.
    pub paint: &'static str,
    /// What it should come out as.
    pub expected: Expected,
}

/// Width of the page, in points.
pub const COLOR_PAGE_WIDTH: f32 = 640.0;
/// Height of the page, in points.
pub const COLOR_PAGE_HEIGHT: f32 = 400.0;
/// Edge of a patch, and the gap between them, in points.
const PATCH: f32 = 80.0;
const GAP: f32 = 20.0;

const fn patch(
    name: &'static str,
    cell: (u32, u32),
    paint: &'static str,
    expected: Expected,
) -> ColorPatch {
    ColorPatch {
        name,
        cell,
        paint,
        expected,
    }
}

/// Every patch of the page.
pub const COLOR_PATCHES: [ColorPatch; 17] = [
    patch(
        "RGB red",
        (0, 0),
        "1 0 0 rg",
        Expected::Exact([255, 0, 0], 0),
    ),
    patch(
        "RGB green",
        (1, 0),
        "0 1 0 rg",
        Expected::Exact([0, 255, 0], 0),
    ),
    patch(
        "RGB blue",
        (2, 0),
        "0 0 1 rg",
        Expected::Exact([0, 0, 255], 0),
    ),
    patch(
        "RGB 50% gray",
        (3, 0),
        "0.5 0.5 0.5 rg",
        Expected::Exact([128, 128, 128], 1),
    ),
    patch(
        "Gray 25%",
        (4, 0),
        "0.25 g",
        Expected::Exact([64, 64, 64], 1),
    ),
    patch(
        "RGB orange",
        (5, 0),
        "1 0.5 0 rg",
        Expected::Exact([255, 128, 0], 1),
    ),
    patch(
        "CMYK cyan",
        (0, 1),
        "1 0 0 0 k",
        Expected::Profile([0, 174, 239], 6.0),
    ),
    patch(
        "CMYK magenta",
        (1, 1),
        "0 1 0 0 k",
        Expected::Profile([236, 0, 140], 6.0),
    ),
    patch(
        "CMYK yellow",
        (2, 1),
        "0 0 1 0 k",
        Expected::Profile([255, 242, 0], 6.0),
    ),
    patch(
        "CMYK white",
        (3, 1),
        "0 0 0 0 k",
        Expected::Exact([255, 255, 255], 0),
    ),
    patch("CMYK black", (4, 1), "0 0 0 1 k", Expected::Measured),
    patch(
        "CMYK rich black",
        (5, 1),
        "0.6 0.4 0.4 1 k",
        Expected::Measured,
    ),
    patch("CMYK red", (0, 2), "0 1 1 0 k", Expected::Measured),
    patch("CMYK green", (1, 2), "1 0 1 0 k", Expected::Measured),
    patch("CMYK blue", (2, 2), "1 1 0 0 k", Expected::Measured),
    patch("CMYK 50% cyan", (3, 2), "0.5 0 0 0 k", Expected::Measured),
    patch("CMYK 20% black", (4, 2), "0 0 0 0.2 k", Expected::Measured),
];

/// Where the two overprinting patches overlap, in points from the top left: cyan first, then
/// yellow over it with overprint on. A renderer that simulates overprint shows green there, one
/// that does not shows yellow.
pub const OVERPRINT_OVERLAP: [f32; 4] = [
    5.0 * (PATCH + GAP) + 40.0 + GAP,
    2.0 * (PATCH + GAP) + GAP,
    40.0,
    PATCH,
];

impl ColorPatch {
    /// `[x, y, width, height]` in points from the page's top-left corner.
    pub fn rect(&self) -> [f32; 4] {
        let (column, row) = self.cell;
        [
            GAP + column as f32 * (PATCH + GAP),
            GAP + row as f32 * (PATCH + GAP),
            PATCH,
            PATCH,
        ]
    }
}

/// Writes the patch page: a single 640 × 400 pt page.
pub fn write_color_fixture(path: &Path) -> io::Result<()> {
    let out = BufWriter::new(File::create(path)?);
    let mut pdf = PdfWriter {
        out,
        position: 0,
        offsets: vec![0],
    };
    pdf.bytes(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n")?;
    pdf.object(
        CATALOG_ID,
        &format!("<< /Type /Catalog /Pages {PAGES_ID} 0 R >>"),
    )?;
    let page_id = 3;
    pdf.object(
        PAGES_ID,
        &format!("<< /Type /Pages /Count 1 /Kids [ {page_id} 0 R ] >>"),
    )?;

    let mut content = String::new();
    for patch in &COLOR_PATCHES {
        let [x, y, width, height] = patch.rect();
        let bottom = COLOR_PAGE_HEIGHT - y - height;
        writeln!(
            content,
            "{} {x} {bottom} {width} {height} re f",
            patch.paint
        )
        .expect("writing to a String cannot fail");
    }
    // The overprint pair, in the last cell of the third row and the one after it.
    let [ox, oy, _, oh] = OVERPRINT_OVERLAP;
    let bottom = COLOR_PAGE_HEIGHT - oy - oh;
    writeln!(
        content,
        "1 0 0 0 k {} {bottom} {PATCH} {PATCH} re f",
        ox - 40.0
    )
    .expect("writing to a String cannot fail");
    writeln!(
        content,
        "q /Over gs 0 0 1 0 k {ox} {bottom} {PATCH} {PATCH} re f Q"
    )
    .expect("writing to a String cannot fail");

    pdf.object(
        page_id,
        &format!(
            "<< /Type /Page /Parent {PAGES_ID} 0 R /MediaBox [0 0 {COLOR_PAGE_WIDTH} {COLOR_PAGE_HEIGHT}] \
             /Resources << /ExtGState << /Over << /Type /ExtGState /OP true /op true /OPM 1 >> >> >> \
             /Contents {} 0 R >>",
            page_id + 1
        ),
    )?;
    pdf.stream(
        page_id + 1,
        "/Filter /FlateDecode",
        &deflate(content.as_bytes())?,
    )?;
    pdf.finish()?;
    Ok(())
}

/// CIE L*a*b* of an sRGB color, against the D65 white.
pub fn srgb_to_lab([r, g, b]: [u8; 3]) -> [f64; 3] {
    let linear = |channel: u8| {
        let c = f64::from(channel) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let (r, g, b) = (linear(r), linear(g), linear(b));
    let x = (0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b) / 0.950_47;
    let y = 0.212_672_9 * r + 0.715_152_2 * g + 0.072_175 * b;
    let z = (0.019_333_9 * r + 0.119_192 * g + 0.950_304_1 * b) / 1.088_83;
    let f = |t: f64| {
        if t > 216.0 / 24_389.0 {
            t.cbrt()
        } else {
            (24_389.0 / 27.0 * t + 16.0) / 116.0
        }
    };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// The CIEDE2000 difference between two sRGB colors: about 1 is the smallest a person notices
/// side by side, 2 to 3 is visible on close look, and above 5 they are different colors.
pub fn delta_e_2000(a: [u8; 3], b: [u8; 3]) -> f64 {
    delta_e_2000_lab(srgb_to_lab(a), srgb_to_lab(b))
}

/// The CIEDE2000 difference between two L*a*b* colors.
pub fn delta_e_2000_lab([l1, a1, b1]: [f64; 3], [l2, a2, b2]: [f64; 3]) -> f64 {
    let c1 = a1.hypot(b1);
    let c2 = a2.hypot(b2);
    let mean_c = (c1 + c2) / 2.0;
    let g = 0.5 * (1.0 - (mean_c.powi(7) / (mean_c.powi(7) + 25f64.powi(7))).sqrt());
    let (a1p, a2p) = ((1.0 + g) * a1, (1.0 + g) * a2);
    let (c1p, c2p) = (a1p.hypot(b1), a2p.hypot(b2));
    let hue = |b: f64, a: f64| {
        if a == 0.0 && b == 0.0 {
            0.0
        } else {
            b.atan2(a).to_degrees().rem_euclid(360.0)
        }
    };
    let (h1p, h2p) = (hue(b1, a1p), hue(b2, a2p));
    let delta_l = l2 - l1;
    let delta_c = c2p - c1p;
    let delta_h = if c1p * c2p == 0.0 {
        0.0
    } else if (h2p - h1p).abs() <= 180.0 {
        h2p - h1p
    } else if h2p - h1p > 180.0 {
        h2p - h1p - 360.0
    } else {
        h2p - h1p + 360.0
    };
    let delta_big_h = 2.0 * (c1p * c2p).sqrt() * (delta_h.to_radians() / 2.0).sin();
    let mean_l = (l1 + l2) / 2.0;
    let mean_cp = (c1p + c2p) / 2.0;
    let mean_hp = if c1p * c2p == 0.0 {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180.0 {
        (h1p + h2p) / 2.0
    } else if h1p + h2p < 360.0 {
        (h1p + h2p + 360.0) / 2.0
    } else {
        (h1p + h2p - 360.0) / 2.0
    };
    let t = 1.0 - 0.17 * (mean_hp - 30.0).to_radians().cos()
        + 0.24 * (2.0 * mean_hp).to_radians().cos()
        + 0.32 * (3.0 * mean_hp + 6.0).to_radians().cos()
        - 0.20 * (4.0 * mean_hp - 63.0).to_radians().cos();
    let delta_theta = 30.0 * (-((mean_hp - 275.0) / 25.0).powi(2)).exp();
    let r_c = 2.0 * (mean_cp.powi(7) / (mean_cp.powi(7) + 25f64.powi(7))).sqrt();
    let s_l = 1.0 + 0.015 * (mean_l - 50.0).powi(2) / (20.0 + (mean_l - 50.0).powi(2)).sqrt();
    let s_c = 1.0 + 0.045 * mean_cp;
    let s_h = 1.0 + 0.015 * mean_cp * t;
    let r_t = -(2.0 * delta_theta).to_radians().sin() * r_c;
    ((delta_l / s_l).powi(2)
        + (delta_c / s_c).powi(2)
        + (delta_big_h / s_h).powi(2)
        + r_t * (delta_c / s_c) * (delta_big_h / s_h))
        .sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_colors_do_not_differ_and_lab_is_right_for_white_and_black() {
        assert_eq!(delta_e_2000([12, 34, 56], [12, 34, 56]), 0.0);
        let [l, a, b] = srgb_to_lab([255, 255, 255]);
        assert!((l - 100.0).abs() < 0.01 && a.abs() < 0.01 && b.abs() < 0.01);
        assert!(srgb_to_lab([0, 0, 0])[0].abs() < 0.01);
    }

    #[test]
    fn differences_match_the_published_test_data() {
        // Pairs 1, 7 and 17 of Sharma, Wu and Dalal's CIEDE2000 test data, to four decimals.
        for (first, second, expected) in [
            ([50.0, 2.6772, -79.7751], [50.0, 0.0, -82.7485], 2.0425),
            ([50.0, 0.0, 0.0], [50.0, -1.0, 2.0], 2.3669),
            ([50.0, 2.5, 0.0], [73.0, 25.0, -18.0], 27.1492),
        ] {
            let found = delta_e_2000_lab(first, second);
            assert!(
                (found - expected).abs() < 1e-4,
                "{first:?} {second:?}: {found}"
            );
        }
        // A step of one in a channel is barely anything; red against orange is a clear difference.
        assert!(delta_e_2000([128, 128, 128], [129, 128, 128]) < 1.0);
        assert!(delta_e_2000([255, 0, 0], [255, 128, 0]) > 20.0);
    }
}

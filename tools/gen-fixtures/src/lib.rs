//! Writes reproducible PDF and EPUB fixtures for the Phase 0 experiments.
//!
//! The files are generated rather than committed: they can be large, and the generator
//! documents exactly what each one is meant to stress. Everything is deterministic, so
//! the same arguments always produce the same bytes.

pub mod books;
pub mod epub;

use std::fmt::Write as _;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;

use flate2::Compression;
use flate2::write::ZlibEncoder;

/// A grayscale image, already Flate-compressed.
pub struct GrayImage {
    width: u32,
    height: u32,
    compressed: Vec<u8>,
}

/// One page to write.
pub struct PageSpec {
    /// `[width, height]` of the MediaBox in points.
    pub media_box: [f32; 2],
    /// `/Rotate` value, in degrees.
    pub rotate: u32,
    /// `[left, bottom, right, top]` of the CropBox, when it differs from the MediaBox.
    pub crop_box: Option<[f32; 4]>,
    /// Index of the image this page may draw as `/Im0`.
    pub image: Option<usize>,
    /// Uncompressed content stream.
    pub content: String,
}

impl PageSpec {
    /// A plain upright page with the given content.
    pub fn new(width: f32, height: f32, content: String) -> Self {
        Self {
            media_box: [width, height],
            rotate: 0,
            crop_box: None,
            image: None,
            content,
        }
    }
}

const CATALOG_ID: usize = 1;
const PAGES_ID: usize = 2;
const FONT_ID: usize = 3;
const FIRST_IMAGE_ID: usize = 4;

struct PdfWriter<W: Write> {
    out: W,
    position: u64,
    offsets: Vec<u64>,
}

impl<W: Write> PdfWriter<W> {
    fn bytes(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.out.write_all(bytes)?;
        self.position += bytes.len() as u64;
        Ok(())
    }

    fn begin_object(&mut self, id: usize) -> io::Result<()> {
        if self.offsets.len() <= id {
            self.offsets.resize(id + 1, 0);
        }
        self.offsets[id] = self.position;
        self.bytes(format!("{id} 0 obj\n").as_bytes())
    }

    fn object(&mut self, id: usize, body: &str) -> io::Result<()> {
        self.begin_object(id)?;
        self.bytes(body.as_bytes())?;
        self.bytes(b"\nendobj\n")
    }

    fn stream(&mut self, id: usize, dictionary: &str, data: &[u8]) -> io::Result<()> {
        self.begin_object(id)?;
        self.bytes(format!("<< {dictionary} /Length {} >>\nstream\n", data.len()).as_bytes())?;
        self.bytes(data)?;
        self.bytes(b"\nendstream\nendobj\n")
    }

    fn finish(mut self) -> io::Result<W> {
        let xref_at = self.position;
        let size = self.offsets.len();
        let mut table = format!("xref\n0 {size}\n0000000000 65535 f \n");
        for &offset in &self.offsets[1..] {
            writeln!(table, "{offset:010} 00000 n ").expect("writing to a String cannot fail");
        }
        write!(
            table,
            "trailer\n<< /Size {size} /Root {CATALOG_ID} 0 R >>\nstartxref\n{xref_at}\n%%EOF\n"
        )
        .expect("writing to a String cannot fail");
        self.bytes(table.as_bytes())?;
        self.out.flush()?;
        Ok(self.out)
    }
}

fn deflate(data: &[u8]) -> io::Result<Vec<u8>> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::new(6));
    encoder.write_all(data)?;
    encoder.finish()
}

/// Writes a PDF with `page_count` pages, asking `page` for each one in order.
pub fn write_pdf(
    path: &Path,
    images: &[GrayImage],
    page_count: usize,
    mut page: impl FnMut(usize) -> PageSpec,
) -> io::Result<()> {
    let out = BufWriter::with_capacity(1 << 20, File::create(path)?);
    let mut pdf = PdfWriter {
        out,
        position: 0,
        offsets: vec![0],
    };
    // The comment with high bytes marks the file as binary for tools that care.
    pdf.bytes(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n")?;

    let first_page_id = FIRST_IMAGE_ID + images.len();
    let page_id = |index: usize| first_page_id + index * 2;

    pdf.object(
        CATALOG_ID,
        &format!("<< /Type /Catalog /Pages {PAGES_ID} 0 R >>"),
    )?;
    let mut kids = String::with_capacity(page_count * 10);
    for index in 0..page_count {
        write!(kids, "{} 0 R ", page_id(index)).expect("writing to a String cannot fail");
    }
    pdf.object(
        PAGES_ID,
        &format!("<< /Type /Pages /Count {page_count} /Kids [ {kids}] >>"),
    )?;
    pdf.object(
        FONT_ID,
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    )?;

    for (index, image) in images.iter().enumerate() {
        pdf.stream(
            FIRST_IMAGE_ID + index,
            &format!(
                "/Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceGray \
                 /BitsPerComponent 8 /Filter /FlateDecode",
                image.width, image.height
            ),
            &image.compressed,
        )?;
    }

    for index in 0..page_count {
        let spec = page(index);
        let id = page_id(index);
        let [width, height] = spec.media_box;
        let mut resources = format!("/Font << /F1 {FONT_ID} 0 R >>");
        if let Some(image) = spec.image {
            write!(
                resources,
                " /XObject << /Im0 {} 0 R >>",
                FIRST_IMAGE_ID + image
            )
            .expect("writing to a String cannot fail");
        }
        let mut dictionary = format!(
            "<< /Type /Page /Parent {PAGES_ID} 0 R /MediaBox [0 0 {width} {height}] \
             /Rotate {} /Resources << {resources} >> /Contents {} 0 R",
            spec.rotate,
            id + 1
        );
        if let Some([left, bottom, right, top]) = spec.crop_box {
            write!(dictionary, " /CropBox [{left} {bottom} {right} {top}]")
                .expect("writing to a String cannot fail");
        }
        dictionary.push_str(" >>");
        pdf.object(id, &dictionary)?;
        pdf.stream(
            id + 1,
            "/Filter /FlateDecode",
            &deflate(spec.content.as_bytes())?,
        )?;
    }
    pdf.finish()?;
    Ok(())
}

/// Small deterministic generator, so fixtures are reproducible byte for byte.
pub(crate) struct XorShift(u64);

impl XorShift {
    pub(crate) fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub(crate) fn below(&mut self, limit: u32) -> u32 {
        (self.next() % u64::from(limit)) as u32
    }

    fn unit(&mut self) -> f32 {
        self.below(1000) as f32 / 1000.0
    }
}

/// Draws something that compresses and decodes like a scanned page: paper grain, plus
/// dark word blocks in slightly skewed lines of text.
pub fn scanned_page_image(width: u32, height: u32, seed: u64) -> io::Result<GrayImage> {
    let mut rng = XorShift::new(seed);
    let (w, h) = (width as usize, height as usize);
    let mut pixels = vec![0u8; w * h];
    for pixel in &mut pixels {
        *pixel = 232 + rng.below(8) as u8;
    }

    let margin = w / 10;
    let line_height = h / 44;
    let skew = rng.unit() * 0.012 - 0.006;
    let mut y = h / 10;
    while y + line_height < h - h / 10 {
        let mut x = margin;
        while x < w - margin {
            let word = line_height / 2 + rng.below(line_height as u32 * 3) as usize;
            let end = (x + word).min(w - margin);
            for column in x..end {
                let top = (y as f32 + column as f32 * skew) as usize;
                let bottom = (top + line_height * 3 / 5).min(h);
                for row in top..bottom {
                    if rng.below(100) < 85 {
                        pixels[row * w + column] = 25 + rng.below(40) as u8;
                    }
                }
            }
            x = end + line_height / 2;
        }
        y += line_height;
    }

    Ok(GrayImage {
        width,
        height,
        compressed: deflate(&pixels)?,
    })
}

/// Pages that exercise geometry: a mix of sizes, `/Rotate`, a CropBox, and solid colours
/// that make byte order and tile offsets checkable.
///
/// | Page | Effective size | Purpose |
/// |------|----------------|---------|
/// | 0 | 612 × 792 | Solid red, for checking the RGBA byte order |
/// | 1 | 842 × 595 | Landscape, solid green |
/// | 2 | 500 × 500 | Square, solid blue |
/// | 3 | 792 × 612 | Portrait MediaBox with `/Rotate 90` |
/// | 4 | 300 × 400 | MediaBox of 612 × 792 cut down by a CropBox |
/// | 5 | 2000 × 3000 | Top-left quadrant red, bottom-right blue, for tile offsets |
pub fn write_mixed_fixture(path: &Path) -> io::Result<()> {
    write_pdf(path, &[], 6, |index| match index {
        0 => PageSpec::new(612.0, 792.0, "1 0 0 rg 0 0 612 792 re f".into()),
        1 => PageSpec::new(842.0, 595.0, "0 1 0 rg 0 0 842 595 re f".into()),
        2 => PageSpec::new(500.0, 500.0, "0 0 1 rg 0 0 500 500 re f".into()),
        3 => PageSpec {
            rotate: 90,
            ..PageSpec::new(612.0, 792.0, "1 1 0 rg 0 0 612 792 re f".into())
        },
        4 => PageSpec {
            crop_box: Some([100.0, 100.0, 400.0, 500.0]),
            ..PageSpec::new(612.0, 792.0, "0 1 1 rg 0 0 612 792 re f".into())
        },
        _ => PageSpec::new(
            2000.0,
            3000.0,
            "1 0 0 rg 0 1500 1000 1500 re f 0 0 1 rg 1000 0 1000 1500 re f".into(),
        ),
    })
}

/// One page per font that PDFium has to find on its own, because none of them is embedded:
/// the standard 14, Windows fonts by name, and a CJK font. Returns the number of pages.
///
/// This is the one thing a sandboxed renderer might get wrong without crashing, since looking
/// for a font is the only moment PDFium reaches out to the system during a render.
pub fn write_font_fixture(path: &Path) -> io::Result<u32> {
    const FONTS: [&str; 7] = [
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Times-Roman >>",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>",
        "<< /Type /Font /Subtype /TrueType /BaseFont /Arial /Encoding /WinAnsiEncoding >>",
        "<< /Type /Font /Subtype /TrueType /BaseFont /TimesNewRoman /Encoding /WinAnsiEncoding >>",
        "<< /Type /Font /Subtype /TrueType /BaseFont /Verdana /Encoding /WinAnsiEncoding >>",
        "<< /Type /Font /Subtype /Type0 /BaseFont /STSong-Light /Encoding /UniGB-UCS2-H \
         /DescendantFonts [ << /Type /Font /Subtype /CIDFontType0 /BaseFont /STSong-Light \
         /CIDSystemInfo << /Registry (Adobe) /Ordering (GB1) /Supplement 2 >> >> ] >>",
    ];
    const FIRST_FONT: usize = 3;
    let first_page = FIRST_FONT + FONTS.len();

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
    let kids: String = (0..FONTS.len())
        .map(|page| format!("{} 0 R ", first_page + page * 2))
        .collect();
    pdf.object(
        PAGES_ID,
        &format!("<< /Type /Pages /Count {} /Kids [ {kids}] >>", FONTS.len()),
    )?;
    for (index, font) in FONTS.iter().enumerate() {
        pdf.object(FIRST_FONT + index, font)?;
    }

    for index in 0..FONTS.len() {
        let id = first_page + index * 2;
        let text = if index == FONTS.len() - 1 {
            // "你好世界", in UCS-2, four times.
            "<4F60597D4E16754C4F60597D4E16754C4F60597D4E16754C4F60597D4E16754C>".to_owned()
        } else {
            "(The quick brown fox jumps over the lazy dog)".to_owned()
        };
        let mut content = String::from("BT /F0 18 Tf 0 g 24 TL 20 480 Td\n");
        for _ in 0..18 {
            writeln!(content, "{text} Tj T*").expect("writing to a String cannot fail");
        }
        content.push_str("ET\n");

        pdf.object(
            id,
            &format!(
                "<< /Type /Page /Parent {PAGES_ID} 0 R /MediaBox [0 0 500 500] \
                 /Resources << /Font << /F0 {} 0 R >> >> /Contents {} 0 R >>",
                FIRST_FONT + index,
                id + 1
            ),
        )?;
        pdf.stream(
            id + 1,
            "/Filter /FlateDecode",
            &deflate(content.as_bytes())?,
        )?;
    }
    pdf.finish()?;
    Ok(FONTS.len() as u32)
}

/// A long scanned-looking book: images shared between pages to keep the file small, a
/// landscape plate every 37 pages, and a unique page label so scrolling is visible.
pub fn write_scan_fixture(path: &Path, page_count: usize) -> io::Result<()> {
    const PORTRAIT: [f32; 2] = [595.0, 842.0];
    const IMAGES: usize = 16;

    let mut images = Vec::with_capacity(IMAGES);
    for index in 0..IMAGES {
        let image = if index == 0 {
            scanned_page_image(1754, 1240, index as u64 + 1)?
        } else {
            scanned_page_image(1240, 1754, index as u64 + 1)?
        };
        images.push(image);
    }

    write_pdf(path, &images, page_count, |index| {
        let landscape = index % 37 == 36;
        let ([width, height], image) = if landscape {
            ([PORTRAIT[1], PORTRAIT[0]], 0)
        } else {
            (PORTRAIT, 1 + index % (IMAGES - 1))
        };
        let content = format!(
            "q {width} 0 0 {height} 0 0 cm /Im0 Do Q BT /F1 12 Tf 0 g 24 18 Td (Page {}) Tj ET",
            index + 1
        );
        PageSpec {
            image: Some(image),
            ..PageSpec::new(width, height, content)
        }
    })
}

/// Vector pages. Most hold a dense but realistic amount of drawing and text; every 50th
/// is a map-like page with ten times the paths, which is what cancellation is for.
pub fn write_vector_fixture(path: &Path, page_count: usize) -> io::Result<()> {
    const WIDTH: f32 = 612.0;
    const HEIGHT: f32 = 792.0;

    write_pdf(path, &[], page_count, |index| {
        let mut rng = XorShift::new(index as u64 + 1000);
        let curves = if index % 50 == 49 { 15_000 } else { 1_500 };
        let mut content = String::with_capacity(curves * 64);
        content.push_str("0.3 w\n");

        for _ in 0..curves {
            let (x, y) = (rng.unit() * WIDTH, rng.unit() * HEIGHT);
            let mut offset = || rng.unit() * 60.0 - 30.0;
            let points = [offset(), offset(), offset(), offset(), offset(), offset()];
            let (red, green, blue) = (rng.unit(), rng.unit(), rng.unit());
            writeln!(
                content,
                "{red:.2} {green:.2} {blue:.2} RG {x:.1} {y:.1} m {:.1} {:.1} {:.1} {:.1} {:.1} {:.1} c S",
                x + points[0],
                y + points[1],
                x + points[2],
                y + points[3],
                x + points[4],
                y + points[5],
            )
            .expect("writing to a String cannot fail");
        }

        content.push_str("BT /F1 9 Tf 0 g 11 TL 40 750 Td\n");
        for line in 0..60 {
            writeln!(
                content,
                "(Page {} line {line}: the quick brown fox jumps over the lazy dog, repeatedly.) Tj T*",
                index + 1
            )
            .expect("writing to a String cannot fail");
        }
        content.push_str("ET\n");
        PageSpec::new(WIDTH, HEIGHT, content)
    })
}

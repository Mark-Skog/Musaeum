//! Comic book archives, CBZ and CBR, with pages of every shape and the junk real archives carry.
//!
//! The archives are written here byte by byte, so that tests know exactly what is in them. RAR is
//! written stored, without compression: the RAR compressor is proprietary, and neither this
//! generator nor anything else in the project reimplements it. The formats follow RARLAB's public
//! technical notes for RAR 4 and RAR 5.
//!
//! Every page is split into four colored quadrants, red, green, blue and yellow from the top left,
//! so a test can tell whether a page came out the right way up.

use std::io::{self, Write};
use std::path::Path;

use flate2::Compression;
use flate2::Crc;
use flate2::write::DeflateEncoder;
use jpeg_encoder::{ColorType, Encoder};

/// One file in an archive.
#[derive(Clone, Debug)]
pub struct ArchiveEntry {
    /// Its path inside the archive. A trailing `/` makes it a directory.
    pub name: String,
    /// Its contents.
    pub data: Vec<u8>,
}

impl ArchiveEntry {
    fn new(name: &str, data: Vec<u8>) -> Self {
        Self {
            name: name.to_owned(),
            data,
        }
    }
}

/// The quadrants' colors, from the top left, clockwise from there: top right, then the bottom row.
pub const QUADRANTS: [[u8; 3]; 4] = [[220, 30, 30], [30, 180, 60], [40, 60, 200], [235, 210, 40]];

/// What a page of the fixture looks like once shown the right way up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ComicPage {
    /// Its file name in the archive.
    pub name: &'static str,
    /// Width once shown, in pixels.
    pub width: u32,
    /// Height once shown, in pixels.
    pub height: u32,
    /// How it is stored.
    pub encoding: PageEncoding,
}

/// How a fixture page is stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageEncoding {
    /// RGB JPEG.
    Jpeg,
    /// RGB JPEG stored on its side, with an EXIF orientation of 6 to turn it upright.
    JpegRotated,
    /// Grayscale JPEG.
    JpegGray,
    /// CMYK JPEG, as some print scans are.
    JpegCmyk,
    /// Opaque RGBA PNG.
    Png,
    /// PNG whose quadrants are inset in a transparent border, which must come out white.
    PngTransparent,
}

/// The pages of every comic fixture, in reading order. The archive stores them in another order,
/// and `p10` must come after `p9`, which only a natural sort gets right.
pub const COMIC_PAGES: [ComicPage; 10] = [
    page("p1.jpg", 600, 900, PageEncoding::Jpeg),
    page("p2.png", 600, 900, PageEncoding::Png),
    page("p3.jpg", 1200, 900, PageEncoding::Jpeg),
    page("p4.jpg", 600, 900, PageEncoding::JpegRotated),
    page("p5.png", 700, 700, PageEncoding::PngTransparent),
    page("p6.jpg", 600, 900, PageEncoding::JpegGray),
    page("p7.jpg", 600, 900, PageEncoding::JpegCmyk),
    page("p8.png", 600, 900, PageEncoding::Png),
    page("p9.jpg", 1200, 900, PageEncoding::Jpeg),
    page("p10.png", 600, 900, PageEncoding::Png),
];

const fn page(name: &'static str, width: u32, height: u32, encoding: PageEncoding) -> ComicPage {
    ComicPage {
        name,
        width,
        height,
        encoding,
    }
}

/// The color a page shows at `(x, y)`, upright.
pub fn quadrant_color(page: &ComicPage, x: u32, y: u32) -> [u8; 3] {
    let right = usize::from(x >= page.width / 2);
    let bottom = usize::from(y >= page.height / 2);
    QUADRANTS[bottom * 2 + right]
}

/// Whether `(x, y)` is in the transparent border of a [`PageEncoding::PngTransparent`] page.
pub fn in_transparent_border(page: &ComicPage, x: u32, y: u32) -> bool {
    let border = page.width.min(page.height) / 8;
    x < border || y < border || x >= page.width - border || y >= page.height - border
}

fn upright_rgb(page: &ComicPage) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((page.width * page.height * 3) as usize);
    for y in 0..page.height {
        for x in 0..page.width {
            pixels.extend_from_slice(&quadrant_color(page, x, y));
        }
    }
    pixels
}

fn jpeg(pixels: &[u8], width: u32, height: u32, color: ColorType, app1: Option<&[u8]>) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = Encoder::new(&mut out, 92);
    if let Some(exif) = app1 {
        encoder
            .add_app_segment(1, exif)
            .expect("an EXIF block of a few bytes fits a segment");
    }
    encoder
        .encode(pixels, width as u16, height as u16, color)
        .expect("in-memory JPEG encoding cannot fail for these sizes");
    out
}

/// EXIF with one tag, Orientation, set to `value`: a little-endian TIFF header and one IFD entry.
fn exif_orientation(value: u16) -> Vec<u8> {
    let mut exif = b"Exif\0\0II\x2A\x00\x08\x00\x00\x00".to_vec();
    exif.extend_from_slice(&1u16.to_le_bytes());
    exif.extend_from_slice(&0x0112u16.to_le_bytes());
    exif.extend_from_slice(&3u16.to_le_bytes());
    exif.extend_from_slice(&1u32.to_le_bytes());
    exif.extend_from_slice(&u32::from(value).to_le_bytes());
    exif.extend_from_slice(&0u32.to_le_bytes());
    exif
}

fn png(pixels: &[u8], width: u32, height: u32) -> io::Result<Vec<u8>> {
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut crc = Crc::new();
        crc.update(kind);
        crc.update(data);
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        out.extend_from_slice(&crc.sum().to_be_bytes());
    }
    let mut raw = Vec::with_capacity(pixels.len() + height as usize);
    for row in pixels.chunks_exact(width as usize * 4) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), Compression::new(6));
    encoder.write_all(&raw)?;
    let compressed = encoder.finish()?;

    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    // 8 bits per channel, RGBA, deflate, no filter beyond the per-row byte, not interlaced.
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &header);
    chunk(&mut out, b"IDAT", &compressed);
    chunk(&mut out, b"IEND", &[]);
    Ok(out)
}

/// Encodes one fixture page as its file.
pub fn encode_page(page: &ComicPage) -> io::Result<Vec<u8>> {
    let rgb = upright_rgb(page);
    Ok(match page.encoding {
        PageEncoding::Jpeg => jpeg(&rgb, page.width, page.height, ColorType::Rgb, None),
        PageEncoding::JpegRotated => {
            // Stored turned a quarter to the left, so that turning it a quarter to the right, as
            // orientation 6 asks, shows it upright.
            let (stored_width, stored_height) = (page.height, page.width);
            let mut stored = Vec::with_capacity(rgb.len());
            for sy in 0..stored_height {
                for sx in 0..stored_width {
                    let (x, y) = (stored_height - 1 - sy, sx);
                    stored.extend_from_slice(&quadrant_color(page, x, y));
                }
            }
            jpeg(
                &stored,
                stored_width,
                stored_height,
                ColorType::Rgb,
                Some(&exif_orientation(6)),
            )
        }
        PageEncoding::JpegGray => {
            let gray: Vec<u8> = rgb
                .as_chunks::<3>()
                .0
                .iter()
                .map(|p| {
                    ((u32::from(p[0]) * 299 + u32::from(p[1]) * 587 + u32::from(p[2]) * 114) / 1000)
                        as u8
                })
                .collect();
            jpeg(&gray, page.width, page.height, ColorType::Luma, None)
        }
        PageEncoding::JpegCmyk => {
            let cmyk: Vec<u8> = rgb
                .as_chunks::<3>()
                .0
                .iter()
                .flat_map(|p| {
                    let k = 255 - p[0].max(p[1]).max(p[2]);
                    let ink = |channel: u8| {
                        if k == 255 {
                            0
                        } else {
                            ((255 - u32::from(channel) - u32::from(k)) * 255 / (255 - u32::from(k)))
                                as u8
                        }
                    };
                    [ink(p[0]), ink(p[1]), ink(p[2]), k]
                })
                .collect();
            jpeg(&cmyk, page.width, page.height, ColorType::Cmyk, None)
        }
        PageEncoding::Png => {
            let rgba: Vec<u8> = rgb
                .as_chunks::<3>()
                .0
                .iter()
                .flat_map(|p| [p[0], p[1], p[2], 255])
                .collect();
            png(&rgba, page.width, page.height)?
        }
        PageEncoding::PngTransparent => {
            let mut rgba = Vec::with_capacity((page.width * page.height * 4) as usize);
            for y in 0..page.height {
                for x in 0..page.width {
                    if in_transparent_border(page, x, y) {
                        rgba.extend_from_slice(&[0, 0, 0, 0]);
                    } else {
                        let [r, g, b] = quadrant_color(page, x, y);
                        rgba.extend_from_slice(&[r, g, b, 255]);
                    }
                }
            }
            png(&rgba, page.width, page.height)?
        }
    })
}

/// Every entry of a comic fixture: the pages in a scrambled order, and what real archives also
/// carry, which must not become pages. With `manga`, a `ComicInfo.xml` asks for right to left.
pub fn comic_entries(manga: bool) -> io::Result<Vec<ArchiveEntry>> {
    let mut entries = Vec::new();
    entries.push(ArchiveEntry::new("extras/", Vec::new()));
    entries.push(ArchiveEntry::new(
        "notes.txt",
        b"Scanned for the tests.\n".to_vec(),
    ));
    // Stored in this order on purpose: p10 first, then the rest backwards with gaps.
    for index in [9, 0, 2, 1, 4, 3, 6, 5, 8, 7] {
        let page = &COMIC_PAGES[index];
        entries.push(ArchiveEntry::new(page.name, encode_page(page)?));
        if index == 0 {
            // macOS leaves these beside every file it touched.
            entries.push(ArchiveEntry::new(
                "__MACOSX/._p1.jpg",
                b"\x00\x05\x16\x07 resource fork".to_vec(),
            ));
        }
    }
    entries.push(ArchiveEntry::new(
        ".hidden.jpg",
        encode_page(&COMIC_PAGES[0])?,
    ));
    entries.push(ArchiveEntry::new("Thumbs.db", vec![0xD0, 0xCF, 0x11, 0xE0]));
    if manga {
        entries.push(ArchiveEntry::new(
            "ComicInfo.xml",
            b"<?xml version=\"1.0\"?>\n<ComicInfo>\n  <Title>Fixture</Title>\n  \
              <Manga>YesAndRightToLeft</Manga>\n</ComicInfo>\n"
                .to_vec(),
        ));
    }
    Ok(entries)
}

/// DOS date and time for 1 January 1980, midnight: the earliest a ZIP or RAR 4 can say.
const DOS_DATE: u16 = (1 << 5) | 1;
const DOS_TIME: u16 = 0;

fn crc32(data: &[u8]) -> u32 {
    let mut crc = Crc::new();
    crc.update(data);
    crc.sum()
}

/// A ZIP archive of `entries`, deflated if `compress` asks and that makes an entry smaller.
pub fn zip_bytes(entries: &[ArchiveEntry], compress: bool) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for entry in entries {
        let crc = crc32(&entry.data);
        let deflated = if compress && !entry.data.is_empty() {
            let mut encoder = DeflateEncoder::new(Vec::new(), Compression::new(6));
            encoder.write_all(&entry.data)?;
            Some(encoder.finish()?).filter(|data| data.len() < entry.data.len())
        } else {
            None
        };
        let (method, stored): (u16, &[u8]) = match &deflated {
            Some(data) => (8, data),
            None => (0, &entry.data),
        };
        let offset = out.len() as u32;
        let name = entry.name.as_bytes();
        // Bit 11: the name is UTF-8.
        let flags: u16 = 0x0800;

        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&flags.to_le_bytes());
        out.extend_from_slice(&method.to_le_bytes());
        out.extend_from_slice(&DOS_TIME.to_le_bytes());
        out.extend_from_slice(&DOS_DATE.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(stored.len() as u32).to_le_bytes());
        out.extend_from_slice(&(entry.data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name);
        out.extend_from_slice(stored);

        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&flags.to_le_bytes());
        central.extend_from_slice(&method.to_le_bytes());
        central.extend_from_slice(&DOS_TIME.to_le_bytes());
        central.extend_from_slice(&DOS_DATE.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&(stored.len() as u32).to_le_bytes());
        central.extend_from_slice(&(entry.data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        // Extra field, comment, disk number and internal attributes: none.
        central.extend_from_slice(&[0; 8]);
        // External attributes: MS-DOS directory bit for directories.
        let external: u32 = if entry.name.ends_with('/') { 0x10 } else { 0 };
        central.extend_from_slice(&external.to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name);
    }
    let central_at = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&central_at.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    Ok(out)
}

/// A RAR 4 block: its CRC is the low half of the CRC-32 of everything after the CRC field.
fn rar4_block(kind: u8, flags: u16, fields: &[u8]) -> Vec<u8> {
    let size = (7 + fields.len()) as u16;
    let mut header = vec![kind];
    header.extend_from_slice(&flags.to_le_bytes());
    header.extend_from_slice(&size.to_le_bytes());
    header.extend_from_slice(fields);
    let mut block = (crc32(&header) as u16).to_le_bytes().to_vec();
    block.extend_from_slice(&header);
    block
}

/// A RAR 4 archive of the files in `entries`, stored without compression. Directories are left out.
pub fn rar4_bytes(entries: &[ArchiveEntry]) -> Vec<u8> {
    let mut out = b"Rar!\x1a\x07\x00".to_vec();
    out.extend_from_slice(&rar4_block(0x73, 0, &[0; 6]));
    for entry in entries.iter().filter(|entry| !entry.name.ends_with('/')) {
        let name = entry.name.as_bytes();
        let mut fields = Vec::with_capacity(25 + name.len());
        fields.extend_from_slice(&(entry.data.len() as u32).to_le_bytes());
        fields.extend_from_slice(&(entry.data.len() as u32).to_le_bytes());
        // Host OS 2 is Windows.
        fields.push(2);
        fields.extend_from_slice(&crc32(&entry.data).to_le_bytes());
        fields
            .extend_from_slice(&((u32::from(DOS_DATE) << 16) | u32::from(DOS_TIME)).to_le_bytes());
        // Version 2.0 is enough to extract, and 0x30 means stored.
        fields.push(20);
        fields.push(0x30);
        fields.extend_from_slice(&(name.len() as u16).to_le_bytes());
        // FILE_ATTRIBUTE_ARCHIVE.
        fields.extend_from_slice(&0x20u32.to_le_bytes());
        fields.extend_from_slice(name);
        // 0x8000: the block is followed by data, whose size is the first field.
        out.extend_from_slice(&rar4_block(0x74, 0x8000, &fields));
        out.extend_from_slice(&entry.data);
    }
    out.extend_from_slice(&rar4_block(0x7b, 0x4000, &[]));
    out
}

/// A RAR 5 variable-length integer: seven bits per byte, low bits first.
fn vint(mut value: u64, out: &mut Vec<u8>) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// A RAR 5 header: CRC-32 of the size and what follows it, the size, then `fields`, which start
/// with the header's type.
fn rar5_header(fields: &[u8]) -> Vec<u8> {
    let mut sized = Vec::with_capacity(fields.len() + 3);
    vint(fields.len() as u64, &mut sized);
    sized.extend_from_slice(fields);
    let mut header = crc32(&sized).to_le_bytes().to_vec();
    header.extend_from_slice(&sized);
    header
}

/// A RAR 5 archive of the files in `entries`, stored without compression. Directories are left out.
pub fn rar5_bytes(entries: &[ArchiveEntry]) -> Vec<u8> {
    let mut out = b"Rar!\x1a\x07\x01\x00".to_vec();
    // Main archive header: type 1, no header flags, no archive flags.
    out.extend_from_slice(&rar5_header(&[1, 0, 0]));
    for entry in entries.iter().filter(|entry| !entry.name.ends_with('/')) {
        let mut fields = Vec::new();
        vint(2, &mut fields);
        // Header flag 2: a data area follows.
        vint(0x0002, &mut fields);
        vint(entry.data.len() as u64, &mut fields);
        // File flag 4: the data's CRC-32 is present.
        vint(0x0004, &mut fields);
        vint(entry.data.len() as u64, &mut fields);
        vint(0x20, &mut fields);
        fields.extend_from_slice(&crc32(&entry.data).to_le_bytes());
        // Compression: algorithm version 0, method 0 (stored), smallest dictionary.
        vint(0, &mut fields);
        // Host OS 0 is Windows.
        vint(0, &mut fields);
        vint(entry.name.len() as u64, &mut fields);
        fields.extend_from_slice(entry.name.as_bytes());
        out.extend_from_slice(&rar5_header(&fields));
        out.extend_from_slice(&entry.data);
    }
    // End of archive: type 5, no header flags, not followed by another volume.
    out.extend_from_slice(&rar5_header(&[5, 0, 0]));
    out
}

/// Writes every comic fixture into `dir`.
///
/// | File | |
/// |------|-|
/// | `comic.cbz` | ZIP, deflated |
/// | `comic-stored.cbz` | ZIP, stored |
/// | `comic-rar4.cbr` | RAR 4, stored |
/// | `comic-rar5.cbr` | RAR 5, stored |
/// | `manga.cbz` | ZIP whose `ComicInfo.xml` asks for right to left |
/// | `zip-named.cbr` | A ZIP named like a RAR, as many real ones are |
pub fn write_comic_fixtures(dir: &Path) -> io::Result<()> {
    let entries = comic_entries(false)?;
    std::fs::write(dir.join("comic.cbz"), zip_bytes(&entries, true)?)?;
    std::fs::write(dir.join("comic-stored.cbz"), zip_bytes(&entries, false)?)?;
    std::fs::write(dir.join("comic-rar4.cbr"), rar4_bytes(&entries))?;
    std::fs::write(dir.join("comic-rar5.cbr"), rar5_bytes(&entries))?;
    std::fs::write(dir.join("zip-named.cbr"), zip_bytes(&entries, true)?)?;
    std::fs::write(
        dir.join("manga.cbz"),
        zip_bytes(&comic_entries(true)?, true)?,
    )?;
    Ok(())
}

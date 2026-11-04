//! A comic book archive as a fixed-layout document: one image per page.
//!
//! Pages are the archive's images in natural order of their names. Each one is measured from its
//! header when the archive opens, turned upright as its EXIF orientation asks, and decoded only
//! when a tile of it is wanted. Tiles are cut from the decoded page by resampling exactly the
//! region they cover; the filter reaches across tile edges into the whole image, so tiles drawn
//! one by one match the page drawn whole.

use std::io::Cursor;
use std::time::Instant;

use fast_image_resize::images::Image;
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageReader, Limits};
use musaeum_archive::{Archive, ArchiveError};
use musaeum_shm::tile::{page_pixels, tile_in_page};
use musaeum_shm::wire::TileFailure;
use musaeum_shm::{PageContent, PageGeometry, TILE_SIZE, TILE_STRIDE, TextLayer, TileKey};
use musaeum_worker::{CancelProbe, Engine, Hints, RenderOutcome};
use quick_xml::Reader;
use quick_xml::events::Event;

use crate::order::{Role, natural};

/// Pixels per inch a page image is taken to have. Comics are usually scanned at about this, so a
/// scan of a printed page shows at the size it was printed, as a PDF page does.
pub const RASTER_DPI: f32 = 300.0;

/// Bytes read from the start of an image to find its size, which is in the header. A JPEG's EXIF
/// block can push it further, and then the whole image is read.
const PROBE_BYTES: usize = 64 * 1024;
/// Largest image file accepted, compressed.
const MAX_IMAGE_BYTES: usize = 256 << 20;
/// Largest `ComicInfo.xml` read.
const MAX_INFO_BYTES: usize = 1 << 20;
/// Most memory one decode may allocate: a 16-megapixel page decodes in a quarter of this.
const MAX_DECODE_BYTES: u64 = 768 << 20;
/// Longest image edge accepted, in pixels.
const MAX_IMAGE_EDGE: u32 = 32_768;
/// Decoded pages kept per worker. One page is four bytes a pixel, so about 25 MB for a
/// 2000 × 3000 scan; the second is the facing page of a spread.
const DECODED_CACHE_LEN: usize = 2;
/// Opaque white, under transparent pixels and around the page.
const PAPER: [u8; 4] = [255; 4];

/// Why an archive cannot be read as a comic.
#[derive(Debug)]
pub enum OpenError {
    /// libarchive could not read it.
    Archive(ArchiveError),
    /// It holds no image that could be a page.
    NoPages,
}

impl OpenError {
    /// The detail word reported to the host.
    pub fn code(&self) -> u32 {
        match self {
            Self::Archive(_) => 1,
            Self::NoPages => 2,
        }
    }
}

struct Page {
    /// Position in the archive.
    entry: usize,
    name: String,
    /// Upright size in pixels, or `None` if the header could not be read.
    pixels: Option<(u32, u32)>,
}

struct Decoded {
    page: u32,
    image: Image<'static>,
    last_used: u64,
}

/// An open comic and its most recently decoded pages.
pub struct Comic<'data> {
    data: &'data [u8],
    pages: Vec<Page>,
    sizes: Vec<PageGeometry>,
    direction: u8,
    geometry_us: u32,
    decoded: Vec<Decoded>,
    clock: u64,
    resizer: Resizer,
}

impl<'data> Comic<'data> {
    /// Opens an archive held in `data` and measures every page from its image header.
    pub fn open(data: &'data [u8]) -> Result<Self, OpenError> {
        let started = Instant::now();
        let mut archive = Archive::open(data).map_err(OpenError::Archive)?;
        let mut pages = Vec::new();
        let mut direction = 0;
        while let Some(entry) = archive.next_entry().map_err(OpenError::Archive)? {
            if !entry.is_file || entry.encrypted {
                continue;
            }
            match crate::order::role(&entry.name) {
                Role::Page => {
                    let pixels = measure(&mut archive);
                    pages.push(Page {
                        entry: entry.index,
                        name: entry.name,
                        pixels,
                    });
                }
                Role::ComicInfo => {
                    if let Ok(info) = archive.read_all(MAX_INFO_BYTES) {
                        direction = comic_info_direction(&info);
                    }
                }
                Role::Other => {}
            }
        }
        if pages.is_empty() {
            return Err(OpenError::NoPages);
        }
        pages.sort_by(|a, b| natural(&a.name, &b.name));

        // A page whose header cannot be read takes the size of the first one that could, so the
        // layout stays plausible; drawing it fails later, alone.
        let fallback = pages
            .iter()
            .find_map(|page| page.pixels)
            .unwrap_or((1988, 3056));
        let sizes = pages
            .iter()
            .map(|page| points(page.pixels.unwrap_or(fallback)))
            .collect();
        Ok(Self {
            data,
            pages,
            sizes,
            direction,
            geometry_us: u32::try_from(started.elapsed().as_micros()).unwrap_or(u32::MAX),
            decoded: Vec::with_capacity(DECODED_CACHE_LEN),
            clock: 0,
            resizer: Resizer::new(),
        })
    }

    /// Pages in the comic.
    pub fn page_count(&self) -> u32 {
        self.pages.len() as u32
    }

    /// Draws a whole page at `scale` in one resampling, as RGBA rows packed tightly: what tiles are
    /// cut from, for tests and the visual comparison. The worker never uses it.
    pub fn render_page(
        &mut self,
        page: u32,
        scale: u32,
    ) -> Result<(u32, u32, Vec<u8>), TileFailure> {
        let size = *self
            .sizes
            .get(page as usize)
            .ok_or(TileFailure::OutOfBounds)?;
        let width = page_pixels(size.width_pt, scale) as u32;
        let height = page_pixels(size.height_pt, scale) as u32;
        let (at, owned, _) = self.decoded(page, false)?;
        let source = match &owned {
            Some(image) => image,
            None => &self.decoded[at].image,
        };
        let filter = if source.width() > width || source.height() > height {
            FilterType::Lanczos3
        } else {
            FilterType::CatmullRom
        };
        let mut whole = Image::new(width, height, PixelType::U8x4);
        self.resizer
            .resize(
                source,
                &mut whole,
                &ResizeOptions::new().resize_alg(ResizeAlg::Convolution(filter)),
            )
            .map_err(|_| TileFailure::RenderFailed)?;
        Ok((width, height, whole.into_vec()))
    }

    /// The page's name in the archive, for messages.
    pub fn page_name(&self, page: u32) -> Option<&str> {
        self.pages.get(page as usize).map(|page| page.name.as_str())
    }

    /// The upright, opaque pixels of a page, decoded or from the cache. Background work does
    /// not keep what it decodes, so the pages being read stay cached.
    fn decoded(
        &mut self,
        page: u32,
        keep: bool,
    ) -> Result<(usize, Option<Image<'static>>, u32), TileFailure> {
        self.clock += 1;
        if let Some(at) = self.decoded.iter().position(|decoded| decoded.page == page) {
            self.decoded[at].last_used = self.clock;
            return Ok((at, None, 0));
        }
        let started = Instant::now();
        let entry = self
            .pages
            .get(page as usize)
            .ok_or(TileFailure::OutOfBounds)?
            .entry;
        let bytes = read_entry(self.data, entry).ok_or(TileFailure::PageLoad)?;
        let image = decode_upright(&bytes).ok_or(TileFailure::DecodeFailed)?;
        let load_us = u32::try_from(started.elapsed().as_micros()).unwrap_or(u32::MAX);
        if !keep {
            return Ok((usize::MAX, Some(image), load_us));
        }
        if self.decoded.len() == DECODED_CACHE_LEN
            && let Some(oldest) =
                (0..self.decoded.len()).min_by_key(|&at| self.decoded[at].last_used)
        {
            self.decoded.swap_remove(oldest);
        }
        self.decoded.push(Decoded {
            page,
            image,
            last_used: self.clock,
        });
        Ok((self.decoded.len() - 1, None, load_us))
    }
}

/// Upright pixel size of a page image whose data is the archive's current entry.
fn measure(archive: &mut Archive<'_>) -> Option<(u32, u32)> {
    let prefix = archive.read_prefix(PROBE_BYTES).ok()?;
    if let Some(size) = upright_size(&prefix) {
        return Some(size);
    }
    // The size lies beyond the first bytes, behind a large EXIF or color profile block.
    if prefix.len() < PROBE_BYTES {
        return None;
    }
    let mut rest = archive.read_all(MAX_IMAGE_BYTES).ok()?;
    let mut whole = prefix;
    whole.append(&mut rest);
    upright_size(&whole)
}

fn reader(bytes: &[u8]) -> Option<ImageReader<Cursor<&[u8]>>> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_EDGE);
    limits.max_image_height = Some(MAX_IMAGE_EDGE);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    Some(reader)
}

fn upright_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut decoder = reader(bytes)?.into_decoder().ok()?;
    let (width, height) = decoder.dimensions();
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    if width == 0 || height == 0 {
        return None;
    }
    Some(if swaps_axes(orientation) {
        (height, width)
    } else {
        (width, height)
    })
}

fn swaps_axes(orientation: Orientation) -> bool {
    matches!(
        orientation,
        Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH
    )
}

/// The page size in points of an image `pixels` in size, at [`RASTER_DPI`].
fn points((width, height): (u32, u32)) -> PageGeometry {
    let scale = 72.0 / RASTER_DPI;
    PageGeometry {
        width_pt: width as f32 * scale,
        height_pt: height as f32 * scale,
    }
}

/// The whole data of the entry at `index`, from a fresh read of the archive. For ZIP this seeks
/// past the entries before it; a solid RAR has to decompress them, which is that format's cost.
fn read_entry(data: &[u8], index: usize) -> Option<Vec<u8>> {
    let mut archive = Archive::open(data).ok()?;
    while let Some(entry) = archive.next_entry().ok()? {
        if entry.index == index {
            return archive.read_all(MAX_IMAGE_BYTES).ok();
        }
    }
    None
}

/// Decodes an image, turns it upright, and flattens any transparency onto white paper. Public so
/// that tests and the visual comparison can draw a page whole, the way tiles are cut from it.
pub fn decode_upright(bytes: &[u8]) -> Option<Image<'static>> {
    let mut decoder = reader(bytes)?.into_decoder().ok()?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut image = DynamicImage::from_decoder(decoder).ok()?;
    image.apply_orientation(orientation);
    let mut rgba = image.into_rgba8();
    for pixel in rgba.pixels_mut() {
        let [red, green, blue, alpha] = pixel.0;
        if alpha < 255 {
            let over_white = |channel: u8| {
                ((u32::from(channel) * u32::from(alpha) + 255 * (255 - u32::from(alpha)) + 127)
                    / 255) as u8
            };
            pixel.0 = [over_white(red), over_white(green), over_white(blue), 255];
        }
    }
    let (width, height) = rgba.dimensions();
    Image::from_vec_u8(width, height, rgba.into_raw(), PixelType::U8x4).ok()
}

/// Reads ComicRack's `<Manga>` element: manga is read right to left.
fn comic_info_direction(xml: &[u8]) -> u8 {
    let mut reader = Reader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut in_manga = false;
    // Elements nest at most a few levels; a document that keeps going is not ComicRack's.
    for _ in 0..10_000 {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(element)) => {
                in_manga = element.local_name().as_ref().eq_ignore_ascii_case("Manga");
            }
            Ok(Event::Text(text)) if in_manga => {
                let value = text.trim();
                return if value.eq_ignore_ascii_case("YesAndRightToLeft")
                    || value.eq_ignore_ascii_case("Yes")
                {
                    2
                } else {
                    0
                };
            }
            Ok(Event::End(_)) => in_manga = false,
            Ok(Event::Eof) | Err(_) => return 0,
            Ok(_) => {}
        }
        buffer.clear();
    }
    0
}

impl Engine for Comic<'_> {
    fn page_sizes(&self) -> &[PageGeometry] {
        &self.sizes
    }

    fn geometry_us(&self) -> u32 {
        self.geometry_us
    }

    fn hints(&mut self) -> Hints {
        Hints {
            page_layout: 0,
            direction: self.direction,
        }
    }

    unsafe fn render_tile(
        &mut self,
        target: *mut u8,
        tile: TileKey,
        background: bool,
        probe: &CancelProbe<'_>,
    ) -> Result<RenderOutcome, TileFailure> {
        let size = *self
            .sizes
            .get(tile.page as usize)
            .ok_or(TileFailure::OutOfBounds)?;
        if !tile_in_page(tile, size) {
            return Err(TileFailure::OutOfBounds);
        }
        if probe.is_cancelled() {
            return Ok(RenderOutcome::Cancelled);
        }
        let (at, owned, load_us) = self.decoded(tile.page, !background)?;
        // Decoding can take tens of milliseconds, and the demand may have moved on since.
        if probe.is_cancelled() {
            return Ok(RenderOutcome::Cancelled);
        }
        let source = match &owned {
            Some(image) => image,
            None => &self.decoded[at].image,
        };

        let page_width = page_pixels(size.width_pt, tile.scale) as f64;
        let page_height = page_pixels(size.height_pt, tile.scale) as f64;
        let edge = u64::from(TILE_SIZE);
        let left = u64::from(tile.x) * edge;
        let top = u64::from(tile.y) * edge;
        let width = (page_width as u64 - left).min(edge) as u32;
        let height = (page_height as u64 - top).min(edge) as u32;
        // Source pixels per page pixel, on each axis.
        let (sx, sy) = (
            f64::from(source.width()) / page_width,
            f64::from(source.height()) / page_height,
        );
        let filter = if sx > 1.0 || sy > 1.0 {
            FilterType::Lanczos3
        } else {
            FilterType::CatmullRom
        };
        let options = ResizeOptions::new()
            .crop(
                left as f64 * sx,
                top as f64 * sy,
                f64::from(width) * sx,
                f64::from(height) * sy,
            )
            .resize_alg(ResizeAlg::Convolution(filter));
        let mut region = Image::new(width, height, PixelType::U8x4);
        self.resizer
            .resize(source, &mut region, &options)
            .map_err(|_| TileFailure::RenderFailed)?;

        // SAFETY: the caller guarantees `target` holds a whole tile and that nothing else touches
        // it until this returns.
        let slot =
            unsafe { std::slice::from_raw_parts_mut(target, TILE_STRIDE * TILE_SIZE as usize) };
        let row = width as usize * 4;
        for (y, line) in slot.as_chunks_mut::<TILE_STRIDE>().0.iter_mut().enumerate() {
            if y < height as usize {
                line[..row].copy_from_slice(&region.buffer()[y * row..(y + 1) * row]);
                for pixel in line[row..].as_chunks_mut::<4>().0 {
                    *pixel = PAPER;
                }
            } else {
                line.fill(255);
            }
        }
        Ok(RenderOutcome::Finished { load_us })
    }

    fn analyze(&mut self, _page: u32) -> Result<PageContent, TileFailure> {
        // Every page of a comic is a picture of the page, and an archive has no text layer.
        Ok(PageContent {
            text_chars: 0,
            image_coverage: 1.0,
        })
    }

    fn text_layer(&mut self, _page: u32) -> Result<TextLayer, TileFailure> {
        Ok(TextLayer::default())
    }

    fn trim(&mut self) {
        self.decoded.clear();
        self.decoded.shrink_to_fit();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manga_reads_right_to_left_and_anything_else_is_unstated() {
        let info = |manga: &str| {
            format!(
                "<?xml version=\"1.0\"?><ComicInfo><Title>x</Title><Manga>{manga}</Manga></ComicInfo>"
            )
        };
        assert_eq!(
            comic_info_direction(info("YesAndRightToLeft").as_bytes()),
            2
        );
        assert_eq!(comic_info_direction(info("Yes").as_bytes()), 2);
        assert_eq!(comic_info_direction(info("No").as_bytes()), 0);
        assert_eq!(comic_info_direction(info("Unknown").as_bytes()), 0);
        assert_eq!(
            comic_info_direction(b"<ComicInfo><Title>Manga</Title></ComicInfo>"),
            0
        );
        assert_eq!(comic_info_direction(b"not xml at all <<<"), 0);
    }

    #[test]
    fn a_page_at_300_dpi_is_its_printed_size() {
        let page = points((1988, 3056));
        assert!((page.width_pt / 72.0 - 6.63).abs() < 0.01);
        assert!((page.height_pt / 72.0 - 10.19).abs() < 0.01);
    }
}

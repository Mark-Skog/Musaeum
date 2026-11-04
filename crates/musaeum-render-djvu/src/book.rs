//! A DjVu document as a fixed-layout document.
//!
//! Every page's size comes from its header, at the resolution it was scanned at, so a page shows at
//! its physical size like a PDF page does. DjVuLibre scales the page to each tile's scale itself,
//! decoding the wavelet layers only as finely as that scale needs.

use std::time::Instant;

use musaeum_djvu::{Document, OpenError, RawWord};
use musaeum_shm::tile::{page_pixels, tile_in_page};
use musaeum_shm::wire::TileFailure;
use musaeum_shm::{PageContent, PageGeometry, TILE_SIZE, TILE_STRIDE, TextLayer, TileKey, Word};
use musaeum_worker::{CancelProbe, Engine, Hints, RenderOutcome};

/// The resolution assumed when a page states none that makes sense.
const DEFAULT_DPI: u32 = 300;

struct Page {
    /// Shown size in pixels.
    width: u32,
    height: u32,
    /// Quarter turns counter-clockwise it is shown at.
    rotation: u8,
}

/// An open DjVu document.
pub struct Book {
    document: Document,
    pages: Vec<Page>,
    sizes: Vec<PageGeometry>,
    geometry_us: u32,
}

impl Book {
    /// Opens a bundled document from its bytes and reads every page's header.
    pub fn open(data: &[u8]) -> Result<Self, OpenError> {
        let started = Instant::now();
        let mut document = Document::open(data)?;
        let count = document.page_count();
        let mut pages = Vec::with_capacity(count as usize);
        let mut sizes = Vec::with_capacity(count as usize);
        let mut fallback = None;
        for index in 0..count {
            match document.page_info(index) {
                Some(info) => {
                    let dpi = if (25..=6000).contains(&info.dpi) {
                        info.dpi
                    } else {
                        DEFAULT_DPI
                    };
                    let size = PageGeometry {
                        width_pt: info.width as f32 * 72.0 / dpi as f32,
                        height_pt: info.height as f32 * 72.0 / dpi as f32,
                    };
                    fallback.get_or_insert(size);
                    pages.push(Page {
                        width: info.width,
                        height: info.height,
                        rotation: info.rotation,
                    });
                    sizes.push(size);
                }
                // An unreadable header: the page keeps a plausible place in the layout and fails
                // alone when it is drawn.
                None => {
                    pages.push(Page {
                        width: 0,
                        height: 0,
                        rotation: 0,
                    });
                    sizes.push(PageGeometry {
                        width_pt: 0.0,
                        height_pt: 0.0,
                    });
                }
            }
        }
        let fallback = fallback.unwrap_or(PageGeometry {
            width_pt: 612.0,
            height_pt: 792.0,
        });
        for size in &mut sizes {
            if size.width_pt <= 0.0 {
                *size = fallback;
            }
        }
        Ok(Self {
            document,
            pages,
            sizes,
            geometry_us: u32::try_from(started.elapsed().as_micros()).unwrap_or(u32::MAX),
        })
    }

    /// Pages in the document.
    pub fn page_count(&self) -> u32 {
        self.pages.len() as u32
    }

    /// Draws a whole page at `scale` in one call, as RGBA rows packed tightly: a reference for
    /// tests and the visual comparison. The worker never uses it.
    pub fn render_page(
        &mut self,
        index: u32,
        scale: u32,
    ) -> Result<(u32, u32, Vec<u8>), TileFailure> {
        self.page(index)?;
        let size = self.sizes[index as usize];
        let width = page_pixels(size.width_pt, scale) as u32;
        let height = page_pixels(size.height_pt, scale) as u32;
        let mut pixels = vec![0u8; width as usize * height as usize * 4];
        // SAFETY: `pixels` holds `height` rows of `width * 4` bytes, and nothing else uses it.
        unsafe {
            self.document.render(
                index,
                false,
                (width, height),
                (0, 0, width, height),
                pixels.as_mut_ptr(),
                width as usize * 4,
            )
        }
        .map_err(|_| TileFailure::DecodeFailed)?;
        Ok((width, height, pixels))
    }

    fn page(&self, index: u32) -> Result<&Page, TileFailure> {
        match self.pages.get(index as usize) {
            Some(page) if page.width > 0 => Ok(page),
            Some(_) => Err(TileFailure::PageLoad),
            None => Err(TileFailure::OutOfBounds),
        }
    }
}

/// A word's box from DjVu, in pixels of the page as stored with the origin at the bottom left, as
/// fractions of the page as shown: `[left, top, right, bottom]` from its top-left corner.
fn to_shown(page: &Page, [x0, y0, x1, y1]: [i32; 4]) -> [f32; 4] {
    // The stored page's size, before rotation.
    let (width, height) = if page.rotation & 1 == 1 {
        (page.height, page.width)
    } else {
        (page.width, page.height)
    };
    let (width, height) = (f64::from(width.max(1)), f64::from(height.max(1)));
    let shown = |x: i32, y: i32| {
        let u = f64::from(x) / width;
        let v = 1.0 - f64::from(y) / height;
        // Turning a quarter counter-clockwise takes the stored top-right corner to the top left.
        match page.rotation & 3 {
            1 => (v, 1.0 - u),
            2 => (1.0 - u, 1.0 - v),
            3 => (1.0 - v, u),
            _ => (u, v),
        }
    };
    let (u0, v0) = shown(x0, y1);
    let (u1, v1) = shown(x1, y0);
    [
        u0.min(u1) as f32,
        v0.min(v1) as f32,
        u0.max(u1) as f32,
        v0.max(v1) as f32,
    ]
}

fn into_layer(page: &Page, words: Vec<RawWord>) -> TextLayer {
    TextLayer {
        words: words
            .into_iter()
            .filter(|word| !word.text.trim().is_empty())
            .map(|word| Word {
                text: word.text.trim().to_owned(),
                rect: to_shown(page, word.rect),
                line_end: word.line_end,
            })
            .collect(),
    }
}

impl Engine for Book {
    fn page_sizes(&self) -> &[PageGeometry] {
        &self.sizes
    }

    fn geometry_us(&self) -> u32 {
        self.geometry_us
    }

    fn hints(&mut self) -> Hints {
        // DjVu states no reading direction and no page layout.
        Hints::default()
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
        self.page(tile.page)?;
        if probe.is_cancelled() {
            return Ok(RenderOutcome::Cancelled);
        }
        let page_width = page_pixels(size.width_pt, tile.scale);
        let page_height = page_pixels(size.height_pt, tile.scale);
        let edge = u64::from(TILE_SIZE);
        let left = u64::from(tile.x) * edge;
        let top = u64::from(tile.y) * edge;
        let width = (page_width - left).min(edge) as u32;
        let height = (page_height - top).min(edge) as u32;
        let (Ok(page_width), Ok(page_height)) =
            (u32::try_from(page_width), u32::try_from(page_height))
        else {
            return Err(TileFailure::OutOfBounds);
        };

        let started = Instant::now();
        // SAFETY: the caller guarantees `target` holds a whole tile that nothing else touches;
        // the rows written are `height` ≤ TILE_SIZE rows of TILE_STRIDE bytes, each with at least
        // `width * 4` bytes, since `width` ≤ TILE_SIZE.
        let drawn = unsafe {
            self.document.render(
                tile.page,
                !background,
                (page_width, page_height),
                (left as u32, top as u32, width, height),
                target,
                TILE_STRIDE,
            )
        };
        drawn.map_err(|error| match error {
            musaeum_djvu::RenderError::Decode => TileFailure::DecodeFailed,
            musaeum_djvu::RenderError::Draw => TileFailure::RenderFailed,
        })?;
        let load_us = u32::try_from(started.elapsed().as_micros()).unwrap_or(u32::MAX);

        // White paper around the page's part, as the other formats leave it.
        // SAFETY: as above; the rendering is over and nothing else touches the slot.
        let slot =
            unsafe { std::slice::from_raw_parts_mut(target, TILE_STRIDE * TILE_SIZE as usize) };
        let row = width as usize * 4;
        for (y, line) in slot.as_chunks_mut::<TILE_STRIDE>().0.iter_mut().enumerate() {
            if y < height as usize {
                line[row..].fill(255);
            } else {
                line.fill(255);
            }
        }
        if probe.is_cancelled() {
            return Ok(RenderOutcome::Cancelled);
        }
        Ok(RenderOutcome::Finished { load_us })
    }

    fn analyze(&mut self, index: u32) -> Result<PageContent, TileFailure> {
        self.page(index)?;
        let words = self
            .document
            .words(index)
            .map_err(|_| TileFailure::DecodeFailed)?;
        let text_chars = words
            .iter()
            .map(|word| word.text.chars().filter(|c| !c.is_whitespace()).count())
            .sum::<usize>();
        // A DjVu page is always a picture of the page; OCR, when there was any, is its text layer.
        Ok(PageContent {
            text_chars: u32::try_from(text_chars).unwrap_or(u32::MAX),
            image_coverage: 1.0,
        })
    }

    fn text_layer(&mut self, index: u32) -> Result<TextLayer, TileFailure> {
        let words = self
            .document
            .words(index)
            .map_err(|_| TileFailure::DecodeFailed)?;
        Ok(into_layer(self.page(index)?, words))
    }

    fn trim(&mut self) {
        self.document.trim();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boxes_turn_with_the_page() {
        // A 600 × 800 page; its top-left tenth, in DjVu's bottom-left coordinates.
        let corner = [0, 720, 60, 800];
        let page = |rotation, (width, height)| Page {
            width,
            height,
            rotation,
        };
        assert_eq!(to_shown(&page(0, (600, 800)), corner), [0.0, 0.0, 0.1, 0.1]);
        // Shown turned a quarter counter-clockwise, 800 × 600: the top left goes to the bottom left.
        assert_eq!(to_shown(&page(1, (800, 600)), corner), [0.0, 0.9, 0.1, 1.0]);
        assert_eq!(to_shown(&page(2, (600, 800)), corner), [0.9, 0.9, 1.0, 1.0]);
        assert_eq!(to_shown(&page(3, (800, 600)), corner), [0.9, 0.0, 1.0, 0.1]);
    }
}

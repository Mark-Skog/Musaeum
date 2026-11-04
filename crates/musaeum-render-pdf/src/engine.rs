//! A small safe layer over the PDFium calls this worker needs.
//!
//! It uses `pdfium-render`'s raw bindings instead of its high-level API, because tile
//! rendering depends on two things that API does not offer: rendering into a caller's
//! buffer (the shared-memory slot) and the progressive renderer, whose pause callback
//! is the only way to stop a page that is already rendering.

use std::ffi::{c_int, c_ulong, c_void};
use std::marker::PhantomData;
use std::path::Path;
use std::ptr;
use std::time::Instant;

use musaeum_ingest::parse::pdf_catalog::{Direction, PageLayout, catalog_hints};
use musaeum_shm::tile::{page_pixels, tile_in_page};
use musaeum_shm::wire::TileFailure;
use musaeum_shm::{PageContent, PageGeometry, TILE_SIZE, TILE_STRIDE, TextLayer, TileKey, Word};
use musaeum_worker::{CancelProbe, Engine, Hints, RenderOutcome};
use pdfium_render::prelude::{
    FPDF_BOOL, FPDF_DOCUMENT, FPDF_LIBRARY_CONFIG, FPDF_PAGE, FPDF_PAGEOBJECT, FPDF_TEXTPAGE,
    FS_MATRIX, FS_RECTF, FS_SIZEF, IFSDK_PAUSE, Pdfium as Binder, PdfiumError,
    PdfiumLibraryBindings,
};

// Values from `fpdfview.h`, `fpdf_progressive.h` and `fpdf_edit.h`.
const FPDF_ANNOT: c_int = 0x01;
const FPDF_REVERSE_BYTE_ORDER: c_int = 0x10;
const FPDF_BITMAP_BGRA: c_int = 4;
const FPDF_RENDER_TOBECONTINUED: c_int = 1;
const FPDF_RENDER_DONE: c_int = 2;
const FPDF_PAGEOBJ_IMAGE: c_int = 3;
const FPDF_PAGEOBJ_FORM: c_int = 5;

/// Opaque white, matching the compositor's paper plate so a tile replacing the plate
/// does not visibly change the page colour. Channel order is irrelevant for white.
const PAPER: u32 = 0xFFFF_FFFF;

/// Used when PDFium cannot report a page's size, so the layout stays stable and the
/// compositor shows a plate where the unreadable page would be.
const FALLBACK_SIZE: PageGeometry = PageGeometry {
    width_pt: 612.0,
    height_pt: 792.0,
};

/// Parsed pages kept open per worker. Consecutive tiles usually come from the same page, and
/// parsing a page's content is often far more expensive than rasterizing one tile. But each one
/// holds its decoded images, a few megabytes per scanned page in every worker, so this stays low.
const PAGE_CACHE_LEN: usize = 3;

/// Cells along each side of the grid that measures how much of a page pictures cover. Fine enough
/// to tell a full-page scan from a figure, coarse enough to cost nothing.
const COVERAGE_GRID: usize = 64;

/// Pixels drawn left of and above every tile, then thrown away. PDFium places a glyph whose origin
/// lies left of or above the bitmap differently from one inside it, so a glyph crossing a tile's
/// left or top edge came out shifted against the same glyph in the tile beside it, which showed as
/// a broken letter at the seam. With this margin, such a glyph has its origin inside the bitmap,
/// unless it is wider or deeper than the margin.
const TILE_MARGIN: c_int = 64;

/// Pixels drawn right of and below every tile, then thrown away. PDFium's image resampling treats the
/// last column and row before a bitmap's edge differently, which left faint lines in scanned pages
/// along tile seams. Far fewer are needed than on the other side: the filter only reaches a few pixels.
/// Both margins together make a third more pixels per tile; on real books, the render time changed
/// less than a busy machine's own variation.
const TILE_TRAILING_MARGIN: c_int = 16;

/// Form XObjects nest; deeper than this, a page is more likely hostile than real.
const MAX_FORM_DEPTH: u32 = 16;

/// The loaded library. Not `Send` or `Sync`, because PDFium has global state and allows
/// only one thread at a time; the compiler now enforces that here.
pub struct Pdfium {
    bindings: Box<dyn PdfiumLibraryBindings>,
    _single_threaded: PhantomData<*const ()>,
}

impl Pdfium {
    /// Loads and initializes PDFium from an absolute path.
    pub fn load(dll: &Path) -> Result<Self, PdfiumError> {
        let bindings = Binder::bind_to_library(dll)?;
        // SAFETY: all zeroes is a valid value for this plain C structure, meaning null
        // pointers and default settings. Version 2 tells PDFium to read only the fields
        // that existed in that version, which this structure has in any release.
        let config = FPDF_LIBRARY_CONFIG {
            version: 2,
            ..unsafe { std::mem::zeroed() }
        };
        // SAFETY: called once, before any other PDFium function, with a valid config.
        unsafe { bindings.FPDF_InitLibraryWithConfig(&config) };
        Ok(Self {
            bindings,
            _single_threaded: PhantomData,
        })
    }
}

impl Drop for Pdfium {
    fn drop(&mut self) {
        // SAFETY: `Document` borrows `Pdfium`, so every document is already closed.
        unsafe { self.bindings.FPDF_DestroyLibrary() };
    }
}

/// PDFium refused the document. `pdfium_error` is `FPDF_GetLastError`'s value.
#[derive(Debug)]
pub struct OpenError {
    /// 3 means corrupted, 4 means password protected, and so on.
    pub pdfium_error: u32,
}

/// PDFium calls this periodically during a progressive render.
unsafe extern "C" fn need_to_pause(this: *mut IFSDK_PAUSE) -> FPDF_BOOL {
    // SAFETY: PDFium passes back the structure built in `render_tile`, whose `user`
    // field points to a probe that outlives the whole render.
    let probe = unsafe { &*(*this).user.cast::<CancelProbe<'_>>() };
    FPDF_BOOL::from(probe.is_cancelled())
}

struct CachedPage {
    index: u32,
    handle: FPDF_PAGE,
    last_used: u64,
}

/// A page loaded for one job. Either it is in the cache, or this job owns it and closes it.
struct LoadedPage {
    handle: FPDF_PAGE,
    load_us: u32,
    owned: bool,
}

/// An open PDF and its most recently used pages.
pub struct Document<'lib, 'data> {
    pdfium: &'lib Pdfium,
    handle: FPDF_DOCUMENT,
    data: &'data [u8],
    sizes: Vec<PageGeometry>,
    geometry_us: u32,
    pages: Vec<CachedPage>,
    clock: u64,
    /// The tile with its margin, before the tile is copied into its slot.
    scratch: Vec<u8>,
}

impl<'lib, 'data> Document<'lib, 'data> {
    /// Opens a document from mapped bytes and reads all of its page sizes.
    ///
    /// `FPDF_GetPageSizeByIndexF` does not parse page content, so this costs a page-tree
    /// walk rather than a rasterization, even for thousands of pages.
    pub fn open(pdfium: &'lib Pdfium, data: &'data [u8]) -> Result<Self, OpenError> {
        // SAFETY: `data` outlives the returned document thanks to `'data`.
        let handle = unsafe { pdfium.bindings.FPDF_LoadMemDocument64(data, None) };
        if handle.is_null() {
            // SAFETY: the library is initialized, and this reads a thread-local-style value.
            let pdfium_error = unsafe { pdfium.bindings.FPDF_GetLastError() };
            return Err(OpenError { pdfium_error });
        }

        let started = Instant::now();
        // SAFETY: `handle` was just checked to be a valid document.
        let page_count = unsafe { pdfium.bindings.FPDF_GetPageCount(handle) }.max(0);
        let mut sizes = Vec::with_capacity(page_count as usize);
        for index in 0..page_count {
            let mut size = FS_SIZEF {
                width: 0.0,
                height: 0.0,
            };
            // SAFETY: the document is valid, `index` is in range, `size` is a valid out-pointer.
            let ok = unsafe {
                pdfium
                    .bindings
                    .FPDF_GetPageSizeByIndexF(handle, index, &mut size)
            };
            sizes.push(if ok != 0 && size.width > 0.0 && size.height > 0.0 {
                PageGeometry {
                    width_pt: size.width,
                    height_pt: size.height,
                }
            } else {
                FALLBACK_SIZE
            });
        }

        Ok(Self {
            pdfium,
            handle,
            data,
            sizes,
            geometry_us: u32::try_from(started.elapsed().as_micros()).unwrap_or(u32::MAX),
            pages: Vec::with_capacity(PAGE_CACHE_LEN),
            clock: 0,
            scratch: Vec::new(),
        })
    }

    /// Pages in the document.
    pub fn page_count(&self) -> u32 {
        self.sizes.len() as u32
    }

    /// Draws a whole page at `scale` in one call, without tiles or the progressive renderer, as RGBA
    /// rows packed tightly. A reference to hold the tiled pipeline against, for tests and the visual
    /// comparison; the worker never uses it.
    pub fn render_page(
        &mut self,
        index: u32,
        scale: u32,
    ) -> Result<(u32, u32, Vec<u8>), TileFailure> {
        let size = *self
            .sizes
            .get(index as usize)
            .ok_or(TileFailure::OutOfBounds)?;
        let width = page_pixels(size.width_pt, scale);
        let height = page_pixels(size.height_pt, scale);
        let (Ok(w), Ok(h)) = (c_int::try_from(width), c_int::try_from(height)) else {
            return Err(TileFailure::OutOfBounds);
        };
        let stride = w.checked_mul(4).ok_or(TileFailure::OutOfBounds)?;
        let mut pixels = vec![0u8; width as usize * height as usize * 4];
        let page = self.load(index, false)?;
        let bindings = &self.pdfium.bindings;
        // SAFETY: `pixels` holds `h` rows of `stride` bytes and outlives the bitmap, which is
        // destroyed before this returns; PDFium does not take ownership of an external buffer.
        let drawn = unsafe {
            let bitmap = bindings.FPDFBitmap_CreateEx(
                w,
                h,
                FPDF_BITMAP_BGRA,
                pixels.as_mut_ptr().cast::<c_void>(),
                stride,
            );
            if bitmap.is_null() {
                false
            } else {
                let _ = bindings.FPDFBitmap_FillRect(bitmap, 0, 0, w, h, PAPER);
                bindings.FPDF_RenderPageBitmap(
                    bitmap,
                    page.handle,
                    0,
                    0,
                    w,
                    h,
                    0,
                    FPDF_ANNOT | FPDF_REVERSE_BYTE_ORDER,
                );
                bindings.FPDFBitmap_Destroy(bitmap);
                true
            }
        };
        self.done(&page);
        if !drawn {
            return Err(TileFailure::BitmapCreate);
        }
        Ok((width as u32, height as u32, pixels))
    }

    /// Closes the cached pages. They load again when they are needed.
    pub fn release_pages(&mut self) {
        for page in self.pages.drain(..) {
            // SAFETY: the page is open and no render is in progress between jobs.
            unsafe { self.pdfium.bindings.FPDF_ClosePage(page.handle) };
        }
    }

    /// A parsed page, from the cache if it is there. With `keep`, a page that had to be loaded
    /// goes into the cache; without it, the caller owns it and must close it with [`Self::done`].
    fn load(&mut self, index: u32, keep: bool) -> Result<LoadedPage, TileFailure> {
        self.clock += 1;
        if let Some(cached) = self.pages.iter_mut().find(|page| page.index == index) {
            cached.last_used = self.clock;
            return Ok(LoadedPage {
                handle: cached.handle,
                load_us: 0,
                owned: false,
            });
        }

        let started = Instant::now();
        let page_index = c_int::try_from(index).map_err(|_| TileFailure::OutOfBounds)?;
        // SAFETY: the document is valid and PDFium checks the index itself.
        let handle = unsafe { self.pdfium.bindings.FPDF_LoadPage(self.handle, page_index) };
        if handle.is_null() {
            return Err(TileFailure::PageLoad);
        }
        let load_us = u32::try_from(started.elapsed().as_micros()).unwrap_or(u32::MAX);
        if !keep {
            return Ok(LoadedPage {
                handle,
                load_us,
                owned: true,
            });
        }

        if self.pages.len() == PAGE_CACHE_LEN
            && let Some(oldest) = (0..self.pages.len()).min_by_key(|&at| self.pages[at].last_used)
        {
            let evicted = self.pages.swap_remove(oldest);
            // SAFETY: the page is open and no render is in progress between jobs.
            unsafe { self.pdfium.bindings.FPDF_ClosePage(evicted.handle) };
        }
        self.pages.push(CachedPage {
            index,
            handle,
            last_used: self.clock,
        });
        Ok(LoadedPage {
            handle,
            load_us,
            owned: false,
        })
    }

    /// Closes a page the job owned.
    fn done(&self, page: &LoadedPage) {
        if page.owned {
            // SAFETY: the page was loaded for this job alone, which is over.
            unsafe { self.pdfium.bindings.FPDF_ClosePage(page.handle) };
        }
    }

    /// The page's visible box in PDF space, and how many quarter turns clockwise it is shown at.
    fn page_frame(&self, page: FPDF_PAGE) -> (FS_RECTF, i32) {
        let bindings = &self.pdfium.bindings;
        let mut rect = FS_RECTF {
            left: 0.0,
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
        };
        // SAFETY: the page is open and `rect` is a valid out-pointer.
        let ok = unsafe { bindings.FPDF_GetPageBoundingBox(page, &mut rect) };
        if ok == 0 || rect.right <= rect.left || rect.top <= rect.bottom {
            // SAFETY: the page is open.
            let (width, height) = unsafe {
                (
                    bindings.FPDF_GetPageWidthF(page),
                    bindings.FPDF_GetPageHeightF(page),
                )
            };
            rect = FS_RECTF {
                left: 0.0,
                top: height.max(1.0),
                right: width.max(1.0),
                bottom: 0.0,
            };
        }
        // SAFETY: the page is open.
        let rotation = unsafe { bindings.FPDFPage_GetRotation(page) }.rem_euclid(4);
        (rect, rotation)
    }

    /// How much of the page raster images cover, from 0 to 1.
    fn image_coverage(&self, page: FPDF_PAGE, frame: &FS_RECTF) -> f32 {
        let bindings = &self.pdfium.bindings;
        let mut grid = Coverage::new(frame);
        // SAFETY: the page is open.
        let count = unsafe { bindings.FPDFPage_CountObjects(page) }.max(0);
        for index in 0..count {
            // SAFETY: `index` is below the count PDFium just gave.
            let object = unsafe { bindings.FPDFPage_GetObject(page, index) };
            self.cover(object, &IDENTITY, 0, &mut grid);
        }
        grid.fraction()
    }

    /// Adds an object's images to the grid. `to_page` maps the space the object lives in, which
    /// is a form's when it is inside one, to the page's.
    fn cover(&self, object: FPDF_PAGEOBJECT, to_page: &FS_MATRIX, depth: u32, grid: &mut Coverage) {
        if object.is_null() || depth > MAX_FORM_DEPTH {
            return;
        }
        let bindings = &self.pdfium.bindings;
        // SAFETY: the object belongs to an open page.
        match unsafe { bindings.FPDFPageObj_GetType(object) } {
            FPDF_PAGEOBJ_IMAGE => {
                let (mut left, mut bottom, mut right, mut top) = (0.0, 0.0, 0.0, 0.0);
                // SAFETY: the object is valid and all four are valid out-pointers.
                let ok = unsafe {
                    bindings.FPDFPageObj_GetBounds(
                        object,
                        &mut left,
                        &mut bottom,
                        &mut right,
                        &mut top,
                    )
                };
                if ok != 0 {
                    grid.add(transform_rect(to_page, [left, bottom, right, top]));
                }
            }
            FPDF_PAGEOBJ_FORM => {
                let mut matrix = IDENTITY;
                // SAFETY: the object is valid and `matrix` is a valid out-pointer.
                if unsafe { bindings.FPDFPageObj_GetMatrix(object, &mut matrix) } == 0 {
                    return;
                }
                let inner = concat(to_page, &matrix);
                // SAFETY: the object is a form.
                let count = unsafe { bindings.FPDFFormObj_CountObjects(object) }.max(0);
                for index in 0..count {
                    // SAFETY: `index` is below the count PDFium just gave.
                    let child = unsafe { bindings.FPDFFormObj_GetObject(object, index as c_ulong) };
                    self.cover(child, &inner, depth + 1, grid);
                }
            }
            _ => {}
        }
    }

    /// Loads the page's text, runs `read` on it, and closes it again.
    fn with_text<T>(
        &self,
        page: FPDF_PAGE,
        read: impl FnOnce(FPDF_TEXTPAGE) -> T,
    ) -> Result<T, TileFailure> {
        let bindings = &self.pdfium.bindings;
        // SAFETY: the page is open.
        let text = unsafe { bindings.FPDFText_LoadPage(page) };
        if text.is_null() {
            return Err(TileFailure::PageLoad);
        }
        let result = read(text);
        // SAFETY: loaded above and used by nothing else.
        unsafe { bindings.FPDFText_ClosePage(text) };
        Ok(result)
    }

    fn measure(&self, page: FPDF_PAGE) -> Result<PageContent, TileFailure> {
        let (frame, _) = self.page_frame(page);
        let image_coverage = self.image_coverage(page, &frame);
        let bindings = &self.pdfium.bindings;
        let text_chars = self.with_text(page, |text| {
            // SAFETY: the text page is open.
            let count = unsafe { bindings.FPDFText_CountChars(text) }.max(0);
            (0..count)
                .filter_map(|index| {
                    // SAFETY: `index` is below the count PDFium just gave.
                    let code = unsafe { bindings.FPDFText_GetUnicode(text, index) };
                    char::from_u32(code)
                })
                .filter(|c| !c.is_whitespace() && !c.is_control())
                .count()
        })?;
        Ok(PageContent {
            text_chars: u32::try_from(text_chars).unwrap_or(u32::MAX),
            image_coverage,
        })
    }

    fn read_words(&self, page: FPDF_PAGE) -> Result<TextLayer, TileFailure> {
        let (frame, rotation) = self.page_frame(page);
        let bindings = &self.pdfium.bindings;
        self.with_text(page, |text| {
            let mut words = Vec::new();
            let mut units: Vec<u16> = Vec::new();
            let mut bounds: Option<[f32; 4]> = None;
            let finish =
                |words: &mut Vec<Word>, units: &mut Vec<u16>, bounds: &mut Option<[f32; 4]>| {
                    if let (false, Some(rect)) = (units.is_empty(), bounds.take()) {
                        words.push(Word {
                            text: String::from_utf16_lossy(units),
                            rect: to_shown(&frame, rotation, rect),
                            line_end: false,
                        });
                    }
                    units.clear();
                };
            // SAFETY: the text page is open.
            let count = unsafe { bindings.FPDFText_CountChars(text) }.max(0);
            for index in 0..count {
                // SAFETY: `index` is below the count PDFium just gave.
                let code = unsafe { bindings.FPDFText_GetUnicode(text, index) };
                let breaks_line = code == u32::from('\r') || code == u32::from('\n');
                let blank =
                    char::from_u32(code).is_some_and(|c| c.is_whitespace() || c.is_control());
                if blank || code == 0 {
                    finish(&mut words, &mut units, &mut bounds);
                    if breaks_line && let Some(last) = words.last_mut() {
                        last.line_end = true;
                    }
                    continue;
                }
                let mut rect = FS_RECTF {
                    left: 0.0,
                    top: 0.0,
                    right: 0.0,
                    bottom: 0.0,
                };
                // SAFETY: `index` is in range and `rect` is a valid out-pointer.
                if unsafe { bindings.FPDFText_GetLooseCharBox(text, index, &mut rect) } != 0 {
                    let char_box = [rect.left, rect.bottom, rect.right, rect.top];
                    bounds = Some(match bounds {
                        Some([l, b, r, t]) => [
                            l.min(char_box[0]),
                            b.min(char_box[1]),
                            r.max(char_box[2]),
                            t.max(char_box[3]),
                        ],
                        None => char_box,
                    });
                }
                // PDFium hands out UTF-16 units here, but a whole character would not fit one.
                match char::from_u32(code).filter(|_| code > 0xFFFF) {
                    Some(wide) => units.extend_from_slice(wide.encode_utf16(&mut [0; 2])),
                    None => units.push(code as u16),
                }
            }
            finish(&mut words, &mut units, &mut bounds);
            if let Some(last) = words.last_mut() {
                last.line_end = true;
            }
            TextLayer { words }
        })
    }
}

impl Engine for Document<'_, '_> {
    fn page_sizes(&self) -> &[PageGeometry] {
        &self.sizes
    }

    fn geometry_us(&self) -> u32 {
        self.geometry_us
    }

    fn hints(&mut self) -> Hints {
        // PDFium cannot say what the catalog asks for, so the bytes are read for it here, by safe
        // code with limits, inside the same sandbox.
        let hints = catalog_hints(self.data);
        Hints {
            page_layout: hints.page_layout.map_or(0, PageLayout::code),
            direction: hints.direction.map_or(0, Direction::code),
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
        // Pages can be enormous at a fine level of detail, but PDFium takes `int` sizes.
        let page_width = page_pixels(size.width_pt, tile.scale);
        let page_height = page_pixels(size.height_pt, tile.scale);
        let (Ok(size_x), Ok(size_y)) = (c_int::try_from(page_width), c_int::try_from(page_height))
        else {
            return Err(TileFailure::OutOfBounds);
        };

        if probe.is_cancelled() {
            return Ok(RenderOutcome::Cancelled);
        }
        let page = self.load(tile.page, !background)?;
        // A page load can take a long time, and the demand may have moved on since.
        if probe.is_cancelled() {
            self.done(&page);
            return Ok(RenderOutcome::Cancelled);
        }

        let tile_size = TILE_SIZE as c_int;
        let edge = tile_size + TILE_MARGIN + TILE_TRAILING_MARGIN;
        self.scratch.resize((edge * edge * 4) as usize, 0);
        let scratch = self.scratch.as_mut_ptr();
        let bindings = &self.pdfium.bindings;
        // SAFETY: `scratch` holds `edge` rows of `edge * 4` bytes and is not touched again until
        // the bitmap is destroyed below. PDFium never takes ownership of an external buffer.
        let bitmap = unsafe {
            bindings.FPDFBitmap_CreateEx(
                edge,
                edge,
                FPDF_BITMAP_BGRA,
                scratch.cast::<c_void>(),
                edge * 4,
            )
        };
        if bitmap.is_null() {
            self.done(&page);
            return Err(TileFailure::BitmapCreate);
        }

        // A tile is a window onto the whole page: rendering the full page size at a negative
        // offset lets PDFium clip to the bitmap, so tile edges line up exactly. The window starts
        // `TILE_MARGIN` pixels left of and above the tile.
        let start_x = TILE_MARGIN - c_int::from(tile.x) * tile_size;
        let start_y = TILE_MARGIN - c_int::from(tile.y) * tile_size;
        let mut pause = IFSDK_PAUSE {
            version: 1,
            NeedToPauseNow: Some(need_to_pause),
            user: ptr::from_ref(probe).cast_mut().cast::<c_void>(),
        };
        let flags = FPDF_ANNOT | FPDF_REVERSE_BYTE_ORDER;

        // SAFETY: bitmap and page are valid, and `pause` and `probe` outlive the render,
        // which ends at the `FPDF_RenderPage_Close` call below.
        let mut status = unsafe {
            let _ = bindings.FPDFBitmap_FillRect(bitmap, 0, 0, edge, edge, PAPER);
            bindings.FPDF_RenderPageBitmap_Start(
                bitmap,
                page.handle,
                start_x,
                start_y,
                size_x,
                size_y,
                0,
                flags,
                &mut pause,
            )
        };
        while status == FPDF_RENDER_TOBECONTINUED && !probe.is_cancelled() {
            // SAFETY: a render of this page is in progress, as `status` says.
            status = unsafe { bindings.FPDF_RenderPage_Continue(page.handle, &mut pause) };
        }
        // SAFETY: closing is documented as required after finishing or abandoning a render.
        // The bitmap is destroyed afterwards, so PDFium cannot touch the slot again.
        unsafe {
            bindings.FPDF_RenderPage_Close(page.handle);
            bindings.FPDFBitmap_Destroy(bitmap);
        }
        self.done(&page);

        match status {
            FPDF_RENDER_DONE => {
                let margin = TILE_MARGIN as usize;
                let (edge, row) = (edge as usize, TILE_STRIDE);
                for y in 0..TILE_SIZE as usize {
                    let from = ((margin + y) * edge + margin) * 4;
                    // SAFETY: row `margin + y` of the scratch holds `edge` pixels, of which the
                    // tile's `TILE_SIZE` start at `margin`; the caller guarantees the whole slot.
                    unsafe {
                        ptr::copy_nonoverlapping(
                            self.scratch.as_ptr().add(from),
                            target.add(y * row),
                            row,
                        );
                    }
                }
                Ok(RenderOutcome::Finished {
                    load_us: page.load_us,
                })
            }
            FPDF_RENDER_TOBECONTINUED => Ok(RenderOutcome::Cancelled),
            _ => Err(TileFailure::RenderFailed),
        }
    }

    fn analyze(&mut self, index: u32) -> Result<PageContent, TileFailure> {
        // Classifying a page is background work: it must not evict the pages being read.
        let page = self.load(index, false)?;
        let content = self.measure(page.handle);
        self.done(&page);
        content
    }

    fn text_layer(&mut self, index: u32) -> Result<TextLayer, TileFailure> {
        let page = self.load(index, false)?;
        let layer = self.read_words(page.handle);
        self.done(&page);
        layer
    }

    fn trim(&mut self) {
        self.release_pages();
    }
}

impl Drop for Document<'_, '_> {
    fn drop(&mut self) {
        // SAFETY: pages are closed before their document, and each only once. Nothing
        // uses these handles afterwards, because they are private to this struct.
        unsafe {
            for page in self.pages.drain(..) {
                self.pdfium.bindings.FPDF_ClosePage(page.handle);
            }
            self.pdfium.bindings.FPDF_CloseDocument(self.handle);
        }
    }
}

const IDENTITY: FS_MATRIX = FS_MATRIX {
    a: 1.0,
    b: 0.0,
    c: 0.0,
    d: 1.0,
    e: 0.0,
    f: 0.0,
};

/// `outer` after `inner`: maps what `inner` maps from, through both.
fn concat(outer: &FS_MATRIX, inner: &FS_MATRIX) -> FS_MATRIX {
    FS_MATRIX {
        a: outer.a * inner.a + outer.c * inner.b,
        b: outer.b * inner.a + outer.d * inner.b,
        c: outer.a * inner.c + outer.c * inner.d,
        d: outer.b * inner.c + outer.d * inner.d,
        e: outer.a * inner.e + outer.c * inner.f + outer.e,
        f: outer.b * inner.e + outer.d * inner.f + outer.f,
    }
}

/// The axis-aligned box around `[left, bottom, right, top]` once `matrix` maps it.
fn transform_rect(matrix: &FS_MATRIX, [left, bottom, right, top]: [f32; 4]) -> [f32; 4] {
    let corners = [(left, bottom), (left, top), (right, bottom), (right, top)].map(|(x, y)| {
        (
            matrix.a * x + matrix.c * y + matrix.e,
            matrix.b * x + matrix.d * y + matrix.f,
        )
    });
    let xs = corners.map(|(x, _)| x);
    let ys = corners.map(|(_, y)| y);
    [
        xs.into_iter().fold(f32::INFINITY, f32::min),
        ys.into_iter().fold(f32::INFINITY, f32::min),
        xs.into_iter().fold(f32::NEG_INFINITY, f32::max),
        ys.into_iter().fold(f32::NEG_INFINITY, f32::max),
    ]
}

/// A box in PDF space, `[left, bottom, right, top]`, as fractions of the page as it is shown:
/// `[left, top, right, bottom]` from its top-left corner, after `rotation` quarter turns clockwise.
fn to_shown(frame: &FS_RECTF, rotation: i32, [left, bottom, right, top]: [f32; 4]) -> [f32; 4] {
    let width = f64::from(frame.right - frame.left).max(f64::EPSILON);
    let height = f64::from(frame.top - frame.bottom).max(f64::EPSILON);
    let shown = |x: f32, y: f32| {
        let u = (f64::from(x) - f64::from(frame.left)) / width;
        let v = (f64::from(frame.top) - f64::from(y)) / height;
        match rotation {
            1 => (1.0 - v, u),
            2 => (1.0 - u, 1.0 - v),
            3 => (v, 1.0 - u),
            _ => (u, v),
        }
    };
    let (u0, v0) = shown(left, top);
    let (u1, v1) = shown(right, bottom);
    [
        u0.min(u1) as f32,
        v0.min(v1) as f32,
        u0.max(u1) as f32,
        v0.max(v1) as f32,
    ]
}

/// Which cells of a grid over the page some picture covers.
struct Coverage {
    frame: [f32; 4],
    cells: Vec<bool>,
}

impl Coverage {
    fn new(frame: &FS_RECTF) -> Self {
        Self {
            frame: [frame.left, frame.bottom, frame.right, frame.top],
            cells: vec![false; COVERAGE_GRID * COVERAGE_GRID],
        }
    }

    /// Marks every cell whose center lies in `[left, bottom, right, top]`.
    fn add(&mut self, [left, bottom, right, top]: [f32; 4]) {
        let [x0, y0, x1, y1] = self.frame;
        let (width, height) = (x1 - x0, y1 - y0);
        if !(width > 0.0 && height > 0.0) || !(left < right && bottom < top) {
            return;
        }
        let cell = |value: f32, origin: f32, extent: f32| {
            ((value - origin) / extent * COVERAGE_GRID as f32 - 0.5).ceil()
        };
        let first_x = cell(left, x0, width).clamp(0.0, COVERAGE_GRID as f32) as usize;
        let last_x = cell(right, x0, width).clamp(0.0, COVERAGE_GRID as f32) as usize;
        let first_y = cell(bottom, y0, height).clamp(0.0, COVERAGE_GRID as f32) as usize;
        let last_y = cell(top, y0, height).clamp(0.0, COVERAGE_GRID as f32) as usize;
        for row in first_y..last_y {
            self.cells[row * COVERAGE_GRID + first_x..row * COVERAGE_GRID + last_x].fill(true);
        }
    }

    fn fraction(&self) -> f32 {
        self.cells.iter().filter(|&&covered| covered).count() as f32 / self.cells.len() as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> FS_RECTF {
        FS_RECTF {
            left: 0.0,
            top: 800.0,
            right: 600.0,
            bottom: 0.0,
        }
    }

    #[test]
    fn coverage_counts_overlaps_once_and_ignores_what_is_off_the_page() {
        let mut grid = Coverage::new(&frame());
        grid.add([0.0, 0.0, 600.0, 400.0]);
        assert!((grid.fraction() - 0.5).abs() < 0.02);
        grid.add([0.0, 200.0, 600.0, 600.0]);
        assert!((grid.fraction() - 0.75).abs() < 0.02);
        grid.add([-1000.0, -1000.0, 0.0, 0.0]);
        assert!((grid.fraction() - 0.75).abs() < 0.02);
        grid.add([-1000.0, -1000.0, 5000.0, 5000.0]);
        assert_eq!(grid.fraction(), 1.0);
    }

    #[test]
    fn a_form_matrix_moves_what_is_inside_it() {
        // A form scaled by 2 and shifted right by 100.
        let form = FS_MATRIX {
            a: 2.0,
            b: 0.0,
            c: 0.0,
            d: 2.0,
            e: 100.0,
            f: 0.0,
        };
        let nested = concat(&form, &form);
        assert_eq!(
            transform_rect(&form, [0.0, 0.0, 10.0, 10.0]),
            [100.0, 0.0, 120.0, 20.0]
        );
        assert_eq!(
            transform_rect(&nested, [0.0, 0.0, 10.0, 10.0]),
            [300.0, 0.0, 340.0, 40.0]
        );
    }

    #[test]
    fn boxes_are_given_as_shown_after_rotation() {
        // The top-left tenth of the page, as stored.
        let corner = [0.0, 720.0, 60.0, 800.0];
        assert_eq!(to_shown(&frame(), 0, corner), [0.0, 0.0, 0.1, 0.1]);
        // A quarter turn clockwise takes the top-left corner to the top right.
        assert_eq!(to_shown(&frame(), 1, corner), [0.9, 0.0, 1.0, 0.1]);
        assert_eq!(to_shown(&frame(), 2, corner), [0.9, 0.9, 1.0, 1.0]);
        assert_eq!(to_shown(&frame(), 3, corner), [0.0, 0.9, 0.1, 1.0]);
    }
}

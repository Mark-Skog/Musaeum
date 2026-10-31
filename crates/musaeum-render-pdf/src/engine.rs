//! A small safe layer over the PDFium calls this worker needs.
//!
//! It uses `pdfium-render`'s raw bindings instead of its high-level API, because tile
//! rendering depends on two things that API does not offer: rendering into a caller's
//! buffer (the shared-memory slot) and the progressive renderer, whose pause callback
//! is the only way to stop a page that is already rendering.

use std::ffi::{c_int, c_void};
use std::marker::PhantomData;
use std::path::Path;
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use musaeum_shm::tile::{page_pixels, tile_in_page};
use musaeum_shm::wire::TileFailure;
use musaeum_shm::{PageGeometry, TILE_SIZE, TILE_STRIDE, TileKey};
use pdfium_render::prelude::{
    FPDF_BOOL, FPDF_DOCUMENT, FPDF_LIBRARY_CONFIG, FPDF_PAGE, FS_SIZEF, IFSDK_PAUSE,
    Pdfium as Binder, PdfiumError, PdfiumLibraryBindings,
};

// Values from `fpdfview.h` and `fpdf_progressive.h`.
const FPDF_ANNOT: c_int = 0x01;
const FPDF_REVERSE_BYTE_ORDER: c_int = 0x10;
const FPDF_BITMAP_BGRA: c_int = 4;
const FPDF_RENDER_TOBECONTINUED: c_int = 1;
const FPDF_RENDER_DONE: c_int = 2;

/// Opaque white, matching the compositor's paper plate so a tile replacing the plate
/// does not visibly change the page colour. Channel order is irrelevant for white.
const PAPER: u32 = 0xFFFF_FFFF;

/// Used when PDFium cannot report a page's size, so the layout stays stable and the
/// compositor shows a plate where the unreadable page would be.
const FALLBACK_SIZE: PageGeometry = PageGeometry {
    width_pt: 612.0,
    height_pt: 792.0,
};

/// Parsed pages kept open per worker. Consecutive tiles usually come from the same page,
/// and parsing a page's content is often far more expensive than rasterizing one tile.
const PAGE_CACHE_LEN: usize = 8;

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

/// Decides whether the current job should stop.
pub struct CancelProbe<'a> {
    cell: &'a AtomicU64,
    job_seq: u64,
}

impl<'a> CancelProbe<'a> {
    /// A probe for `job_seq`, watching the worker's cancel cell.
    pub fn new(cell: &'a AtomicU64, job_seq: u64) -> Self {
        Self { cell, job_seq }
    }

    fn is_cancelled(&self) -> bool {
        self.cell.load(Ordering::Acquire) >= self.job_seq
    }
}

/// PDFium calls this periodically during a progressive render.
unsafe extern "C" fn need_to_pause(this: *mut IFSDK_PAUSE) -> FPDF_BOOL {
    // SAFETY: PDFium passes back the structure built in `render_tile`, whose `user`
    // field points to a probe that outlives the whole render.
    let probe = unsafe { &*(*this).user.cast::<CancelProbe<'_>>() };
    FPDF_BOOL::from(probe.is_cancelled())
}

/// How a render ended, when it did not fail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderOutcome {
    /// The buffer holds the complete tile.
    Finished {
        /// Microseconds spent loading the page; 0 if it was cached.
        load_us: u32,
    },
    /// Cancelled; the buffer's contents are undefined.
    Cancelled,
}

struct CachedPage {
    index: u32,
    handle: FPDF_PAGE,
    last_used: u64,
}

/// An open PDF and its most recently used pages.
pub struct Document<'lib, 'data> {
    pdfium: &'lib Pdfium,
    handle: FPDF_DOCUMENT,
    sizes: Vec<PageGeometry>,
    geometry_us: u32,
    pages: Vec<CachedPage>,
    clock: u64,
    // PDFium reads from this memory for as long as the document is open.
    _data: PhantomData<&'data [u8]>,
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
            sizes,
            geometry_us: u32::try_from(started.elapsed().as_micros()).unwrap_or(u32::MAX),
            pages: Vec::with_capacity(PAGE_CACHE_LEN),
            clock: 0,
            _data: PhantomData,
        })
    }

    /// Pages in the document.
    pub fn page_count(&self) -> u32 {
        self.sizes.len() as u32
    }

    /// Every page's size, in page order.
    pub fn page_sizes(&self) -> &[PageGeometry] {
        &self.sizes
    }

    /// Microseconds spent reading all page sizes when the document opened.
    pub fn geometry_us(&self) -> u32 {
        self.geometry_us
    }

    /// Renders one tile into `target`, checking `probe` before and during the work.
    ///
    /// # Safety
    ///
    /// `target` must be valid for writes of one whole tile (`TILE_BYTES`) and nothing
    /// else may access that memory until this returns.
    pub unsafe fn render_tile(
        &mut self,
        target: *mut u8,
        tile: TileKey,
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
        let page_width = page_pixels(size.width_pt, tile.lod);
        let page_height = page_pixels(size.height_pt, tile.lod);
        let (Ok(size_x), Ok(size_y)) = (c_int::try_from(page_width), c_int::try_from(page_height))
        else {
            return Err(TileFailure::OutOfBounds);
        };

        if probe.is_cancelled() {
            return Ok(RenderOutcome::Cancelled);
        }
        let (page, load_us) = self.page(tile.page)?;
        // A page load can take a long time, and the demand may have moved on since.
        if probe.is_cancelled() {
            return Ok(RenderOutcome::Cancelled);
        }

        let bindings = &self.pdfium.bindings;
        let tile_size = TILE_SIZE as c_int;
        // SAFETY: the caller guarantees `target` holds a whole tile. PDFium never takes
        // ownership of an external buffer, so `FPDFBitmap_Destroy` below leaves it alone.
        let bitmap = unsafe {
            bindings.FPDFBitmap_CreateEx(
                tile_size,
                tile_size,
                FPDF_BITMAP_BGRA,
                target.cast::<c_void>(),
                TILE_STRIDE as c_int,
            )
        };
        if bitmap.is_null() {
            return Err(TileFailure::BitmapCreate);
        }

        // A tile is a window onto the whole page: rendering the full page size at a
        // negative offset lets PDFium clip to the bitmap, so tile edges line up exactly.
        let start_x = -(c_int::from(tile.x) * tile_size);
        let start_y = -(c_int::from(tile.y) * tile_size);
        let mut pause = IFSDK_PAUSE {
            version: 1,
            NeedToPauseNow: Some(need_to_pause),
            user: ptr::from_ref(probe).cast_mut().cast::<c_void>(),
        };
        let flags = FPDF_ANNOT | FPDF_REVERSE_BYTE_ORDER;

        // SAFETY: bitmap and page are valid, and `pause` and `probe` outlive the render,
        // which ends at the `FPDF_RenderPage_Close` call below.
        let mut status = unsafe {
            let _ = bindings.FPDFBitmap_FillRect(bitmap, 0, 0, tile_size, tile_size, PAPER);
            bindings.FPDF_RenderPageBitmap_Start(
                bitmap, page, start_x, start_y, size_x, size_y, 0, flags, &mut pause,
            )
        };
        while status == FPDF_RENDER_TOBECONTINUED && !probe.is_cancelled() {
            // SAFETY: a render of this page is in progress, as `status` says.
            status = unsafe { bindings.FPDF_RenderPage_Continue(page, &mut pause) };
        }
        // SAFETY: closing is documented as required after finishing or abandoning a render.
        // The bitmap is destroyed afterwards, so PDFium cannot touch the slot again.
        unsafe {
            bindings.FPDF_RenderPage_Close(page);
            bindings.FPDFBitmap_Destroy(bitmap);
        }

        match status {
            FPDF_RENDER_DONE => Ok(RenderOutcome::Finished { load_us }),
            FPDF_RENDER_TOBECONTINUED => Ok(RenderOutcome::Cancelled),
            _ => Err(TileFailure::RenderFailed),
        }
    }

    /// Returns a parsed page and the microseconds it took to load, using the cache.
    fn page(&mut self, index: u32) -> Result<(FPDF_PAGE, u32), TileFailure> {
        self.clock += 1;
        if let Some(cached) = self.pages.iter_mut().find(|page| page.index == index) {
            cached.last_used = self.clock;
            return Ok((cached.handle, 0));
        }

        let started = Instant::now();
        let page_index = c_int::try_from(index).map_err(|_| TileFailure::OutOfBounds)?;
        // SAFETY: the document is valid and PDFium checks the index itself.
        let handle = unsafe { self.pdfium.bindings.FPDF_LoadPage(self.handle, page_index) };
        if handle.is_null() {
            return Err(TileFailure::PageLoad);
        }
        let load_us = u32::try_from(started.elapsed().as_micros()).unwrap_or(u32::MAX);

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
        Ok((handle, load_us))
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

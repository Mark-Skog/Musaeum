//! Reads DjVu documents held in memory, with DjVuLibre linked in statically.
//!
//! A bridge written in C++ (`shim/`) turns DjVuLibre's message-driven API into calls that finish
//! before they return, so this side declares a few plain functions. Only bundled documents open:
//! an indirect one keeps its pages in other files, and the process reading it has no way, and no
//! right, to open them.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::marker::PhantomData;
use std::ptr::NonNull;

#[cfg(feature = "encode")]
pub mod encode;

#[allow(non_camel_case_types)]
#[repr(C)]
struct mdj_doc {
    _private: [u8; 0],
}

type WordCallback = unsafe extern "C" fn(*mut c_void, *const c_char, c_int, c_int, c_int, c_int);

unsafe extern "C" {
    fn mdj_open(data: *const u8, len: usize, why: *mut c_int) -> *mut mdj_doc;
    fn mdj_close(doc: *mut mdj_doc);
    fn mdj_page_count(doc: *mut mdj_doc) -> c_int;
    fn mdj_page_info(
        doc: *mut mdj_doc,
        page: c_int,
        width: *mut c_int,
        height: *mut c_int,
        dpi: *mut c_int,
        rotation: *mut c_int,
    ) -> c_int;
    fn mdj_render(
        doc: *mut mdj_doc,
        page: c_int,
        keep: c_int,
        page_width: c_int,
        page_height: c_int,
        x: c_int,
        y: c_int,
        w: c_int,
        h: c_int,
        rgba: *mut u8,
        stride: usize,
    ) -> c_int;
    fn mdj_text(doc: *mut mdj_doc, page: c_int, word: WordCallback, context: *mut c_void) -> c_int;
    fn mdj_trim(doc: *mut mdj_doc);
}

/// Why a document did not open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenError {
    /// DjVuLibre could not start.
    NoContext,
    /// Not DjVu, or damaged.
    Unreadable,
    /// An indirect document, whose pages are other files.
    Indirect,
}

impl OpenError {
    /// The detail word a worker reports to the host.
    pub fn code(self) -> u32 {
        match self {
            Self::NoContext => 1,
            Self::Unreadable => 2,
            Self::Indirect => 3,
        }
    }
}

/// A page's size as it is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageInfo {
    /// Width in pixels, with the page's rotation applied.
    pub width: u32,
    /// Height in pixels, with the page's rotation applied.
    pub height: u32,
    /// Resolution the page was scanned at.
    pub dpi: u32,
    /// Quarter turns counter-clockwise the page is shown at.
    pub rotation: u8,
}

/// Why a page could not be drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderError {
    /// The page's data could not be decoded.
    Decode,
    /// DjVuLibre failed while drawing it.
    Draw,
}

/// A word of a text layer, as DjVu stores it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawWord {
    /// The word.
    pub text: String,
    /// `[x0, y0, x1, y1]` in pixels of the page before rotation, from its bottom-left corner.
    pub rect: [i32; 4],
    /// Whether a line ends after it.
    pub line_end: bool,
}

/// An open document. DjVuLibre's context belongs to the thread that made it, so this is neither
/// `Send` nor `Sync`.
pub struct Document {
    raw: NonNull<mdj_doc>,
    _single_threaded: PhantomData<*const ()>,
}

impl Document {
    /// Opens a document from its bytes. DjVuLibre keeps a copy of them.
    pub fn open(data: &[u8]) -> Result<Self, OpenError> {
        let mut why = 0;
        // SAFETY: `data` is valid for its length during the call, and `why` is a valid
        // out-pointer. The bridge copies what it keeps.
        let raw = unsafe { mdj_open(data.as_ptr(), data.len(), &mut why) };
        match NonNull::new(raw) {
            Some(raw) => Ok(Self {
                raw,
                _single_threaded: PhantomData,
            }),
            None => Err(match why {
                3 => OpenError::Indirect,
                1 => OpenError::NoContext,
                _ => OpenError::Unreadable,
            }),
        }
    }

    /// Pages in the document.
    pub fn page_count(&self) -> u32 {
        // SAFETY: the document is open.
        u32::try_from(unsafe { mdj_page_count(self.raw.as_ptr()) }).unwrap_or(0)
    }

    /// A page's size, from its header alone.
    pub fn page_info(&mut self, page: u32) -> Option<PageInfo> {
        let page = c_int::try_from(page).ok()?;
        let (mut width, mut height, mut dpi, mut rotation) = (0, 0, 0, 0);
        // SAFETY: the document is open, and the four are valid out-pointers.
        let status = unsafe {
            mdj_page_info(
                self.raw.as_ptr(),
                page,
                &mut width,
                &mut height,
                &mut dpi,
                &mut rotation,
            )
        };
        if status != 0 || width <= 0 || height <= 0 {
            return None;
        }
        Some(PageInfo {
            width: width as u32,
            height: height as u32,
            dpi: u32::try_from(dpi).unwrap_or(0),
            rotation: (rotation & 3) as u8,
        })
    }

    /// Draws the part `(x, y, w, h)` of a page shown `page_size` pixels large into `rgba`, as RGBA
    /// rows of `stride` bytes. With `keep`, the decoded page stays for the next call.
    ///
    /// # Safety
    ///
    /// `rgba` must be valid for writes of `h` rows of `stride` bytes, with `stride` at least
    /// `w * 4`, and nothing else may access that memory until this returns.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn render(
        &mut self,
        page: u32,
        keep: bool,
        page_size: (u32, u32),
        (x, y, w, h): (u32, u32, u32, u32),
        rgba: *mut u8,
        stride: usize,
    ) -> Result<(), RenderError> {
        let int = |value: u32| c_int::try_from(value).map_err(|_| RenderError::Draw);
        // SAFETY: the document is open, and the caller guarantees the buffer.
        let status = unsafe {
            mdj_render(
                self.raw.as_ptr(),
                int(page)?,
                c_int::from(keep),
                int(page_size.0)?,
                int(page_size.1)?,
                int(x)?,
                int(y)?,
                int(w)?,
                int(h)?,
                rgba,
                stride,
            )
        };
        match status {
            0 => Ok(()),
            -1 => Err(RenderError::Decode),
            _ => Err(RenderError::Draw),
        }
    }

    /// A page's text layer, word by word. Empty if the page has none.
    pub fn words(&mut self, page: u32) -> Result<Vec<RawWord>, RenderError> {
        unsafe extern "C" fn collect(
            context: *mut c_void,
            text: *const c_char,
            x0: c_int,
            y0: c_int,
            x1: c_int,
            y1: c_int,
        ) {
            // SAFETY: the context is the vector passed to `mdj_text` below, alive for the call.
            let words = unsafe { &mut *context.cast::<Vec<RawWord>>() };
            if text.is_null() {
                if let Some(last) = words.last_mut() {
                    last.line_end = true;
                }
                return;
            }
            // SAFETY: DjVuLibre passes a NUL-terminated string that lives during the callback.
            let text = unsafe { CStr::from_ptr(text) }
                .to_string_lossy()
                .into_owned();
            words.push(RawWord {
                text,
                rect: [x0, y0, x1, y1],
                line_end: false,
            });
        }

        let page = c_int::try_from(page).map_err(|_| RenderError::Decode)?;
        let mut words: Vec<RawWord> = Vec::new();
        // SAFETY: the document is open, `collect` matches the callback type, and `words`
        // outlives the call, which makes every callback before returning.
        let status = unsafe {
            mdj_text(
                self.raw.as_ptr(),
                page,
                collect,
                (&raw mut words).cast::<c_void>(),
            )
        };
        if status != 0 {
            return Err(RenderError::Decode);
        }
        Ok(words)
    }

    /// Lets go of the decoded pages kept between renders.
    pub fn trim(&mut self) {
        // SAFETY: the document is open.
        unsafe { mdj_trim(self.raw.as_ptr()) };
    }
}

impl Drop for Document {
    fn drop(&mut self) {
        // SAFETY: opened by `mdj_open`, and closed once, here.
        unsafe { mdj_close(self.raw.as_ptr()) };
    }
}

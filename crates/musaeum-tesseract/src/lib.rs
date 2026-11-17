//! Tesseract 5 and Leptonica, compiled in, behind a small safe API: an [`Engine`] made from models
//! held in memory, which recognizes gray pictures into [`Word`]s with their boxes and confidence.
//!
//! Only the OCR worker links this crate. Tesseract reads hostile pixels, so it runs in that
//! sandboxed process and never in the host; nothing here opens a file, which the sandbox would
//! refuse anyway: the models arrive as bytes, and Tesseract's reader is served from them.

use std::ffi::{CString, c_char, c_int};
use std::ptr::NonNull;

/// The Tesseract this crate builds.
pub const TESSERACT_VERSION: &str = "5.5.4";

#[repr(C)]
struct RawEngine {
    _private: [u8; 0],
}

unsafe extern "C" {
    fn musaeum_ocr_add_model(language: *const c_char, data: *const u8, size: usize) -> c_int;
    fn musaeum_ocr_create(
        languages: *const c_char,
        error: *mut c_char,
        error_size: usize,
    ) -> *mut RawEngine;
    fn musaeum_ocr_destroy(engine: *mut RawEngine);
    fn musaeum_ocr_recognize(
        engine: *mut RawEngine,
        gray: *const u8,
        width: c_int,
        height: c_int,
        stride: c_int,
        psm: c_int,
        ppi: c_int,
        out: *mut *mut u8,
        out_size: *mut usize,
        error: *mut c_char,
        error_size: usize,
    ) -> c_int;
    fn musaeum_ocr_free(buffer: *mut u8);
}

/// How Tesseract looks for text on a picture: its page segmentation modes this program uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Segmentation {
    /// Finds the blocks and lines itself (`PSM_AUTO`).
    Auto,
    /// One block of text, lines found inside it (`PSM_SINGLE_BLOCK`).
    Block,
    /// One line of text (`PSM_SINGLE_LINE`).
    Line,
    /// As much text as it can find, in no particular order (`PSM_SPARSE_TEXT`).
    Sparse,
}

impl Segmentation {
    fn psm(self) -> c_int {
        match self {
            Self::Auto => 3,
            Self::Block => 6,
            Self::Line => 7,
            Self::Sparse => 11,
        }
    }
}

/// A word Tesseract read, in pixels of the picture it was given.
#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    pub text: String,
    /// Right and bottom exclusive.
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    /// From 0 to 100.
    pub confidence: f32,
    /// Counted from 0 across the picture, in reading order.
    pub block: u32,
    pub paragraph: u32,
    pub line: u32,
    /// The word's baseline, from one end to the other.
    pub baseline: [i32; 4],
}

/// A picture to read: one byte a pixel, 0 black.
#[derive(Clone, Copy, Debug)]
pub struct GrayImage<'a> {
    pub width: u32,
    pub height: u32,
    pub pixels: &'a [u8],
}

/// Keeps a copy of the model for `language` (`por`, `eng`), the bytes of its `.traineddata`, for
/// engines made after this.
pub fn add_model(language: &str, data: &[u8]) -> Result<(), String> {
    let name = CString::new(language).map_err(|_| "a language name has a NUL".to_owned())?;
    if data.is_empty() {
        return Err(format!("the model for {language} is empty"));
    }
    // SAFETY: both pointers are valid for the call; the shim copies the bytes and keeps nothing
    // of ours.
    let status = unsafe { musaeum_ocr_add_model(name.as_ptr(), data.as_ptr(), data.len()) };
    if status == 0 {
        Ok(())
    } else {
        Err(format!("the model for {language} could not be kept"))
    }
}

fn error_text(buffer: &[u8]) -> String {
    let end = buffer.iter().position(|&b| b == 0).unwrap_or(buffer.len());
    String::from_utf8_lossy(&buffer[..end]).into_owned()
}

/// A Tesseract ready to read, for one set of languages.
#[derive(Debug)]
pub struct Engine {
    raw: NonNull<RawEngine>,
}

// SAFETY: a TessBaseAPI may move between threads; it is only ever used by one at a time, which
// `&mut self` on `recognize` guarantees.
unsafe impl Send for Engine {}

impl Engine {
    /// An engine for `languages`, `+` between them, as Tesseract takes them: `por+eng`. Their
    /// models must have been given to [`add_model`].
    pub fn new(languages: &str) -> Result<Self, String> {
        let languages =
            CString::new(languages).map_err(|_| "a language name has a NUL".to_owned())?;
        let mut error = [0u8; 512];
        // SAFETY: the name and the error buffer are valid for the call, and the error buffer's
        // size is what is passed.
        let raw = unsafe {
            musaeum_ocr_create(languages.as_ptr(), error.as_mut_ptr().cast(), error.len())
        };
        NonNull::new(raw)
            .map(|raw| Self { raw })
            .ok_or_else(|| error_text(&error))
    }

    /// The words on `image`, read in mode `mode` at `ppi` pixels per inch.
    pub fn recognize(
        &mut self,
        image: GrayImage<'_>,
        mode: Segmentation,
        ppi: u32,
    ) -> Result<Vec<Word>, String> {
        let (width, height) = (image.width as usize, image.height as usize);
        if width == 0 || height == 0 || image.pixels.len() < width * height {
            return Err("the picture is empty or shorter than its size".to_owned());
        }
        if width > 32_000 || height > 32_000 {
            return Err("the picture is too large".to_owned());
        }
        let mut out: *mut u8 = std::ptr::null_mut();
        let mut out_size = 0usize;
        let mut error = [0u8; 512];
        // SAFETY: the engine is ours and used by this thread only; the pixels hold `width ×
        // height` bytes with rows `width` apart; every out-pointer is valid.
        let status = unsafe {
            musaeum_ocr_recognize(
                self.raw.as_ptr(),
                image.pixels.as_ptr(),
                width as c_int,
                height as c_int,
                width as c_int,
                mode.psm(),
                ppi.min(2400) as c_int,
                &mut out,
                &mut out_size,
                error.as_mut_ptr().cast(),
                error.len(),
            )
        };
        if status != 0 || out.is_null() {
            return Err(error_text(&error));
        }
        // SAFETY: on success the shim returned a buffer of `out_size` bytes it allocated, which is
        // ours until it is given back below.
        let bytes = unsafe { std::slice::from_raw_parts(out, out_size) }.to_vec();
        // SAFETY: the buffer came from the shim and is released once.
        unsafe { musaeum_ocr_free(out) };
        parse_words(&bytes)
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // SAFETY: the engine came from `musaeum_ocr_create` and is destroyed once.
        unsafe { musaeum_ocr_destroy(self.raw.as_ptr()) };
    }
}

/// The words of the shim's buffer.
fn parse_words(bytes: &[u8]) -> Result<Vec<Word>, String> {
    let mut at = 0usize;
    let mut take = |count: usize| -> Result<&[u8], String> {
        let end = at
            .checked_add(count)
            .filter(|&end| end <= bytes.len())
            .ok_or("the words are truncated")?;
        let slice = &bytes[at..end];
        at = end;
        Ok(slice)
    };
    let u32_of = |slice: &[u8]| u32::from_le_bytes(slice.try_into().expect("four bytes"));
    let count = u32_of(take(4)?) as usize;
    let mut words = Vec::with_capacity(count.min(100_000));
    for _ in 0..count {
        // Thirteen fields of four bytes: the box, the confidence, three counters, the baseline and the
        // length of the text.
        let fixed = take(4 * 13)?;
        let int = |index: usize| {
            i32::from_le_bytes(
                fixed[index * 4..index * 4 + 4]
                    .try_into()
                    .expect("four bytes"),
            )
        };
        let float = f32::from_le_bytes(fixed[16..20].try_into().expect("four bytes"));
        let length = u32_of(&fixed[48..52]) as usize;
        let text = String::from_utf8_lossy(take(length)?).into_owned();
        words.push(Word {
            text,
            left: int(0),
            top: int(1),
            right: int(2),
            bottom: int(3),
            confidence: if float.is_finite() { float } else { 0.0 },
            block: int(5) as u32,
            paragraph: int(6) as u32,
            line: int(7) as u32,
            baseline: [int(8), int(9), int(10), int(11)],
        });
    }
    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_buffer_of_words_is_read_back() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&1u32.to_le_bytes());
        for value in [10i32, 20, 60, 40] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&91.5f32.to_le_bytes());
        for value in [0u32, 0, 2] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for value in [10i32, 38, 60, 39] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        let text = "ação";
        bytes.extend_from_slice(&(text.len() as u32).to_le_bytes());
        bytes.extend_from_slice(text.as_bytes());
        let words = parse_words(&bytes).unwrap();
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].text, "ação");
        assert_eq!(words[0].line, 2);
        assert_eq!(words[0].baseline, [10, 38, 60, 39]);
        assert!(parse_words(&bytes[..bytes.len() - 1]).is_err());
    }
}

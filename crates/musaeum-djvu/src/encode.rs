//! Writes bundled DjVu documents with DjVuLibre's own encoders, for test fixtures.

use std::ffi::{CString, c_char, c_int};
use std::ptr;

#[repr(C)]
struct MdjWord {
    x0: c_int,
    y0: c_int,
    x1: c_int,
    y1: c_int,
    text: *const c_char,
    line_end: c_int,
}

#[repr(C)]
struct MdjPage {
    width: c_int,
    height: c_int,
    dpi: c_int,
    rotation: c_int,
    rgb: *const u8,
    mask: *const u8,
    words: *const MdjWord,
    word_count: c_int,
}

unsafe extern "C" {
    fn mdj_encode(
        pages: *const MdjPage,
        count: c_int,
        out: *mut *mut u8,
        out_len: *mut usize,
    ) -> c_int;
    fn mdj_free(buffer: *mut u8);
}

/// A word to put in a page's text layer.
#[derive(Clone, Debug)]
pub struct EncodeWord {
    /// `[x0, y0, x1, y1]` in pixels of the page as stored, from its top-left corner.
    pub rect: [i32; 4],
    /// The word.
    pub text: String,
    /// Whether a line ends after it.
    pub line_end: bool,
}

/// A page to encode.
#[derive(Clone, Debug, Default)]
pub struct EncodePage {
    /// Width in pixels, as stored.
    pub width: u32,
    /// Height in pixels, as stored.
    pub height: u32,
    /// Resolution.
    pub dpi: u32,
    /// Quarter turns counter-clockwise to show it at.
    pub rotation: u8,
    /// RGB pixels, rows from the top, for the color layer.
    pub rgb: Option<Vec<u8>>,
    /// One byte per pixel, non-zero where the black layer is, rows from the top.
    pub mask: Option<Vec<u8>>,
    /// The text layer.
    pub words: Vec<EncodeWord>,
}

/// Encodes `pages` as one bundled document.
pub fn encode(pages: &[EncodePage]) -> Result<Vec<u8>, String> {
    for (index, page) in pages.iter().enumerate() {
        let pixels = page.width as usize * page.height as usize;
        if page.rgb.as_ref().is_some_and(|rgb| rgb.len() != pixels * 3)
            || page.mask.as_ref().is_some_and(|mask| mask.len() != pixels)
        {
            return Err(format!("page {index}: layers do not match its size"));
        }
    }
    let texts: Vec<Vec<CString>> = pages
        .iter()
        .map(|page| {
            page.words
                .iter()
                .map(|word| CString::new(word.text.as_str()).map_err(|error| error.to_string()))
                .collect::<Result<_, _>>()
        })
        .collect::<Result<_, _>>()?;
    let words: Vec<Vec<MdjWord>> = pages
        .iter()
        .zip(&texts)
        .map(|(page, texts)| {
            page.words
                .iter()
                .zip(texts)
                .map(|(word, text)| MdjWord {
                    x0: word.rect[0],
                    y0: word.rect[1],
                    x1: word.rect[2],
                    y1: word.rect[3],
                    text: text.as_ptr(),
                    line_end: c_int::from(word.line_end),
                })
                .collect()
        })
        .collect();
    let raw: Vec<MdjPage> = pages
        .iter()
        .zip(&words)
        .map(|(page, words)| MdjPage {
            width: page.width as c_int,
            height: page.height as c_int,
            dpi: page.dpi as c_int,
            rotation: c_int::from(page.rotation),
            rgb: page.rgb.as_ref().map_or(ptr::null(), |rgb| rgb.as_ptr()),
            mask: page.mask.as_ref().map_or(ptr::null(), |mask| mask.as_ptr()),
            words: words.as_ptr(),
            word_count: words.len() as c_int,
        })
        .collect();

    let mut out = ptr::null_mut();
    let mut len = 0;
    // SAFETY: every pointer in `raw` points into vectors and strings that live until after the
    // call, with the lengths checked above; `out` and `len` are valid out-pointers.
    let status = unsafe { mdj_encode(raw.as_ptr(), raw.len() as c_int, &mut out, &mut len) };
    if status != 0 || out.is_null() {
        return Err("DjVuLibre could not encode the document".to_owned());
    }
    // SAFETY: the bridge returned a buffer of `len` bytes it allocated, freed right after.
    let bytes = unsafe { std::slice::from_raw_parts(out, len) }.to_vec();
    // SAFETY: allocated by `mdj_encode`, and freed once.
    unsafe { mdj_free(out) };
    Ok(bytes)
}

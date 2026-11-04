//! A DjVu book of every shape, written with DjVuLibre's own encoders.
//!
//! Like the comics, every page's color layer is four quadrants, red, green, blue and yellow from
//! the top left of the page as stored, so a test can tell which way up it came out. Pages with a
//! black layer carry three lines of word-shaped bars, and pages with a text layer name those same
//! bars as words, so where a word's box says it is, ink should be.

use std::io;
use std::path::Path;

use musaeum_djvu::encode::{EncodePage, EncodeWord, encode};

use crate::CONTENT_SENTENCE;
use crate::comics::QUADRANTS;

/// One page of the DjVu fixture, as stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DjvuPage {
    /// Width in pixels, before rotation.
    pub width: u32,
    /// Height in pixels, before rotation.
    pub height: u32,
    /// Resolution.
    pub dpi: u32,
    /// Quarter turns counter-clockwise it is shown at.
    pub rotation: u8,
    /// Whether it has the color layer of quadrants.
    pub background: bool,
    /// Whether it has the black layer of word bars.
    pub mask: bool,
    /// Whether it has a text layer naming the bars.
    pub text: bool,
    /// What a classifier should call it.
    pub class: &'static str,
}

const fn page(
    (width, height, dpi, rotation): (u32, u32, u32, u8),
    (background, mask, text): (bool, bool, bool),
    class: &'static str,
) -> DjvuPage {
    DjvuPage {
        width,
        height,
        dpi,
        rotation,
        background,
        mask,
        text,
        class,
    }
}

/// The pages, in order.
///
/// | Page | Shown | Layers | Class |
/// |------|-------|--------|-------|
/// | 0 | 600 × 800 at 100 dpi | color, black, text | hybrid |
/// | 1 | 800 × 600 at 100 dpi | color | scanned |
/// | 2 | 800 × 600: 600 × 800 turned a quarter counter-clockwise | color, black, text | hybrid |
/// | 3 | 700 × 700 at 100 dpi | black | scanned |
/// | 4 | 600 × 800 at 200 dpi, half the size of page 0 | color, black | scanned |
pub const DJVU_PAGES: [DjvuPage; 5] = [
    page((600, 800, 100, 0), (true, true, true), "hybrid"),
    page((800, 600, 100, 0), (true, false, false), "scanned"),
    page((600, 800, 100, 1), (true, true, true), "hybrid"),
    page((700, 700, 100, 0), (false, true, false), "scanned"),
    page((600, 800, 200, 0), (true, true, false), "scanned"),
];

/// Lines of words drawn on a page with a black layer.
pub const DJVU_LINES: u32 = 3;

impl DjvuPage {
    /// Size as shown, in pixels.
    pub fn shown(&self) -> (u32, u32) {
        if self.rotation & 1 == 1 {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }

    /// The pixel of the page as stored that shows at `(x, y)`.
    pub fn stored_point(&self, x: u32, y: u32) -> (u32, u32) {
        let (w, h) = (self.width, self.height);
        match self.rotation & 3 {
            // A quarter turn counter-clockwise takes the stored top-right corner to the top left.
            1 => (w - 1 - y, x),
            2 => (w - 1 - x, h - 1 - y),
            3 => (y, h - 1 - x),
            _ => (x, y),
        }
    }

    /// The color layer at a stored pixel.
    pub fn quadrant(&self, x: u32, y: u32) -> [u8; 3] {
        let right = usize::from(x >= self.width / 2);
        let bottom = usize::from(y >= self.height / 2);
        QUADRANTS[bottom * 2 + right]
    }

    /// The word bars, `[x0, y0, x1, y1]` in stored pixels from the top left, with their words.
    pub fn words(&self) -> Vec<EncodeWord> {
        let mut words = Vec::new();
        for line in 0..DJVU_LINES as i32 {
            let top = 60 + line * 40;
            let mut x = 50;
            let sentence: Vec<&str> = CONTENT_SENTENCE.split(' ').collect();
            for (index, word) in sentence.iter().enumerate() {
                let width = word.len() as i32 * 10;
                words.push(EncodeWord {
                    rect: [x, top, x + width, top + 24],
                    text: (*word).to_owned(),
                    line_end: index == sentence.len() - 1,
                });
                x += width + 10;
            }
        }
        words
    }

    fn encoded(&self) -> EncodePage {
        let (w, h) = (self.width as usize, self.height as usize);
        let rgb = self.background.then(|| {
            let mut rgb = Vec::with_capacity(w * h * 3);
            for y in 0..self.height {
                for x in 0..self.width {
                    rgb.extend_from_slice(&self.quadrant(x, y));
                }
            }
            rgb
        });
        let words = self.words();
        let mask = self.mask.then(|| {
            let mut mask = vec![0u8; w * h];
            for word in &words {
                let [x0, y0, x1, y1] = word.rect.map(|value| value as usize);
                for row in mask[y0 * w..y1 * w].chunks_exact_mut(w) {
                    row[x0..x1].fill(1);
                }
            }
            mask
        });
        EncodePage {
            width: self.width,
            height: self.height,
            dpi: self.dpi,
            rotation: self.rotation,
            rgb,
            mask,
            words: if self.text { words } else { Vec::new() },
        }
    }
}

/// The fixture's bytes.
pub fn djvu_bytes() -> io::Result<Vec<u8>> {
    let pages: Vec<EncodePage> = DJVU_PAGES.iter().map(DjvuPage::encoded).collect();
    encode(&pages).map_err(io::Error::other)
}

/// Writes the fixture to `path`.
pub fn write_djvu_fixture(path: &Path) -> io::Result<()> {
    std::fs::write(path, djvu_bytes()?)
}

//! Finds the lines of text on a picture of a page, for selecting and marking on scanned books.
//!
//! Classical computer vision, with nothing learned: the page is binarized by Sauvola's adaptive
//! threshold over an integral image, its ink labeled into connected components and sorted into
//! glyphs, specks, figures and rules, its skew estimated from where the glyphs' bottoms line up
//! best, its columns found at the gutters, and its lines found strip by strip and linked into
//! baselines that bend where the paper does. The result is a [`LineModel`]: polylines in
//! fractions of the page's crop box, as the character quads of the text layer are, small enough
//! to keep in `cache.db` and to hit-test in under a millisecond.
//!
//! Every step takes a gray picture about 1500 pixels tall, which the render workers draw in the
//! background; nothing here reads a file.

pub mod components;
pub mod detect;
pub mod gray;
pub mod hash;
pub mod lines;
pub mod model;
pub mod near;
pub mod orient;
pub mod picture;
pub mod sauvola;
pub mod score;
pub mod skew;
pub mod synth;
pub mod verify;

pub use detect::{DetectOptions, Detection, Stats, detect, to_shown, to_stored};
pub use gray::{Gray, Integral};
pub use hash::{dhash, hamming};
pub use model::{
    Column, LINE_TURNED, LINES_MAGIC, LINES_VERSION, LineModel, MAX_COLUMNS, ModelError, VisualLine,
};
pub use sauvola::{Bitmap, SauvolaOptions};

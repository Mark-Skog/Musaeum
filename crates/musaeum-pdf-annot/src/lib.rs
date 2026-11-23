//! A PDF's own annotations, in and out of Musaeum, without ever writing over a book.
//!
//! - [`document`] walks a PDF's structure as a reader does, and [`syntax`] reads its objects with
//!   where each one is, so a page can be written again as its own bytes with one entry changed.
//! - [`read`] reads the annotations a PDF holds, and [`write`] writes Musaeum's as an incremental
//!   update: bytes to put after the original's, which stay as they are. The host copies the book,
//!   adds them to the copy and renames it into place; the book is only ever read.
//! - [`xfdf`] reads and writes the XML Acrobat exchanges annotations in, strictly.
//! - [`protocol`] and [`client`] are the conversation with the worker that does all of this, in a
//!   sandbox, since every byte of a PDF or an XFDF file is hostile input; it also checks a written
//!   copy with PDFium.
//!
//! Places are fractions of a page's frame before its rotation ([`model::Frame`]), as the render
//! worker measures characters and the library keeps annotations.

pub mod client;
pub mod document;
pub mod model;
pub mod protocol;
pub mod read;
pub mod syntax;
pub mod text;
pub mod write;
pub mod xfdf;

pub use document::{PdfError, PdfFile};
pub use model::{Annot, Frame, Subtype};

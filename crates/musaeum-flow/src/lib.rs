//! Flowing books (EPUB, MOBI, AZW3, FB2, TXT and Markdown) in one safe, deterministic shape, and
//! positions in them that survive a change of font, of mode and of reader.
//!
//! Everything here reads hostile bytes, so it is all safe Rust, every size it reads is limited, and
//! it runs in the isolated `ingest` process. The output is an EPUB 3 package that a reader can show
//! from a separate origin: no scripts, no event handlers, nothing that reaches the network, sections
//! small enough to lay out in time, and the same bytes for the same input every time.

#![forbid(unsafe_code)]

pub mod cfi;
pub mod css;
pub mod dom;
pub mod encoding;
mod entities;
pub mod formats;
pub mod html;
pub mod locator;
pub mod normalize;
pub mod package;
pub mod sanitize;
pub mod serialize;
pub mod source;
pub mod split;
pub mod url;
pub mod xml;
mod zipwrite;

/// The version of everything that decides the normalized form. A CFI or a text offset is only
/// exact for the version that produced it, so every Locator carries it, and a new version means
/// positions are found again by their quoted text.
pub const NORMALIZER_VERSION: u32 = 1;

/// What a book says about itself, read without converting it: the headers of a Mobipocket file,
/// the description of a FictionBook, the front matter of Markdown. EPUB and plain text have their
/// own readers elsewhere, or nothing to say.
pub fn metadata(
    format: package::SourceFormat,
    bytes: &[u8],
    limits: &source::Limits,
) -> source::Result<package::Metadata> {
    use package::SourceFormat;
    match format {
        SourceFormat::Mobi | SourceFormat::Azw3 => formats::mobi::read_metadata(bytes),
        SourceFormat::Fb2 => formats::fb2::read_metadata(bytes, limits),
        SourceFormat::Markdown => Ok(formats::markdown::read(bytes, limits)?.metadata),
        SourceFormat::Epub | SourceFormat::Txt => Ok(package::Metadata::default()),
    }
}

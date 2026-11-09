//! A book as a format reader leaves it: documents parsed but not yet made safe, files, navigation.
//! Every format reader produces this, and [`crate::normalize`] makes the normalized book from it.

use thiserror::Error;

use crate::dom::Dom;
use crate::package::{Metadata, SourceFormat};

/// Why a book could not be normalized.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum NormalizeError {
    #[error("not a {0} file")]
    NotThisFormat(&'static str),
    #[error("damaged: {0}")]
    Malformed(String),
    #[error("too large: {0}")]
    TooLarge(String),
    #[error("protected by DRM")]
    Drm,
    #[error("not supported: {0}")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, NormalizeError>;

/// Limits on what one book may make the normalizer hold.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Files in an archive.
    pub max_files: usize,
    /// One file, unpacked.
    pub max_file_bytes: usize,
    /// All files, unpacked, together.
    pub max_total_bytes: usize,
    /// Nodes in one document.
    pub max_nodes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_files: 50_000,
            max_file_bytes: 256 << 20,
            max_total_bytes: 1 << 30,
            max_nodes: 2_000_000,
        }
    }
}

/// How a document was read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Parsed {
    Xml,
    Html,
    /// Made by the normalizer: a picture wrapped in a page, or a page standing in for a document
    /// that could not be read.
    Generated,
}

impl Parsed {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Xml => "xml",
            Self::Html => "html",
            Self::Generated => "generated",
        }
    }
}

/// One document of the reading order.
#[derive(Debug)]
pub struct SourceDocument {
    /// Its path in the source, which relative references in it start from.
    pub path: String,
    /// The `id` of its spine `itemref`.
    pub id: Option<String>,
    pub linear: bool,
    pub properties: Vec<String>,
    pub dom: Dom,
    pub parsed: Parsed,
    /// `true` for an SVG document, `false` for XHTML.
    pub svg: bool,
}

/// A file other than a document.
#[derive(Debug)]
pub struct SourceResource {
    pub path: String,
    pub media_type: String,
    pub data: Vec<u8>,
}

/// An entry of the table of contents, the page list or the landmarks.
#[derive(Clone, Debug, Default)]
pub struct SourceNav {
    pub label: String,
    /// A path in the source and a fragment.
    pub target: Option<(String, Option<String>)>,
    pub kind: Option<String>,
    pub children: Vec<SourceNav>,
}

/// A whole book as a format reader leaves it.
#[derive(Debug)]
pub struct SourceBook {
    pub format: SourceFormat,
    pub version: Option<String>,
    pub fixed_layout: bool,
    pub direction: Option<String>,
    /// The cover is a path in the source here.
    pub metadata: Metadata,
    pub cover: Option<String>,
    pub documents: Vec<SourceDocument>,
    pub resources: Vec<SourceResource>,
    pub toc: Vec<SourceNav>,
    pub page_list: Vec<SourceNav>,
    pub landmarks: Vec<SourceNav>,
    /// Files of the source left out on purpose.
    pub dropped_files: u32,
    pub warnings: Vec<String>,
}

impl SourceBook {
    pub fn new(format: SourceFormat) -> Self {
        Self {
            format,
            version: None,
            fixed_layout: false,
            direction: None,
            metadata: Metadata::default(),
            cover: None,
            documents: Vec::new(),
            resources: Vec::new(),
            toc: Vec::new(),
            page_list: Vec::new(),
            landmarks: Vec::new(),
            dropped_files: 0,
            warnings: Vec::new(),
        }
    }
}

/// White space folded to single spaces, ends trimmed: how labels and metadata are kept.
pub fn fold_space(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `text` folded and cut to at most `max` characters, or `None` if nothing is left.
pub fn clean_field(text: &str, max: usize) -> Option<String> {
    let folded: String = fold_space(text)
        .chars()
        .filter(|c| !c.is_control())
        .take(max)
        .collect();
    (!folded.is_empty()).then_some(folded)
}

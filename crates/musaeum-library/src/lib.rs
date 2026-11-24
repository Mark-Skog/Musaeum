//! The library without an interface: what is on the disk, what it is, and what a person did with it.
//!
//! - [`Library`] holds `library.db`, which cannot be replaced, and `cache.db`, which can.
//! - [`identity`] recognizes a file through renames, moves and edits, and the same bytes anywhere.
//! - [`Indexer`] keeps all that current, while the rest of the program does something else.
//! - Everything the library does to the disk goes through a journal, with a way back.
//!
//! All the unsafe code this depends on is in `musaeum-fs`, and this crate has none of its own.

#![forbid(unsafe_code)]

pub mod anchoring;
pub mod annotations;
mod batch;
mod cache_report;
pub mod calibre;
pub mod catalog;
mod content;
mod content_kind;
mod db;
mod error;
pub mod exchange;
mod fingerprint;
pub mod flow;
mod fulltext;
pub mod identity;
mod indexer;
mod ingest;
mod journal;
mod library;
mod lines;
mod merge;
mod metadata;
mod model;
pub mod order;
pub mod preferences;
mod probe;
mod reading;
mod reconcile;
mod scan;
mod search;
pub mod smart;
mod sql;
mod taxonomy;
mod thumbnails;

pub use annotations::{
    AnchorState, Annotation, AnnotationEdit, AnnotationHit, AnnotationKind, ImportOutcome,
    ImportedAnnotation, NewAnnotation, QuoteSource, Reanchoring, Segment, SegmentUpdate, Selector,
    SelectorKind,
};
pub use batch::{BatchEdit, BatchSummary, MAX_BATCH, Shared};
pub use cache_report::CacheReport;
pub use calibre::{
    CalibreBook, CalibreColumn, CalibreFile, CalibreLibrary, CalibreReport, read_calibre,
};
pub use catalog::{BookFile, BookRow, BrowsePage, BrowseQuery, FolderEntry, MAX_BROWSE};
pub use content::ContentReport;
pub use db::backup::{BackupInfo, BackupPolicy};
pub use error::{Error, Result};
pub use exchange::{
    ExchangeExport, ExchangeImport, ExchangeScope, ExchangeTarget, ImportedInto, MAX_JSONLD_BYTES,
    UnmatchedWork,
};
pub use fingerprint::Hash;
pub use flow::FlowStore;
pub use fulltext::{
    BookHit, FULLTEXT_VERSION, FullTextResults, FullTextStatus, MAX_BOOKS_FOUND, PendingText,
    TextSource, TextUnit, UnitHit, UnitKind, fulltext_dir,
};
pub use identity::ReconcileReport;
pub use indexer::{Indexer, IndexerConfig, IndexerEvent};
pub use ingest::{
    CoverOutcome, ImageKind, IngestClient, IngestConfig, NormalizeOutcome, NormalizedFiles,
    check_normalized,
};
pub use journal::RecoveryReport;
pub use library::{Library, LibraryOptions, LibraryPaths};
pub use lines::{
    LineModelRecord, MAX_LINE_MODEL_BYTES, MAX_OCR_LINE_BYTES, OcrLineRecord, OcrReading,
};
pub use merge::{DuplicatePair, DuplicateReason, MAX_DUPLICATES, Review, SuggestionReview};
pub use metadata::{
    Credit, CustomValue, Identifier, ROLES, SCHEMES, WorkEdit, WorkMetadata, isbn, language_tag,
};
pub use model::{
    AnnotationId, AnnotationInfo, DocumentId, DocumentInfo, FolderId, InstanceId, InstanceInfo,
    InstanceState, LibraryStatus, OperationId, OperationInfo, RevisionId, RevisionInfo,
    RevisionOrigin, RootId, RootInfo, SuggestionId, SuggestionInfo, SuggestionKind, WorkId,
    WorkInfo,
};
pub use preferences::{Motion, Preferences, PreferencesPatch, PreferencesStore, Theme};
pub use probe::{FsProbe, RealFileSystem};
pub use reading::{
    CONTENT_MAJORITY_PERCENT, CONTENT_RULES_VERSION, DocumentContent, DocumentGeometry,
    FlowReadingState, OpenedFile, PageContentClass, PageContentRecord, PageOrientation, PageRecord,
    ReadingMode, ReadingState, Spreads, content_summary,
};
pub use reconcile::ReconcileScope;
pub use search::{DEFAULT_SEARCH_LIMIT, MatchKind, SearchHit};
pub use smart::{Match as SmartMatch, Rule as SmartRule, Rules as SmartRules};
pub use taxonomy::{
    Category, CategoryId, Collection, CollectionId, CustomField, CustomFieldId, FIELD_KINDS,
    MAX_CATEGORY_DEPTH, Place, SmartCollection, SmartCollectionId, TAG_COLORS, Tag, TagId,
};
pub use thumbnails::ThumbnailStore;

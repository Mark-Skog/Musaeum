//! The library without an interface: what is on the disk, what it is, and what a person did with it.
//!
//! - [`Library`] holds `library.db`, which cannot be replaced, and `cache.db`, which can.
//! - [`identity`] recognizes a file through renames, moves and edits, and the same bytes anywhere.
//! - [`Indexer`] keeps all that current, while the rest of the program does something else.
//! - Everything the library does to the disk goes through a journal, with a way back.
//!
//! All the unsafe code this depends on is in `musaeum-fs`, and this crate has none of its own.

#![forbid(unsafe_code)]

mod content;
mod content_kind;
mod db;
mod error;
mod fingerprint;
pub mod flow;
pub mod identity;
mod indexer;
mod ingest;
mod journal;
mod library;
mod model;
mod probe;
mod reading;
mod reconcile;
mod scan;
mod search;
mod sql;
mod thumbnails;

pub use content::ContentReport;
pub use db::backup::{BackupInfo, BackupPolicy};
pub use error::{Error, Result};
pub use fingerprint::Hash;
pub use flow::FlowStore;
pub use identity::ReconcileReport;
pub use indexer::{Indexer, IndexerConfig, IndexerEvent};
pub use ingest::{IngestClient, IngestConfig, NormalizeOutcome, NormalizedFiles, check_normalized};
pub use journal::RecoveryReport;
pub use library::{Library, LibraryOptions, LibraryPaths};
pub use model::{
    AnnotationId, AnnotationInfo, DocumentId, DocumentInfo, FolderId, InstanceId, InstanceInfo,
    InstanceState, LibraryStatus, OperationId, OperationInfo, RevisionId, RevisionInfo,
    RevisionOrigin, RootId, RootInfo, SuggestionId, SuggestionInfo, SuggestionKind, WorkEdit,
    WorkId, WorkInfo,
};
pub use probe::{FsProbe, RealFileSystem};
pub use reading::{
    CONTENT_MAJORITY_PERCENT, CONTENT_RULES_VERSION, DocumentContent, DocumentGeometry, OpenedFile,
    PageContentClass, PageContentRecord, PageOrientation, PageRecord, ReadingMode, ReadingState,
    Spreads, content_summary,
};
pub use reconcile::ReconcileScope;
pub use search::{DEFAULT_SEARCH_LIMIT, MatchKind, SearchHit};
pub use thumbnails::ThumbnailStore;

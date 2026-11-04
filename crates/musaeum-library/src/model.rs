//! The library's vocabulary, as the rest of the program sees it.

use std::path::{Path, PathBuf};

use rusqlite::ToSql;
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSqlOutput, ValueRef};
use serde::{Serialize, Serializer};

/// A monitored folder.
pub type RootId = i64;
/// A directory in a root.
pub type FolderId = i64;
/// A work the user organizes.
pub type WorkId = i64;
/// An edition of a work, in one format.
pub type DocumentId = i64;
/// One exact sequence of bytes of a document.
pub type RevisionId = i64;
/// One place where a revision's bytes are.
pub type InstanceId = i64;
/// A note on a document.
pub type AnnotationId = i64;
/// Something the library noticed and did not act on.
pub type SuggestionId = i64;
/// An entry in the journal of disk operations.
pub type OperationId = i64;

/// What is known about where a file instance is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceState {
    /// Its bytes are here.
    Present,
    /// It is not where it was last seen. Everything about it stays, until it turns up again.
    Absent,
    /// It is here, and its bytes are in the cloud. Reading it would start a download.
    CloudOnly,
    /// It is here, and could not be read last time it was tried.
    Unreadable,
}

impl InstanceState {
    /// How the database writes this.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Present => "present",
            Self::Absent => "absent",
            Self::CloudOnly => "cloud_only",
            Self::Unreadable => "unreadable",
        }
    }
}

impl std::fmt::Display for InstanceState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl ToSql for InstanceState {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(self.as_str()))
    }
}

impl FromSql for InstanceState {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        match value.as_str()? {
            "present" => Ok(Self::Present),
            "absent" => Ok(Self::Absent),
            "cloud_only" => Ok(Self::CloudOnly),
            "unreadable" => Ok(Self::Unreadable),
            _ => Err(FromSqlError::InvalidType),
        }
    }
}

/// How a revision came to exist.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RevisionOrigin {
    /// The file was found, and it was new.
    Import,
    /// The same file, in the same place, has new contents.
    ChangedInPlace,
    /// A different file took the place of this one, which is what a safe save looks like.
    ReplacedAtPath,
}

impl RevisionOrigin {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Import => "import",
            Self::ChangedInPlace => "changed_in_place",
            Self::ReplacedAtPath => "replaced_at_path",
        }
    }
}

impl FromSql for RevisionOrigin {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        match value.as_str()? {
            "import" => Ok(Self::Import),
            "changed_in_place" => Ok(Self::ChangedInPlace),
            "replaced_at_path" => Ok(Self::ReplacedAtPath),
            _ => Err(FromSqlError::InvalidType),
        }
    }
}

/// What makes the library suggest that two documents may be one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionKind {
    /// The files have different bytes, and the same `/ID[0]` or `dc:identifier`.
    SameInternalId,
    /// A file with a known File ID appeared elsewhere, with different contents. IDs get reused, so
    /// this alone concludes nothing.
    SameFileId,
    /// A file appeared where one that went missing used to be.
    SamePath,
    /// The same fast fingerprint as a file that went missing before it could be fully hashed, which
    /// makes it impossible to confirm.
    SameFingerprint,
}

impl SuggestionKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::SameInternalId => "same_internal_id",
            Self::SameFileId => "same_file_id",
            Self::SamePath => "same_path",
            Self::SameFingerprint => "same_fingerprint",
        }
    }
}

impl FromSql for SuggestionKind {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        match value.as_str()? {
            "same_internal_id" => Ok(Self::SameInternalId),
            "same_file_id" => Ok(Self::SameFileId),
            "same_path" => Ok(Self::SamePath),
            "same_fingerprint" => Ok(Self::SameFingerprint),
            _ => Err(FromSqlError::InvalidType),
        }
    }
}

fn path_as_text<S: Serializer>(path: &Path, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&path.to_string_lossy())
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
}

/// Bytes as lowercase hexadecimal, as they are shown to people.
pub(crate) fn hex_of(bytes: Option<Vec<u8>>) -> Option<String> {
    bytes.map(|bytes| hex(&bytes))
}

/// A monitored folder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RootInfo {
    /// Its ID.
    pub id: RootId,
    /// Where it is.
    #[serde(serialize_with = "path_as_text")]
    pub path: PathBuf,
    /// How the library follows it.
    pub policy: String,
    /// Name patterns it leaves out, besides the ones always left out.
    pub exclusions: Vec<String>,
    /// `available`, `unavailable` or `removed`.
    pub status: String,
    /// When it was last scanned from top to bottom, in milliseconds since the Unix epoch.
    pub last_reconciled_at: Option<i64>,
}

/// One place where a revision's bytes are, or were.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct InstanceInfo {
    /// Its ID.
    pub id: InstanceId,
    /// The revision it holds.
    pub revision_id: RevisionId,
    /// That revision's document.
    pub document_id: DocumentId,
    /// That document's work.
    pub work_id: WorkId,
    /// The folder it is in.
    pub folder_id: FolderId,
    /// Where it is, or for an absent file, the last place it was seen.
    #[serde(serialize_with = "path_as_text")]
    pub path: PathBuf,
    /// What is known about it.
    pub state: InstanceState,
    /// Its File ID, in hexadecimal.
    pub file_id: Option<String>,
    /// When its contents last changed, in nanoseconds since the Unix epoch.
    pub mtime_ns: i64,
}

/// One exact sequence of bytes of a document.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RevisionInfo {
    /// Its ID.
    pub id: RevisionId,
    /// Its document.
    pub document_id: DocumentId,
    /// The BLAKE3 of its bytes, in hexadecimal, or `None` while it is unconfirmed.
    pub blake3: Option<String>,
    /// Its fast fingerprint, in hexadecimal, or `None` if its bytes have not been available.
    pub fast_fingerprint: Option<String>,
    /// Its length in bytes.
    pub size: u64,
    /// The revision this one replaced, which is where re-anchoring starts from.
    pub previous_revision_id: Option<RevisionId>,
    /// How it came to exist.
    pub origin: RevisionOrigin,
    /// Every place its bytes are or were.
    pub instances: Vec<InstanceInfo>,
}

/// An edition of a work, in one format.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DocumentInfo {
    /// Its ID.
    pub id: DocumentId,
    /// Its work.
    pub work_id: WorkId,
    /// That work's title.
    pub title: String,
    /// Its format, by file extension.
    pub format: String,
    /// `fixed` or `flow`.
    pub layout: String,
    /// The PDF's `/ID[0]`.
    pub pdf_id0: Option<String>,
    /// The EPUB's `dc:identifier`.
    pub dc_identifier: Option<String>,
    /// Its ISBN.
    pub isbn: Option<String>,
    /// Its revisions, oldest first.
    pub revisions: Vec<RevisionInfo>,
    /// The notes on it.
    pub annotations: Vec<AnnotationInfo>,
}

/// A note on a document.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AnnotationInfo {
    /// Its ID.
    pub id: AnnotationId,
    /// Its document.
    pub document_id: DocumentId,
    /// The revision it was made on.
    pub revision_id: RevisionId,
    /// What it says.
    pub note: String,
}

/// Something the library noticed and did not act on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SuggestionInfo {
    /// Its ID.
    pub id: SuggestionId,
    /// What made the library notice.
    pub kind: SuggestionKind,
    /// The document this is about, which is the newer one.
    pub document_id: DocumentId,
    /// The document it may be the same as.
    pub candidate_document_id: DocumentId,
    /// What was seen, for a person to judge.
    pub evidence: String,
    /// `open` or `dismissed`.
    pub state: String,
}

/// An entry in the journal of disk operations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct OperationInfo {
    /// Its ID.
    pub id: OperationId,
    /// `move_file`, `move_folder`, `recycle` or `restore`.
    pub kind: String,
    /// `intent`, `done`, `aborted`, `failed` or `undone`.
    pub state: String,
    /// The file it is about.
    pub instance_id: Option<InstanceId>,
    /// The folder it is about.
    pub folder_id: Option<FolderId>,
    /// Where it started.
    #[serde(serialize_with = "path_as_text")]
    pub source_path: PathBuf,
    /// Where it was going, if anywhere.
    pub target_path: Option<String>,
    /// The operation this one undoes.
    pub undo_of: Option<OperationId>,
    /// Why it failed.
    pub error: Option<String>,
}

/// How many of each thing the library holds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct LibraryStatus {
    /// Works.
    pub works: u64,
    /// Documents.
    pub documents: u64,
    /// Revisions.
    pub revisions: u64,
    /// Revisions whose full hash is not known yet.
    pub unconfirmed_revisions: u64,
    /// File instances whose bytes are here.
    pub present: u64,
    /// File instances not where they were last seen.
    pub absent: u64,
    /// File instances whose bytes are in the cloud.
    pub cloud_only: u64,
    /// File instances that could not be read.
    pub unreadable: u64,
    /// Folders on disk.
    pub folders: u64,
    /// Annotations not deleted.
    pub annotations: u64,
    /// Suggestions nobody has decided on.
    pub open_suggestions: u64,
    /// Journal entries whose outcome is not known.
    pub unfinished_operations: u64,
    /// The monitored folders.
    pub roots: Vec<RootInfo>,
}

/// What a user may change about a work. `None` leaves a field as it is.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorkEdit {
    /// Its title.
    pub title: Option<String>,
    /// Its rating, from 1 to 5.
    pub rating: Option<i64>,
    /// `unread`, `reading`, `read` or `abandoned`.
    pub status: Option<String>,
    /// Its series.
    pub series: Option<String>,
    /// Its position in that series.
    pub series_position: Option<f64>,
    /// Its authors, in order.
    pub creators: Option<Vec<String>>,
}

/// A work.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WorkInfo {
    /// Its ID.
    pub id: WorkId,
    /// Its title.
    pub title: String,
    /// Its language.
    pub language: Option<String>,
    /// Its year.
    pub year: Option<i64>,
    /// Its series.
    pub series: Option<String>,
    /// Its position in that series.
    pub series_position: Option<f64>,
    /// Its rating.
    pub rating: Option<i64>,
    /// `unread`, `reading`, `read` or `abandoned`.
    pub status: String,
    /// Its authors, in order.
    pub creators: Vec<String>,
    /// The documents it has.
    pub document_ids: Vec<DocumentId>,
}

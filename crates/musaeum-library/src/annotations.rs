//! What a person marks and writes in a book, and how each mark finds its text again.
//!
//! The model is the W3C Web Annotation one, kept relationally: an [`Annotation`] has one or more
//! [`Segment`]s, one per page or document it covers, and each segment has several [`Selector`]s,
//! tried in order until one finds the text (see `anchoring`). Everything goes through the one
//! writer of `library.db`; the interface shows a change before it is written and puts back what
//! the writer refused.
//!
//! Nothing here deletes anything: an annotation is deleted logically and can be restored, and one
//! whose selectors all fail is an orphan, still listed and found by search with its quote and note.
//! The schema enforces that too (migration 0004).

use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::library::Library;
use crate::model::{AnnotationId, DocumentId, RevisionId};
use crate::search::{self, MatchKind};
use crate::sql::now_ms;

/// The longest note kept, in bytes of Markdown.
pub const MAX_NOTE_BYTES: usize = 1 << 20;
/// The longest quote kept, in bytes.
pub const MAX_QUOTE_BYTES: usize = 256 << 10;
/// The most segments one annotation can have.
pub const MAX_SEGMENTS: usize = 4096;
/// The largest selector, in bytes of JSON: a long highlight's quads.
pub const MAX_SELECTOR_BYTES: usize = 1 << 20;

/// What kind of mark an annotation is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnnotationKind {
    Highlight,
    Underline,
    /// A note, shown as a mark on its text, or about the whole document when it has no segment.
    Note,
}

/// Where an annotation's quote was read from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuoteSource {
    /// A fixed layout's own text layer.
    TextLayer,
    /// A flowing book's normalized text.
    Flow,
    /// Text recognized in a picture of the page.
    Ocr,
    /// Nothing could be read there.
    None,
}

/// Whether an annotation, or a segment of it, is where it was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum AnchorState {
    /// Found again with the text it quotes.
    Exact,
    /// Found where the text is near enough to what it quotes, or only some segments were found.
    Approximate,
    /// Not found at all. It stays, with its quote and note.
    Orphan,
}

/// One way of finding a segment again.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectorKind {
    /// Canonical CFIs of where it starts and ends: `{"start", "end"}`.
    Cfi,
    /// UTF-16 code units into the document's normalized text: `{"start", "end"}`.
    TextPosition,
    /// `{"exact", "prefix", "suffix"}`.
    TextQuote,
    /// Characters of a page's text layer: `{"page", "start", "end"}`, the end excluded.
    PageCharRange,
    /// `{"page", "quads"}`, each quad `[x0, y0, x1, y1, x2, y2, x3, y3]` as fractions of the
    /// unrotated crop box: top left, top right, bottom right, bottom left.
    PageQuads,
    /// A mark on the picture of a page: lines found on a scan, a box or a stroke. See
    /// [`crate::anchoring::region`].
    PageRegion,
}

macro_rules! sql_names {
    ($type:ty { $($variant:ident => $name:literal),+ $(,)? }) => {
        impl $type {
            /// As the database spells it.
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $name),+
                }
            }

            fn parse(text: &str) -> rusqlite::Result<Self> {
                match text {
                    $($name => Ok(Self::$variant),)+
                    other => Err(rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        format!("unknown value {other:?}").into(),
                    )),
                }
            }
        }
    };
}

sql_names!(AnnotationKind { Highlight => "highlight", Underline => "underline", Note => "note" });
sql_names!(QuoteSource { TextLayer => "text_layer", Flow => "flow", Ocr => "ocr", None => "none" });
sql_names!(AnchorState { Exact => "exact", Approximate => "approximate", Orphan => "orphan" });
sql_names!(SelectorKind {
    Cfi => "cfi",
    TextPosition => "text_position",
    TextQuote => "text_quote",
    PageCharRange => "page_char_range",
    PageQuads => "page_quads",
    PageRegion => "page_region",
});

/// One way of finding a segment again, with what it counts in.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Selector {
    pub kind: SelectorKind,
    pub data: serde_json::Value,
    /// Lower goes first.
    pub priority: i64,
    /// The normalizer's version for a flowing book, the text layer's for a fixed one.
    pub normalizer_version: Option<u32>,
    /// The revision it was worked out on.
    pub revision_id: Option<RevisionId>,
}

/// One stretch of an annotation, on one page or in one document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    /// Its place in the annotation, from 0.
    pub ordinal: u32,
    /// The page it is on, in a fixed layout.
    pub page: Option<u32>,
    /// The document it is in, in a flowing book.
    pub href: Option<String>,
    /// A clip on the page, `[left, top, right, bottom]` in the unrotated crop box.
    pub clip: Option<[f64; 4]>,
    pub anchor_state: AnchorState,
    pub selectors: Vec<Selector>,
}

/// An annotation about to be written.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewAnnotation {
    pub document_id: DocumentId,
    /// The revision it is made on.
    pub revision_id: RevisionId,
    pub kind: AnnotationKind,
    pub color: String,
    pub note: String,
    pub quote: Option<String>,
    pub quote_source: QuoteSource,
    pub progression: Option<f64>,
    pub anchor_state: AnchorState,
    /// The version of what its selectors count in.
    pub resolved_version: Option<u32>,
    pub pdf_nm: Option<String>,
    pub segments: Vec<Segment>,
}

/// An annotation as the library has it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Annotation {
    /// The row, which the search indexes point at. Only meaningful inside this library.
    pub id: AnnotationId,
    /// What identifies it anywhere: a UUIDv7, hyphenated.
    pub uuid: String,
    pub document_id: DocumentId,
    /// The revision it was made on.
    pub revision_id: RevisionId,
    pub kind: AnnotationKind,
    pub color: String,
    pub note: String,
    pub quote: Option<String>,
    pub quote_source: QuoteSource,
    pub progression: Option<f64>,
    pub anchor_state: AnchorState,
    /// Where its anchors were last worked out, and the version of what they count in.
    pub resolved_revision_id: Option<RevisionId>,
    pub resolved_version: Option<u32>,
    pub pdf_nm: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub deleted_at: Option<i64>,
    pub segments: Vec<Segment>,
}

/// An annotation that comes from outside the library: a PDF's own, or one in an XFDF file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedAnnotation {
    /// What it is. Its `pdf_nm` is what tells it apart from the document's other annotations, and
    /// must be set: the PDF's `/NM`, or a name made from what it is when it has none.
    pub new: NewAnnotation,
    /// The UUID it had where it came from, kept when no annotation of this library has it: an
    /// annotation Musaeum exported comes back as itself.
    pub uuid: Option<String>,
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,
}

/// What an import did.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportOutcome {
    pub added: Vec<Annotation>,
    /// How many were already the document's, by their name or their UUID, deleted ones too.
    pub already: usize,
}

/// The most annotations one import may bring.
pub const MAX_IMPORTED: usize = 100_000;

/// What a person can change in an annotation. `None` leaves a field as it is.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnotationEdit {
    pub kind: Option<AnnotationKind>,
    pub color: Option<String>,
    pub note: Option<String>,
}

/// Where a segment was found again.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SegmentUpdate {
    pub ordinal: u32,
    pub page: Option<u32>,
    pub href: Option<String>,
    pub anchor_state: AnchorState,
    /// Selectors worked out again, replacing those of the same kind. A quote is never replaced.
    pub selectors: Vec<Selector>,
}

/// The outcome of finding an annotation again on a revision.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reanchoring {
    pub uuid: String,
    pub anchor_state: AnchorState,
    pub progression: Option<f64>,
    /// The revision it was worked out on, and the version of what it counts in.
    pub revision_id: RevisionId,
    pub version: Option<u32>,
    pub segments: Vec<SegmentUpdate>,
}

/// An annotation a search found.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnotationHit {
    pub annotation: Annotation,
    pub matched: MatchKind,
}

/// A new UUIDv7, hyphenated.
pub fn new_uuid() -> String {
    uuid::Uuid::now_v7().hyphenated().to_string()
}

fn uuid_bytes(text: &str) -> Result<[u8; 16]> {
    let parsed = uuid::Uuid::try_parse(text)
        .map_err(|_| Error::Refused(format!("{text:?} is not a UUID")))?;
    if parsed.get_version_num() != 7 {
        return Err(Error::Refused(format!("{text} is not a UUIDv7")));
    }
    Ok(*parsed.as_bytes())
}

fn uuid_text(bytes: &[u8]) -> rusqlite::Result<String> {
    uuid::Uuid::from_slice(bytes)
        .map(|uuid| uuid.hyphenated().to_string())
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Blob, error.into())
        })
}

fn check_color(color: &str) -> Result<()> {
    let fine = !color.is_empty()
        && color.len() <= 32
        && color
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'#');
    if fine {
        Ok(())
    } else {
        Err(Error::Refused(format!("{color:?} is not a color name")))
    }
}

fn check_note(note: &str) -> Result<()> {
    if note.len() > MAX_NOTE_BYTES {
        return Err(Error::Refused("the note is too long".to_owned()));
    }
    Ok(())
}

fn check_segment(segment: &Segment) -> Result<()> {
    if segment.page.is_some() == segment.href.is_some() {
        return Err(Error::Refused(
            "a segment is on a page or in a document, and only one".to_owned(),
        ));
    }
    if segment
        .href
        .as_ref()
        .is_some_and(|href| href.is_empty() || href.len() > 2048)
    {
        return Err(Error::Refused("that document name is too long".to_owned()));
    }
    if let Some(clip) = segment.clip
        && !clip.iter().all(|value| (0.0..=1.0).contains(value))
    {
        return Err(Error::Refused("a clip is outside the page".to_owned()));
    }
    let mut kinds = HashSet::new();
    for selector in &segment.selectors {
        if !kinds.insert(selector.kind) {
            return Err(Error::Refused(format!(
                "two {} selectors in one segment",
                selector.kind.as_str()
            )));
        }
        if serde_json::to_string(&selector.data)?.len() > MAX_SELECTOR_BYTES {
            return Err(Error::Refused("a selector is too large".to_owned()));
        }
    }
    Ok(())
}

fn check_new(new: &NewAnnotation) -> Result<()> {
    check_color(&new.color)?;
    check_note(&new.note)?;
    if new
        .quote
        .as_ref()
        .is_some_and(|quote| quote.len() > MAX_QUOTE_BYTES)
    {
        return Err(Error::Refused("the quote is too long".to_owned()));
    }
    if new
        .progression
        .is_some_and(|value| !(0.0..=1.0).contains(&value))
    {
        return Err(Error::Refused(
            "a progression is between 0 and 1".to_owned(),
        ));
    }
    if new.segments.len() > MAX_SEGMENTS {
        return Err(Error::Refused("too many segments".to_owned()));
    }
    let mut ordinals = HashSet::new();
    for segment in &new.segments {
        if !ordinals.insert(segment.ordinal) {
            return Err(Error::Refused(format!(
                "two segments numbered {}",
                segment.ordinal
            )));
        }
        check_segment(segment)?;
    }
    if new.pdf_nm.as_ref().is_some_and(|name| name.len() > 256) {
        return Err(Error::Refused("that /NM is too long".to_owned()));
    }
    Ok(())
}

fn insert_selector(transaction: &Transaction<'_>, segment: i64, selector: &Selector) -> Result<()> {
    transaction
        .prepare_cached(
            "INSERT INTO anchor_selector
                 (segment_id, kind, data, priority, normalizer_version, revision_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?
        .execute(params![
            segment,
            selector.kind.as_str(),
            serde_json::to_string(&selector.data)?,
            selector.priority,
            selector.normalizer_version,
            selector.revision_id,
        ])?;
    Ok(())
}

fn insert_segment(
    transaction: &Transaction<'_>,
    annotation: AnnotationId,
    segment: &Segment,
) -> Result<()> {
    let clip = segment
        .clip
        .map(|clip| serde_json::to_string(&clip))
        .transpose()?;
    transaction
        .prepare_cached(
            "INSERT INTO annotation_segment (annotation_id, ordinal, page, href, clip, anchor_state)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?
        .execute(params![
            annotation,
            segment.ordinal,
            segment.page,
            segment.href,
            clip,
            segment.anchor_state.as_str(),
        ])?;
    let id = transaction.last_insert_rowid();
    for selector in &segment.selectors {
        insert_selector(transaction, id, selector)?;
    }
    Ok(())
}

const ANNOTATION_COLUMNS: &str = "a.id, a.uuid, a.document_id, a.revision_id, a.kind, a.color, \
    a.note, a.quote, a.quote_source, a.progression, a.anchor_state, a.resolved_revision_id, \
    a.resolved_version, a.pdf_nm, a.created_at, a.updated_at, a.deleted_at";

fn annotation_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Annotation> {
    Ok(Annotation {
        id: row.get(0)?,
        uuid: uuid_text(&row.get::<_, Vec<u8>>(1)?)?,
        document_id: row.get(2)?,
        revision_id: row.get(3)?,
        kind: AnnotationKind::parse(&row.get::<_, String>(4)?)?,
        color: row.get(5)?,
        note: row.get(6)?,
        quote: row.get(7)?,
        quote_source: QuoteSource::parse(&row.get::<_, String>(8)?)?,
        progression: row.get(9)?,
        anchor_state: AnchorState::parse(&row.get::<_, String>(10)?)?,
        resolved_revision_id: row.get(11)?,
        resolved_version: row.get(12)?,
        pdf_nm: row.get(13)?,
        created_at: row.get(14)?,
        updated_at: row.get(15)?,
        deleted_at: row.get(16)?,
        segments: Vec::new(),
    })
}

/// Fills in the segments and selectors of annotations already read.
fn attach_segments(connection: &Connection, annotations: &mut [Annotation]) -> Result<()> {
    if annotations.is_empty() {
        return Ok(());
    }
    let at: HashMap<AnnotationId, usize> = annotations
        .iter()
        .enumerate()
        .map(|(index, annotation)| (annotation.id, index))
        .collect();
    // One query per table, through a JSON array of the ids, however many there are.
    let ids = serde_json::to_string(&at.keys().collect::<Vec<_>>())?;
    let mut segment_at: HashMap<i64, (usize, usize)> = HashMap::new();
    {
        let mut statement = connection.prepare_cached(
            "SELECT s.id, s.annotation_id, s.ordinal, s.page, s.href, s.clip, s.anchor_state
             FROM annotation_segment s
             WHERE s.annotation_id IN (SELECT value FROM json_each(?1))
             ORDER BY s.annotation_id, s.ordinal",
        )?;
        let rows = statement.query_map([&ids], |row| {
            let clip: Option<String> = row.get(5)?;
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, AnnotationId>(1)?,
                Segment {
                    ordinal: row.get(2)?,
                    page: row.get(3)?,
                    href: row.get(4)?,
                    clip: clip.and_then(|text| serde_json::from_str(&text).ok()),
                    anchor_state: AnchorState::parse(&row.get::<_, String>(6)?)?,
                    selectors: Vec::new(),
                },
            ))
        })?;
        for row in rows {
            let (id, annotation, segment) = row?;
            let index = at[&annotation];
            segment_at.insert(id, (index, annotations[index].segments.len()));
            annotations[index].segments.push(segment);
        }
    }
    let mut statement = connection.prepare_cached(
        "SELECT x.segment_id, x.kind, x.data, x.priority, x.normalizer_version, x.revision_id
         FROM anchor_selector x JOIN annotation_segment s ON s.id = x.segment_id
         WHERE s.annotation_id IN (SELECT value FROM json_each(?1))
         ORDER BY x.segment_id, x.priority, x.id",
    )?;
    let rows = statement.query_map([&ids], |row| {
        let data: String = row.get(2)?;
        Ok((
            row.get::<_, i64>(0)?,
            Selector {
                kind: SelectorKind::parse(&row.get::<_, String>(1)?)?,
                data: serde_json::from_str(&data).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        2,
                        rusqlite::types::Type::Text,
                        error.into(),
                    )
                })?,
                priority: row.get(3)?,
                normalizer_version: row.get(4)?,
                revision_id: row.get(5)?,
            },
        ))
    })?;
    for row in rows {
        let (segment, selector) = row?;
        if let Some(&(annotation, index)) = segment_at.get(&segment) {
            annotations[annotation].segments[index]
                .selectors
                .push(selector);
        }
    }
    Ok(())
}

fn load_where(
    connection: &Connection,
    condition: &str,
    value: &dyn rusqlite::ToSql,
) -> Result<Vec<Annotation>> {
    let sql = format!(
        "SELECT {ANNOTATION_COLUMNS} FROM annotation a WHERE {condition}
         ORDER BY a.progression IS NULL, a.progression, a.created_at, a.id"
    );
    let mut annotations = connection
        .prepare_cached(&sql)?
        .query_map([value], annotation_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    attach_segments(connection, &mut annotations)?;
    Ok(annotations)
}

fn load_one(connection: &Connection, uuid: &[u8; 16]) -> Result<Annotation> {
    load_where(connection, "a.uuid = ?1", &uuid.as_slice())?
        .pop()
        .ok_or_else(|| Error::NotFound("there is no such annotation".to_owned()))
}

fn id_of(transaction: &Transaction<'_>, uuid: &[u8; 16]) -> Result<(AnnotationId, DocumentId)> {
    transaction
        .prepare_cached("SELECT id, document_id FROM annotation WHERE uuid = ?1")?
        .query_row([uuid.as_slice()], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?
        .ok_or_else(|| Error::NotFound("there is no such annotation".to_owned()))
}

/// Refuses a revision that is not the document's.
fn check_owner(
    transaction: &Transaction<'_>,
    revision: RevisionId,
    document: DocumentId,
) -> Result<()> {
    let owner: Option<DocumentId> = transaction
        .prepare_cached("SELECT document_id FROM revision WHERE id = ?1")?
        .query_row([revision], |row| row.get(0))
        .optional()?;
    if owner != Some(document) {
        return Err(Error::Refused(format!(
            "revision {revision} is not one of document {document}'s"
        )));
    }
    Ok(())
}

/// Writes an annotation and its segments, with the UUID and the dates given.
fn insert_annotation(
    transaction: &Transaction<'_>,
    new: &NewAnnotation,
    uuid: &[u8; 16],
    created: i64,
    updated: i64,
) -> Result<()> {
    transaction
        .prepare_cached(
            "INSERT INTO annotation (uuid, document_id, revision_id, kind, color, note, quote,
                 quote_source, progression, anchor_state, resolved_revision_id,
                 resolved_version, pdf_nm, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?3, ?11, ?12, ?13, ?14)",
        )?
        .execute(params![
            uuid.as_slice(),
            new.document_id,
            new.revision_id,
            new.kind.as_str(),
            new.color,
            new.note,
            new.quote,
            new.quote_source.as_str(),
            new.progression,
            new.anchor_state.as_str(),
            new.resolved_version,
            new.pdf_nm,
            created,
            updated,
        ])?;
    let id = transaction.last_insert_rowid();
    for segment in &new.segments {
        insert_segment(transaction, id, segment)?;
    }
    Ok(())
}

impl Library {
    /// Writes a new annotation and returns it as stored, with its UUID.
    pub fn create_annotation(&self, new: NewAnnotation) -> Result<Annotation> {
        check_new(&new)?;
        let uuid = uuid_bytes(&new_uuid())?;
        self.database().write(move |transaction| {
            check_owner(transaction, new.revision_id, new.document_id)?;
            let now = now_ms();
            insert_annotation(transaction, &new, &uuid, now, now)?;
            load_one(transaction, &uuid)
        })
    }

    /// Brings annotations from outside into one document, in one transaction. One the document
    /// already has, by its `pdf_nm` or by a UUID that is its `pdf_nm`, deleted ones included, is
    /// left as it is, so importing the same file again adds nothing. Each keeps the UUID it came
    /// with when that is a UUIDv7 no annotation of the library has, and its dates.
    pub fn import_annotations(
        &self,
        document: DocumentId,
        revision: RevisionId,
        imported: Vec<ImportedAnnotation>,
    ) -> Result<ImportOutcome> {
        if imported.len() > MAX_IMPORTED {
            return Err(Error::Refused(
                "too many annotations for one import".to_owned(),
            ));
        }
        for item in &imported {
            check_new(&item.new)?;
            if item.new.document_id != document || item.new.revision_id != revision {
                return Err(Error::Refused(
                    "an imported annotation of another document".to_owned(),
                ));
            }
            if item
                .new
                .pdf_nm
                .as_deref()
                .is_none_or(|name| name.trim().is_empty())
            {
                return Err(Error::Refused(
                    "an imported annotation needs a name to be told apart".to_owned(),
                ));
            }
        }
        self.database().write(move |transaction| {
            check_owner(transaction, revision, document)?;
            let now = now_ms();
            let mut seen = HashSet::new();
            let mut added = Vec::new();
            let mut already = 0;
            for item in &imported {
                let name = item.new.pdf_nm.clone().unwrap_or_default();
                let as_uuid = uuid::Uuid::try_parse(&name)
                    .ok()
                    .map(|uuid| uuid.as_bytes().to_vec());
                let known: bool = transaction
                    .prepare_cached(
                        "SELECT EXISTS (SELECT 1 FROM annotation
                         WHERE document_id = ?1 AND (pdf_nm = ?2 OR uuid = ?3))",
                    )?
                    .query_row(params![document, name, as_uuid], |row| row.get(0))?;
                if known || !seen.insert(name) {
                    already += 1;
                    continue;
                }
                let kept = match item.uuid.as_deref().and_then(|text| uuid_bytes(text).ok()) {
                    Some(bytes) => {
                        let taken: bool = transaction
                            .prepare_cached(
                                "SELECT EXISTS (SELECT 1 FROM annotation WHERE uuid = ?1)",
                            )?
                            .query_row([bytes.as_slice()], |row| row.get(0))?;
                        (!taken).then_some(bytes)
                    }
                    None => None,
                };
                let uuid = match kept {
                    Some(bytes) => bytes,
                    None => uuid_bytes(&new_uuid())?,
                };
                let created = item.created_at.filter(|ms| *ms > 0).unwrap_or(now);
                let updated = item
                    .updated_at
                    .filter(|ms| *ms > 0)
                    .unwrap_or(created)
                    .max(created);
                insert_annotation(transaction, &item.new, &uuid, created, updated)?;
                added.push(load_one(transaction, &uuid)?);
            }
            Ok(ImportOutcome { added, already })
        })
    }

    /// Fills the quote of an annotation that had none, once: what the OCR read under a mark on a
    /// scanned page. Each segment named by its ordinal gets the selectors given, a quote of the text
    /// under it among them; a selector of a kind the segment already has is left as it was. An
    /// annotation that already quotes something is returned unchanged: its quote never changes.
    pub fn fill_quote(
        &self,
        uuid: &str,
        quote: &str,
        source: QuoteSource,
        selectors: Vec<(u32, Selector)>,
    ) -> Result<Annotation> {
        let bytes = uuid_bytes(uuid)?;
        if quote.trim().is_empty() || quote.len() > MAX_QUOTE_BYTES {
            return Err(Error::Refused(
                "a quote is some text, and not too long".to_owned(),
            ));
        }
        for (_, selector) in &selectors {
            if serde_json::to_string(&selector.data)?.len() > MAX_SELECTOR_BYTES {
                return Err(Error::Refused("a selector is too large".to_owned()));
            }
        }
        let quote = quote.to_owned();
        self.database().write(move |transaction| {
            let (id, _) = id_of(transaction, &bytes)?;
            let current: Option<String> = transaction
                .prepare_cached("SELECT quote FROM annotation WHERE id = ?1")?
                .query_row([id], |row| row.get(0))?;
            if current.is_some() {
                return load_one(transaction, &bytes);
            }
            transaction
                .prepare_cached(
                    "UPDATE annotation SET quote = ?2, quote_source = ?3, updated_at = ?4
                     WHERE id = ?1",
                )?
                .execute(params![id, quote, source.as_str(), now_ms()])?;
            for (ordinal, selector) in &selectors {
                let segment: Option<i64> = transaction
                    .prepare_cached(
                        "SELECT id FROM annotation_segment
                         WHERE annotation_id = ?1 AND ordinal = ?2",
                    )?
                    .query_row(params![id, ordinal], |row| row.get(0))
                    .optional()?;
                let Some(segment) = segment else {
                    continue;
                };
                transaction
                    .prepare_cached(
                        "INSERT OR IGNORE INTO anchor_selector
                             (segment_id, kind, data, priority, normalizer_version, revision_id)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    )?
                    .execute(params![
                        segment,
                        selector.kind.as_str(),
                        serde_json::to_string(&selector.data)?,
                        selector.priority,
                        selector.normalizer_version,
                        selector.revision_id,
                    ])?;
            }
            load_one(transaction, &bytes)
        })
    }

    /// A document's annotations, by where they start; the deleted ones too if asked.
    pub fn annotations(
        &self,
        document: DocumentId,
        include_deleted: bool,
    ) -> Result<Vec<Annotation>> {
        self.database().read(|connection| {
            if include_deleted {
                load_where(connection, "a.document_id = ?1", &document)
            } else {
                load_where(
                    connection,
                    "a.document_id = ?1 AND a.deleted_at IS NULL",
                    &document,
                )
            }
        })
    }

    /// One annotation, deleted or not.
    pub fn annotation(&self, uuid: &str) -> Result<Annotation> {
        let bytes = uuid_bytes(uuid)?;
        self.database()
            .read(|connection| load_one(connection, &bytes))
    }

    /// Changes what a person can change: the kind of mark, its color, its note.
    pub fn edit_annotation(&self, uuid: &str, edit: AnnotationEdit) -> Result<Annotation> {
        let bytes = uuid_bytes(uuid)?;
        if let Some(color) = &edit.color {
            check_color(color)?;
        }
        if let Some(note) = &edit.note {
            check_note(note)?;
        }
        self.database().write(move |transaction| {
            let (id, _) = id_of(transaction, &bytes)?;
            transaction
                .prepare_cached(
                    "UPDATE annotation SET kind = coalesce(?2, kind), color = coalesce(?3, color),
                         note = coalesce(?4, note), updated_at = ?5
                     WHERE id = ?1",
                )?
                .execute(params![
                    id,
                    edit.kind.map(AnnotationKind::as_str),
                    edit.color,
                    edit.note,
                    now_ms(),
                ])?;
            load_one(transaction, &bytes)
        })
    }

    /// Deletes an annotation logically: it leaves the reader and the search, and can come back.
    pub fn delete_annotation(&self, uuid: &str) -> Result<Annotation> {
        self.set_deleted(uuid, true)
    }

    /// Brings back an annotation deleted logically.
    pub fn restore_annotation(&self, uuid: &str) -> Result<Annotation> {
        self.set_deleted(uuid, false)
    }

    fn set_deleted(&self, uuid: &str, deleted: bool) -> Result<Annotation> {
        let bytes = uuid_bytes(uuid)?;
        self.database().write(move |transaction| {
            let (id, _) = id_of(transaction, &bytes)?;
            let now = now_ms();
            transaction
                .prepare_cached(
                    "UPDATE annotation SET deleted_at = CASE WHEN ?2 THEN coalesce(deleted_at, ?3) END,
                         updated_at = ?3
                     WHERE id = ?1",
                )?
                .execute(params![id, deleted, now])?;
            load_one(transaction, &bytes)
        })
    }

    /// Records where annotations were found on a revision. A segment's selectors of the kinds given
    /// are replaced and the others kept; a quote is never replaced, and nothing is deleted.
    pub fn record_anchoring(&self, outcomes: Vec<Reanchoring>) -> Result<()> {
        let mut parsed = Vec::with_capacity(outcomes.len());
        for outcome in outcomes {
            if outcome
                .progression
                .is_some_and(|value| !(0.0..=1.0).contains(&value))
            {
                return Err(Error::Refused(
                    "a progression is between 0 and 1".to_owned(),
                ));
            }
            for segment in &outcome.segments {
                check_segment(&Segment {
                    ordinal: segment.ordinal,
                    page: segment.page,
                    href: segment.href.clone(),
                    clip: None,
                    anchor_state: segment.anchor_state,
                    selectors: segment.selectors.clone(),
                })?;
            }
            parsed.push((uuid_bytes(&outcome.uuid)?, outcome));
        }
        self.database().write(move |transaction| {
            for (uuid, outcome) in &parsed {
                let (id, document) = id_of(transaction, uuid)?;
                let owner: Option<DocumentId> = transaction
                    .prepare_cached("SELECT document_id FROM revision WHERE id = ?1")?
                    .query_row([outcome.revision_id], |row| row.get(0))
                    .optional()?;
                if owner != Some(document) {
                    return Err(Error::Refused(format!(
                        "revision {} is not one of document {document}'s",
                        outcome.revision_id
                    )));
                }
                transaction
                    .prepare_cached(
                        "UPDATE annotation SET anchor_state = ?2, progression = coalesce(?3, progression),
                             resolved_revision_id = ?4, resolved_version = ?5
                         WHERE id = ?1",
                    )?
                    .execute(params![
                        id,
                        outcome.anchor_state.as_str(),
                        outcome.progression,
                        outcome.revision_id,
                        outcome.version,
                    ])?;
                for segment in &outcome.segments {
                    let segment_id: i64 = transaction
                        .prepare_cached(
                            "SELECT id FROM annotation_segment WHERE annotation_id = ?1 AND ordinal = ?2",
                        )?
                        .query_row(params![id, segment.ordinal], |row| row.get(0))
                        .optional()?
                        .ok_or_else(|| {
                            Error::NotFound(format!("the annotation has no segment {}", segment.ordinal))
                        })?;
                    transaction
                        .prepare_cached(
                            "UPDATE annotation_segment SET page = ?2, href = ?3, anchor_state = ?4
                             WHERE id = ?1",
                        )?
                        .execute(params![
                            segment_id,
                            segment.page,
                            segment.href,
                            segment.anchor_state.as_str(),
                        ])?;
                    for selector in &segment.selectors {
                        if selector.kind == SelectorKind::TextQuote {
                            continue;
                        }
                        let updated = transaction
                            .prepare_cached(
                                "UPDATE anchor_selector SET data = ?3, priority = ?4,
                                     normalizer_version = ?5, revision_id = ?6
                                 WHERE segment_id = ?1 AND kind = ?2",
                            )?
                            .execute(params![
                                segment_id,
                                selector.kind.as_str(),
                                serde_json::to_string(&selector.data)?,
                                selector.priority,
                                selector.normalizer_version,
                                selector.revision_id,
                            ])?;
                        if updated == 0 {
                            insert_selector(transaction, segment_id, selector)?;
                        }
                    }
                }
            }
            Ok(())
        })
    }

    /// Finds annotations by what they quote and what their notes say, in one document or in all.
    /// Words find what starts with what was typed, in any language's accents; trigrams, the middle
    /// of a word. Orphans are found too; deleted annotations are not.
    pub fn search_annotations(
        &self,
        document: Option<DocumentId>,
        query: &str,
        limit: usize,
    ) -> Result<Vec<AnnotationHit>> {
        let cut = search::terms_of(query);
        let terms: Vec<&str> = cut.iter().map(String::as_str).collect();
        if terms.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        self.database().read(|connection| {
            let tiers = [
                (
                    "annotation_words",
                    search::word_start_query(&terms),
                    MatchKind::WordStart,
                ),
                (
                    "annotation_trigrams",
                    search::substring_query(&terms),
                    MatchKind::Substring,
                ),
            ];
            let mut hits: Vec<AnnotationHit> = Vec::new();
            let mut seen = HashSet::new();
            for (table, expression, matched) in tiers {
                let Some(expression) = expression else {
                    continue;
                };
                let sql = format!(
                    "SELECT a.id FROM {table} f JOIN annotation a ON a.id = f.rowid
                     WHERE {table} MATCH ?1 AND a.deleted_at IS NULL
                       AND (?2 IS NULL OR a.document_id = ?2)
                     LIMIT ?3"
                );
                let ids = connection
                    .prepare_cached(&sql)?
                    .query_map(params![expression, document, limit as i64], |row| {
                        row.get::<_, AnnotationId>(0)
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                let fresh: Vec<AnnotationId> =
                    ids.into_iter().filter(|id| seen.insert(*id)).collect();
                if fresh.is_empty() {
                    continue;
                }
                let list = serde_json::to_string(&fresh)?;
                for annotation in load_where(
                    connection,
                    "a.id IN (SELECT value FROM json_each(?1))",
                    &list,
                )? {
                    if hits.len() == limit {
                        break;
                    }
                    hits.push(AnnotationHit {
                        annotation,
                        matched,
                    });
                }
                if hits.len() == limit {
                    break;
                }
            }
            Ok(hits)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuids_are_version_7_and_sort_by_time() {
        let first = new_uuid();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let second = new_uuid();
        assert!(first < second, "{first} {second}");
        assert_eq!(uuid_bytes(&first).unwrap()[6] >> 4, 7);
        assert!(
            uuid_bytes("6ba7b810-9dad-11d1-80b4-00c04fd430c8").is_err(),
            "a UUIDv1"
        );
        assert!(uuid_bytes("not a uuid").is_err());
    }

    #[test]
    fn colors_are_names() {
        for color in ["yellow", "green", "x-1", "#ffcc00"] {
            assert!(check_color(color).is_ok(), "{color}");
        }
        for color in ["", "red; drop", "a b", &"x".repeat(33)] {
            assert!(check_color(color).is_err(), "{color}");
        }
    }
}

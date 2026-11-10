//! What reading needs from the library: where the reader is in each document, and the shape of each
//! fixed-layout file's pages.
//!
//! The position is the user's and lives in `library.db`. The pages can always be read again from the
//! file, so they live in `cache.db`, by the exact bytes, which is also what keeps them right after the
//! file is renamed and never wrong after it is edited. That goes for their size and shape, and for what
//! they are made of: text, a scan, or a scan with OCR's text over it.

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::fingerprint::Hash;
use crate::library::Library;
use crate::model::{DocumentId, RevisionId};
use crate::sql::now_ms;

/// The two ways of reading a fixed-layout document. Only paginated reading turns pages, so only it
/// has an animation to turn on or off, and [`ReadingState`] keeps that preference across a switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadingMode {
    /// One page or one spread at a time.
    Paginated,
    /// All pages one after another, scrolled.
    Continuous,
}

/// Whether pages are shown side by side.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Spreads {
    /// As the document's `/PageLayout` asks.
    Auto,
    /// One page at a time.
    Single,
    /// Facing pages, as printed.
    Double,
}

/// Where a reader is in a fixed-layout document, and how they read it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReadingState {
    /// The revision the position was taken on.
    pub revision_id: Option<RevisionId>,
    /// Zero-based page of the Locator.
    pub page: u32,
    /// Horizontal point on that page, from 0 at its left edge to 1 at its right edge.
    pub u: f64,
    /// Vertical point on that page, from 0 at its top to 1 at its bottom.
    pub v: f64,
    /// How far through the document, from 0 to 1.
    pub progress: f64,
    /// How the document is being read.
    pub mode: ReadingMode,
    /// Whether paginated reading animates turning a page. Kept while reading continuously.
    pub page_turn_animation: bool,
    /// A factor over what fits the window in the current mode.
    pub zoom: f64,
    /// Whether pages are shown side by side.
    pub spreads: Spreads,
    /// When it was saved, in milliseconds since the Unix epoch. Ignored when saving.
    pub updated_at: i64,
}

/// The shape of a page, from its proportions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageOrientation {
    /// Taller than wide.
    Portrait,
    /// Wider than tall.
    Landscape,
    /// Close enough to square to be neither.
    Square,
}

/// The size of one page.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PageRecord {
    /// Width in points, with the page's `/Rotate` applied.
    pub width_pt: f32,
    /// Height in points, with the page's `/Rotate` applied.
    pub height_pt: f32,
    /// Its shape, as whoever measured it classified it.
    pub orientation: PageOrientation,
}

/// The pages of a fixed-layout file, and what the file asks of a reader.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DocumentGeometry {
    /// Every page, in order.
    pub pages: Vec<PageRecord>,
    /// The catalog's `/PageLayout`, such as `TwoPageRight`.
    pub page_layout: Option<String>,
    /// `/ViewerPreferences /Direction`: `L2R` or `R2L`.
    pub direction: Option<String>,
}

impl DocumentGeometry {
    /// What all its pages are like: one orientation if they share it, `mixed` if not.
    pub fn summary(&self) -> &'static str {
        let mut shapes = self.pages.iter().map(|page| page.orientation);
        let Some(first) = shapes.next() else {
            return "empty";
        };
        if shapes.any(|shape| shape != first) {
            "mixed"
        } else {
            first.as_str()
        }
    }

    fn count(&self, orientation: PageOrientation) -> i64 {
        self.pages
            .iter()
            .filter(|page| page.orientation == orientation)
            .count() as i64
    }
}

impl PageOrientation {
    /// How the cache writes this.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Portrait => "portrait",
            Self::Landscape => "landscape",
            Self::Square => "square",
        }
    }
}

macro_rules! text_column {
    ($type:ty, $($variant:ident => $text:literal),+) => {
        impl ToSql for $type {
            fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
                Ok(ToSqlOutput::from(match self {
                    $(Self::$variant => $text,)+
                }))
            }
        }

        impl FromSql for $type {
            fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
                match value.as_str()? {
                    $($text => Ok(Self::$variant),)+
                    _ => Err(FromSqlError::InvalidType),
                }
            }
        }
    };
}

text_column!(ReadingMode, Paginated => "paginated", Continuous => "continuous");
text_column!(Spreads, Auto => "auto", Single => "single", Double => "double");
text_column!(
    PageOrientation,
    Portrait => "portrait",
    Landscape => "landscape",
    Square => "square"
);

/// What a page is made of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageContentClass {
    /// Text and drawings the document describes.
    Text,
    /// A picture of the page, with no text layer.
    Scanned,
    /// A picture of the page with a text layer, as OCR leaves one.
    Hybrid,
}

impl PageContentClass {
    /// How the cache writes this.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Scanned => "scanned",
            Self::Hybrid => "hybrid",
        }
    }
}

text_column!(
    PageContentClass,
    Text => "text",
    Scanned => "scanned",
    Hybrid => "hybrid"
);

/// Which rules classified a page. Pages classified by other rules count as not classified.
pub const CONTENT_RULES_VERSION: i64 = 1;

/// The share of a document's pages, in percent, that one class needs to name the whole document.
/// Below it the document is `mixed`. Strict unanimity left most text books `mixed` over a scanned
/// cover or a few plates, so the project chose a qualified majority at the Phase 2 audit.
pub const CONTENT_MAJORITY_PERCENT: u64 = 95;

/// What a document is made of, from how many of its pages are of each class: the class that at
/// least [`CONTENT_MAJORITY_PERCENT`] of them have, or `mixed`, and whether every page has it.
/// `None` for a document with no pages.
pub fn content_summary(text: u64, scanned: u64, hybrid: u64) -> Option<(&'static str, bool)> {
    let total = text + scanned + hybrid;
    if total == 0 {
        return None;
    }
    let counts = [("text", text), ("scanned", scanned), ("hybrid", hybrid)];
    let uniform = counts.iter().filter(|(_, count)| *count > 0).count() == 1;
    let class = counts
        .iter()
        .find(|(_, count)| count * 100 >= total * CONTENT_MAJORITY_PERCENT)
        .map_or("mixed", |(class, _)| *class);
    Some((class, uniform))
}

/// What one page is made of, and the measurements that decided it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PageContentRecord {
    /// The class.
    pub class: PageContentClass,
    /// Characters in the page's text layer, white space not counted.
    pub text_chars: u32,
    /// Fraction of the page that raster images cover, from 0 to 1.
    pub image_coverage: f32,
}

/// What a file's pages are made of, as far as they have been classified.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DocumentContent {
    /// Every page, in order: `None` until it is classified by the current rules.
    pub pages: Vec<Option<PageContentRecord>>,
    /// `text`, `scanned` or `hybrid` if at least [`CONTENT_MAJORITY_PERCENT`] of the pages are
    /// that, `mixed` if not; `None` until every page is classified.
    pub summary: Option<String>,
    /// Whether every page has the same class; `None` until every page is classified.
    pub is_strictly_uniform: Option<bool>,
}

impl DocumentContent {
    /// The pages still to classify.
    pub fn unclassified(&self) -> Vec<u32> {
        (0u32..)
            .zip(&self.pages)
            .filter(|(_, page)| page.is_none())
            .map(|(index, _)| index)
            .collect()
    }
}

/// Most zoom a reading state may hold, which is also the database's limit.
const MAX_ZOOM: f64 = 64.0;

fn check(state: &ReadingState) -> Result<()> {
    let unit = |value: f64| value.is_finite() && (0.0..=1.0).contains(&value);
    if !(unit(state.u) && unit(state.v) && unit(state.progress)) {
        return Err(Error::Refused(
            "a position and its progress go from 0 to 1".to_owned(),
        ));
    }
    if !(state.zoom.is_finite() && state.zoom > 0.0 && state.zoom <= MAX_ZOOM) {
        return Err(Error::Refused(format!(
            "a zoom goes above 0 and up to {MAX_ZOOM}"
        )));
    }
    Ok(())
}

/// The longest Locator a reading state keeps, as JSON.
const MAX_LOCATOR: usize = 16 << 10;
/// The longest name and value of a setting.
const MAX_PREFERENCE_KEY: usize = 64;
const MAX_PREFERENCE_VALUE: usize = 64 << 10;

/// Where a reader is in a flowing book, and how they read it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FlowReadingState {
    /// The revision the position was taken on.
    pub revision_id: Option<RevisionId>,
    /// The Locator, as JSON: a canonical CFI, the text around it and the progressions.
    pub locator: String,
    /// How far through the book, from 0 to 1.
    pub progress: f64,
    /// Paginated or continuous.
    pub mode: ReadingMode,
    /// Whether paginated reading animates turning a page.
    pub page_turn_animation: bool,
    /// When it was saved, in milliseconds since the Unix epoch. Ignored when saving.
    pub updated_at: i64,
}

fn check_flow(state: &FlowReadingState) -> Result<()> {
    if !(state.progress.is_finite() && (0.0..=1.0).contains(&state.progress)) {
        return Err(Error::Refused("progress goes from 0 to 1".to_owned()));
    }
    if state.locator.len() > MAX_LOCATOR {
        return Err(Error::Refused(
            "that Locator is too large to keep".to_owned(),
        ));
    }
    match serde_json::from_str::<serde_json::Value>(&state.locator) {
        Ok(serde_json::Value::Object(_)) => Ok(()),
        _ => Err(Error::Refused("a Locator is a JSON object".to_owned())),
    }
}

/// The document has to exist, and the revision, if there is one, has to be one of its own.
fn check_owner(
    transaction: &rusqlite::Transaction<'_>,
    document: DocumentId,
    revision: Option<RevisionId>,
) -> Result<()> {
    let known: Option<i64> = transaction
        .prepare_cached("SELECT 1 FROM document WHERE id = ?1")?
        .query_row([document], |row| row.get(0))
        .optional()?;
    if known.is_none() {
        return Err(Error::NotFound(format!("there is no document {document}")));
    }
    if let Some(revision) = revision {
        let owner: Option<DocumentId> = transaction
            .prepare_cached("SELECT document_id FROM revision WHERE id = ?1")?
            .query_row([revision], |row| row.get(0))
            .optional()?;
        if owner != Some(document) {
            return Err(Error::Refused(format!(
                "revision {revision} is not one of document {document}'s"
            )));
        }
    }
    Ok(())
}

/// Which document a file being opened is, and its exact bytes, when the library knows them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpenedFile {
    /// The document.
    pub document: DocumentId,
    /// The revision the file is.
    pub revision: RevisionId,
    /// That revision's hash, which is what the cache knows pages by. `None` until it is confirmed.
    pub blake3: Option<Hash>,
}

impl Library {
    /// The document a file is, if the library has it at that path.
    pub fn opened_file(&self, path: &std::path::Path) -> Result<Option<OpenedFile>> {
        let Some(instance) = self.instance_at(path)? else {
            return Ok(None);
        };
        let blake3: Option<Vec<u8>> = self.database().read(|connection| {
            Ok(connection
                .prepare_cached("SELECT blake3 FROM revision WHERE id = ?1")?
                .query_row([instance.revision_id], |row| row.get(0))
                .optional()?
                .flatten())
        })?;
        Ok(Some(OpenedFile {
            document: instance.document_id,
            revision: instance.revision_id,
            blake3: blake3.and_then(|bytes| bytes.try_into().ok()),
        }))
    }

    /// Where the reader left this document, if they ever opened it.
    pub fn reading_state(&self, document: DocumentId) -> Result<Option<ReadingState>> {
        self.database().read(|connection| {
            Ok(connection
                .prepare_cached(
                    "SELECT revision_id, page, u, v, progress, mode, page_turn_animation, zoom,
                            spreads, updated_at
                     FROM reading_state WHERE document_id = ?1",
                )?
                .query_row([document], |row| {
                    Ok(ReadingState {
                        revision_id: row.get(0)?,
                        page: row.get(1)?,
                        u: row.get(2)?,
                        v: row.get(3)?,
                        progress: row.get(4)?,
                        mode: row.get(5)?,
                        page_turn_animation: row.get(6)?,
                        zoom: row.get(7)?,
                        spreads: row.get(8)?,
                        updated_at: row.get(9)?,
                    })
                })
                .optional()?)
        })
    }

    /// Remembers where the reader is. The revision, if given, has to be one of this document's.
    pub fn save_reading_state(&self, document: DocumentId, state: &ReadingState) -> Result<()> {
        check(state)?;
        let state = state.clone();
        self.database().write(move |transaction| {
            check_owner(transaction, document, state.revision_id)?;
            transaction
                .prepare_cached(
                    "INSERT INTO reading_state (document_id, revision_id, page, u, v, progress, mode,
                                                page_turn_animation, zoom, spreads, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                     ON CONFLICT (document_id) DO UPDATE SET
                         revision_id = excluded.revision_id, page = excluded.page, u = excluded.u,
                         v = excluded.v, progress = excluded.progress, mode = excluded.mode,
                         page_turn_animation = excluded.page_turn_animation, zoom = excluded.zoom,
                         spreads = excluded.spreads, updated_at = excluded.updated_at",
                )?
                .execute(params![
                    document,
                    state.revision_id,
                    state.page,
                    state.u,
                    state.v,
                    state.progress,
                    state.mode,
                    state.page_turn_animation,
                    state.zoom,
                    state.spreads,
                    now_ms(),
                ])?;
            Ok(())
        })
    }

    /// Where the reader left this flowing book, if they ever opened it.
    pub fn flow_reading_state(&self, document: DocumentId) -> Result<Option<FlowReadingState>> {
        self.database().read(|connection| {
            Ok(connection
                .prepare_cached(
                    "SELECT revision_id, locator, progress, mode, page_turn_animation, updated_at
                     FROM reading_state WHERE document_id = ?1 AND locator IS NOT NULL",
                )?
                .query_row([document], |row| {
                    Ok(FlowReadingState {
                        revision_id: row.get(0)?,
                        locator: row.get(1)?,
                        progress: row.get(2)?,
                        mode: row.get(3)?,
                        page_turn_animation: row.get(4)?,
                        updated_at: row.get(5)?,
                    })
                })
                .optional()?)
        })
    }

    /// Remembers where the reader is in a flowing book. The Locator is kept as the reader gave it,
    /// once it is known to be a JSON object of reasonable size.
    pub fn save_flow_reading_state(
        &self,
        document: DocumentId,
        state: &FlowReadingState,
    ) -> Result<()> {
        check_flow(state)?;
        let state = state.clone();
        self.database().write(move |transaction| {
            check_owner(transaction, document, state.revision_id)?;
            transaction
                .prepare_cached(
                    "INSERT INTO reading_state (document_id, revision_id, page, u, v, progress, mode,
                                                page_turn_animation, zoom, spreads, updated_at, locator)
                     VALUES (?1, ?2, 0, 0, 0, ?3, ?4, ?5, 1, 'auto', ?6, ?7)
                     ON CONFLICT (document_id) DO UPDATE SET
                         revision_id = excluded.revision_id, progress = excluded.progress,
                         mode = excluded.mode, page_turn_animation = excluded.page_turn_animation,
                         updated_at = excluded.updated_at, locator = excluded.locator",
                )?
                .execute(params![
                    document,
                    state.revision_id,
                    state.progress,
                    state.mode,
                    state.page_turn_animation,
                    now_ms(),
                    state.locator,
                ])?;
            Ok(())
        })
    }

    /// A setting of the reader's, as the JSON it was saved as.
    pub fn preference(&self, key: &str) -> Result<Option<String>> {
        self.database().read(|connection| {
            Ok(connection
                .prepare_cached("SELECT value FROM preference WHERE key = ?1")?
                .query_row([key], |row| row.get(0))
                .optional()?)
        })
    }

    /// Keeps a setting of the reader's. The value has to be JSON.
    pub fn set_preference(&self, key: &str, value: &str) -> Result<()> {
        if key.is_empty() || key.len() > MAX_PREFERENCE_KEY || value.len() > MAX_PREFERENCE_VALUE {
            return Err(Error::Refused(
                "that setting is too large to keep".to_owned(),
            ));
        }
        if serde_json::from_str::<serde_json::Value>(value).is_err() {
            return Err(Error::Refused("a setting is kept as JSON".to_owned()));
        }
        let (key, value) = (key.to_owned(), value.to_owned());
        self.database().write(move |transaction| {
            transaction
                .prepare_cached(
                    "INSERT INTO preference (key, value, updated_at) VALUES (?1, ?2, ?3)
                     ON CONFLICT (key) DO UPDATE SET value = excluded.value,
                         updated_at = excluded.updated_at",
                )?
                .execute(params![key, value, now_ms()])?;
            Ok(())
        })
    }

    /// The pages of the file with these bytes, if they were measured before. `None` also when there
    /// is no cache, which is never an error: the pages can always be measured again.
    pub fn page_geometry(&self, blake3: &Hash) -> Result<Option<DocumentGeometry>> {
        let Some(cache) = self.cache() else {
            return Ok(None);
        };
        cache.read(|connection| {
            let document: Option<(i64, Option<String>, Option<String>)> = connection
                .prepare_cached(
                    "SELECT page_count, page_layout, direction FROM document_geometry
                     WHERE blake3 = ?1",
                )?
                .query_row([blake3.as_slice()], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .optional()?;
            let Some((page_count, page_layout, direction)) = document else {
                return Ok(None);
            };
            let pages: Vec<PageRecord> = connection
                .prepare_cached(
                    "SELECT width_pt, height_pt, orientation FROM page_geometry
                     WHERE blake3 = ?1 ORDER BY page",
                )?
                .query_map([blake3.as_slice()], |row| {
                    Ok(PageRecord {
                        width_pt: row.get::<_, f64>(0)? as f32,
                        height_pt: row.get::<_, f64>(1)? as f32,
                        orientation: row.get(2)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
            // Both are written together, so a difference means the cache was damaged.
            if pages.len() as i64 != page_count {
                return Ok(None);
            }
            Ok(Some(DocumentGeometry {
                pages,
                page_layout,
                direction,
            }))
        })
    }

    /// What the pages of the file with these bytes are made of, if its pages were measured before.
    /// A page classified by older rules counts as not classified.
    pub fn page_content(&self, blake3: &Hash) -> Result<Option<DocumentContent>> {
        let Some(cache) = self.cache() else {
            return Ok(None);
        };
        cache.read(|connection| {
            let summary: Option<(Option<String>, Option<bool>)> = connection
                .prepare_cached(
                    "SELECT content, is_strictly_uniform FROM document_geometry WHERE blake3 = ?1",
                )?
                .query_row([blake3.as_slice()], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()?;
            let Some((summary, is_strictly_uniform)) = summary else {
                return Ok(None);
            };
            let pages = connection
                .prepare_cached(
                    "SELECT content, text_chars, image_coverage, content_version FROM page_geometry
                     WHERE blake3 = ?1 ORDER BY page",
                )?
                .query_map([blake3.as_slice()], |row| {
                    let class: Option<PageContentClass> = row.get(0)?;
                    let version: Option<i64> = row.get(3)?;
                    Ok(match (class, version) {
                        (Some(class), Some(CONTENT_RULES_VERSION)) => Some(PageContentRecord {
                            class,
                            text_chars: row.get::<_, Option<u32>>(1)?.unwrap_or(0),
                            image_coverage: row.get::<_, Option<f64>>(2)?.unwrap_or(0.0) as f32,
                        }),
                        _ => None,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let complete = pages.iter().all(Option::is_some);
            Ok(Some(DocumentContent {
                pages,
                summary: summary.filter(|_| complete),
                is_strictly_uniform: is_strictly_uniform.filter(|_| complete),
            }))
        })
    }

    /// Records what some pages of the file with these bytes are made of. Once every page is
    /// classified, the file's summary is written too. Pages that were never measured are skipped,
    /// and without a cache this does nothing.
    pub fn store_page_content(
        &self,
        blake3: &Hash,
        pages: &[(u32, PageContentRecord)],
    ) -> Result<()> {
        let Some(cache) = self.cache() else {
            return Ok(());
        };
        for (_, record) in pages {
            if !(record.image_coverage.is_finite() && (0.0..=1.0).contains(&record.image_coverage))
            {
                return Err(Error::Refused(format!(
                    "a page cannot be {} covered",
                    record.image_coverage
                )));
            }
        }
        let blake3 = *blake3;
        let pages = pages.to_vec();
        cache.write(move |transaction| {
            for (page, record) in &pages {
                transaction
                    .prepare_cached(
                        "UPDATE page_geometry SET content = ?3, text_chars = ?4, image_coverage = ?5,
                                                  content_version = ?6
                         WHERE blake3 = ?1 AND page = ?2",
                    )?
                    .execute(params![
                        blake3.as_slice(),
                        i64::from(*page),
                        record.class,
                        record.text_chars,
                        f64::from(record.image_coverage),
                        CONTENT_RULES_VERSION,
                    ])?;
            }
            let (total, classified, text, scanned, hybrid): (i64, i64, i64, i64, i64) = transaction
                .prepare_cached(
                    "SELECT count(*),
                            count(*) FILTER (WHERE content IS NOT NULL AND content_version = ?2),
                            count(*) FILTER (WHERE content = 'text'),
                            count(*) FILTER (WHERE content = 'scanned'),
                            count(*) FILTER (WHERE content = 'hybrid')
                     FROM page_geometry WHERE blake3 = ?1",
                )?
                .query_row(params![blake3.as_slice(), CONTENT_RULES_VERSION], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
                })?;
            let count = |value: i64| u64::try_from(value).unwrap_or(0);
            let summary = content_summary(count(text), count(scanned), count(hybrid));
            if let Some((summary, uniform)) = summary.filter(|_| classified == total) {
                transaction
                    .prepare_cached(
                        "UPDATE document_geometry SET content = ?2, is_strictly_uniform = ?3,
                             text_pages = ?4, scanned_pages = ?5, hybrid_pages = ?6,
                             classified_at = ?7
                         WHERE blake3 = ?1",
                    )?
                    .execute(params![
                        blake3.as_slice(),
                        summary,
                        uniform,
                        text,
                        scanned,
                        hybrid,
                        now_ms()
                    ])?;
            }
            Ok(())
        })
    }

    /// Keeps the pages of the file with these bytes, replacing what was there. Without a cache this
    /// does nothing, since the pages can always be measured again.
    pub fn store_page_geometry(&self, blake3: &Hash, geometry: &DocumentGeometry) -> Result<()> {
        let Some(cache) = self.cache() else {
            return Ok(());
        };
        let blake3 = *blake3;
        let geometry = geometry.clone();
        cache.write(move |transaction| {
            transaction
                .prepare_cached("DELETE FROM page_geometry WHERE blake3 = ?1")?
                .execute([blake3.as_slice()])?;
            for (page, record) in geometry.pages.iter().enumerate() {
                transaction
                    .prepare_cached(
                        "INSERT INTO page_geometry (blake3, page, width_pt, height_pt, orientation)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                    )?
                    .execute(params![
                        blake3.as_slice(),
                        page as i64,
                        f64::from(record.width_pt),
                        f64::from(record.height_pt),
                        record.orientation,
                    ])?;
            }
            transaction
                .prepare_cached(
                    "INSERT OR REPLACE INTO document_geometry
                         (blake3, page_count, summary, portrait, landscape, square, page_layout,
                          direction, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                )?
                .execute(params![
                    blake3.as_slice(),
                    geometry.pages.len() as i64,
                    geometry.summary(),
                    geometry.count(PageOrientation::Portrait),
                    geometry.count(PageOrientation::Landscape),
                    geometry.count(PageOrientation::Square),
                    geometry.page_layout,
                    geometry.direction,
                    now_ms(),
                ])?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(orientation: PageOrientation) -> PageRecord {
        PageRecord {
            width_pt: 1.0,
            height_pt: 1.0,
            orientation,
        }
    }

    #[test]
    fn a_summary_names_the_one_orientation_or_says_mixed() {
        let geometry = |pages: Vec<PageRecord>| DocumentGeometry {
            pages,
            ..DocumentGeometry::default()
        };
        assert_eq!(geometry(vec![]).summary(), "empty");
        assert_eq!(
            geometry(vec![page(PageOrientation::Portrait); 3]).summary(),
            "portrait"
        );
        assert_eq!(
            geometry(vec![
                page(PageOrientation::Portrait),
                page(PageOrientation::Landscape)
            ])
            .summary(),
            "mixed"
        );
    }

    #[test]
    fn only_pages_classified_by_these_rules_count() {
        let content = DocumentContent {
            pages: vec![
                Some(PageContentRecord {
                    class: PageContentClass::Text,
                    text_chars: 10,
                    image_coverage: 0.0,
                }),
                None,
                None,
            ],
            summary: None,
            is_strictly_uniform: None,
        };
        assert_eq!(content.unclassified(), vec![1, 2]);
    }

    #[test]
    fn one_class_names_the_document_from_95_percent_of_its_pages() {
        assert_eq!(content_summary(0, 0, 0), None);
        assert_eq!(content_summary(300, 0, 0), Some(("text", true)));
        assert_eq!(content_summary(0, 12, 0), Some(("scanned", true)));
        // A text book with a scanned cover and plates: 95% exactly is enough.
        assert_eq!(content_summary(19, 1, 0), Some(("text", false)));
        assert_eq!(content_summary(570, 28, 2), Some(("text", false)));
        // Just under 95% is not.
        assert_eq!(content_summary(18, 1, 1), Some(("mixed", false)));
        assert_eq!(content_summary(1162, 0, 88), Some(("mixed", false)));
        // OCR scans whose blank pages have no text layer.
        assert_eq!(content_summary(0, 3, 97), Some(("hybrid", false)));
        // With few pages, one page of another class is already too many.
        assert_eq!(content_summary(9, 1, 0), Some(("mixed", false)));
    }

    #[test]
    fn a_position_outside_the_page_is_refused() {
        let state = ReadingState {
            revision_id: None,
            page: 0,
            u: 0.5,
            v: 0.5,
            progress: 0.0,
            mode: ReadingMode::Paginated,
            page_turn_animation: true,
            zoom: 1.0,
            spreads: Spreads::Auto,
            updated_at: 0,
        };
        assert!(check(&state).is_ok());
        for broken in [
            ReadingState {
                u: 1.5,
                ..state.clone()
            },
            ReadingState {
                v: f64::NAN,
                ..state.clone()
            },
            ReadingState {
                progress: -0.1,
                ..state.clone()
            },
            ReadingState {
                zoom: 0.0,
                ..state.clone()
            },
            ReadingState {
                zoom: f64::INFINITY,
                ..state.clone()
            },
        ] {
            assert!(check(&broken).is_err(), "{broken:?}");
        }
    }
}

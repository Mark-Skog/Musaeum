//! Two works that are one book: what the library suggests, what a person decides, and the merge.
//!
//! The library never merges two works by itself once both exist: it suggests, and the person
//! decides. A merge keeps one work and moves the other's documents into it, each whole, with its
//! revisions, annotations and where the reader was; the other's categories, tags, collections and
//! custom values go along, and so do the fields the person set on it (or that came from Calibre)
//! where the kept work has only what a file said. A field the kept work does not have at all is
//! taken from the other.

use musaeum_i18n::tr;
use std::collections::{BTreeMap, HashSet};

use rusqlite::{OptionalExtension, Transaction, params};
use serde::Serialize;

use crate::content::{merge_empty_work, set_provenance};
use crate::error::{Error, Result};
use crate::library::Library;
use crate::model::{SuggestionInfo, WorkId};
use crate::search;
use crate::sql::{fold, now_ms};

/// The most pairs a review lists.
pub const MAX_DUPLICATES: usize = 500;
/// Works with the same title and author beyond this many are listed in pairs with the first ones.
const MAX_GROUP: usize = 12;

/// Why two works may be one book.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DuplicateReason {
    /// They have an identifier in common.
    SameIdentifier { scheme: String, value: String },
    /// Their titles and first authors are the same, compared as names are.
    SameTitleAndAuthor { title: String, author: String },
}

/// Two works the library thinks may be one, the older first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicatePair {
    pub works: [WorkId; 2],
    pub reason: DuplicateReason,
}

/// A suggestion about two documents, with the works they are in now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestionReview {
    #[serde(flatten)]
    pub suggestion: SuggestionInfo,
    pub work_id: crate::model::WorkId,
    pub candidate_work_id: crate::model::WorkId,
}

/// Everything waiting for a person's word.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    /// Two documents that may be one: the same file that changed, or a file another took the place
    /// of.
    pub suggestions: Vec<SuggestionReview>,
    /// Two works that may be one book.
    pub duplicates: Vec<DuplicatePair>,
}

fn pair(a: WorkId, b: WorkId) -> (WorkId, WorkId) {
    if a < b { (a, b) } else { (b, a) }
}

/// A work with its title and first author, as found.
type Titled = (WorkId, String, String);

fn duplicates(connection: &rusqlite::Connection, limit: usize) -> Result<Vec<DuplicatePair>> {
    let dismissed: HashSet<(WorkId, WorkId)> = connection
        .prepare_cached("SELECT work_a, work_b FROM work_pair_dismissed")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut found: BTreeMap<(WorkId, WorkId), DuplicateReason> = BTreeMap::new();
    // An identifier in common is the strongest sign; `other` is anything at all, so it is not one.
    {
        let mut statement = connection.prepare_cached(
            "SELECT a.work_id, b.work_id, a.scheme, a.value
             FROM work_identifier a JOIN work_identifier b
                 ON b.scheme = a.scheme AND b.value = a.value AND b.work_id <> a.work_id
             WHERE a.scheme <> 'other' AND a.work_id < b.work_id
             ORDER BY a.work_id, b.work_id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, WorkId>(0)?,
                row.get::<_, WorkId>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        for row in rows {
            let (a, b, scheme, value) = row?;
            if !dismissed.contains(&(a, b)) {
                found
                    .entry((a, b))
                    .or_insert(DuplicateReason::SameIdentifier { scheme, value });
            }
        }
    }
    // The same title by the same first author. Without an author, a title alone says too little.
    let mut groups: BTreeMap<(String, String), Vec<Titled>> = BTreeMap::new();
    {
        let mut statement = connection.prepare_cached(
            "SELECT w.id, w.title,
                    (SELECT c.name FROM work_creator x JOIN creator c ON c.id = x.creator_id
                     WHERE x.work_id = w.id ORDER BY x.position LIMIT 1)
             FROM work w ORDER BY w.id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, WorkId>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?;
        for row in rows {
            let (work, title, author) = row?;
            let Some(author) = author else { continue };
            let key = (fold(&title), fold(&author));
            if key.0.is_empty() || key.1.is_empty() {
                continue;
            }
            groups.entry(key).or_default().push((work, title, author));
        }
    }
    for works in groups.values().filter(|works| works.len() > 1) {
        for (index, (a, title, author)) in works.iter().enumerate().take(MAX_GROUP) {
            for (b, _, _) in works.iter().skip(index + 1).take(MAX_GROUP) {
                let key = pair(*a, *b);
                if !dismissed.contains(&key) {
                    found
                        .entry(key)
                        .or_insert_with(|| DuplicateReason::SameTitleAndAuthor {
                            title: title.clone(),
                            author: author.clone(),
                        });
                }
            }
        }
    }
    Ok(found
        .into_iter()
        .take(limit)
        .map(|((a, b), reason)| DuplicatePair {
            works: [a, b],
            reason,
        })
        .collect())
}

/// The fields of a work a merge fills in the kept one when it has none, with the name its
/// provenance is kept under. The names come from this list, never from a caller.
const GAPS: [&str; 6] = [
    "series",
    "series_position",
    "year",
    "language",
    "rating",
    "description",
];

fn fill_gaps(transaction: &Transaction<'_>, keep: WorkId, other: WorkId, now: i64) -> Result<()> {
    for column in GAPS {
        let filled = transaction.execute(
            &format!(
                "UPDATE work SET {column} = (SELECT {column} FROM work WHERE id = ?2)
                 WHERE id = ?1 AND {column} IS NULL
                   AND (SELECT {column} FROM work WHERE id = ?2) IS NOT NULL"
            ),
            params![keep, other],
        )?;
        if filled > 0 {
            let origin: Option<String> = transaction
                .prepare_cached(
                    "SELECT origin FROM field_provenance WHERE work_id = ?1 AND field = ?2",
                )?
                .query_row(params![other, column], |row| row.get(0))
                .optional()?;
            if let Some(origin) = origin {
                set_provenance(transaction, keep, column, &origin, now)?;
            }
        }
    }
    let people = |work: WorkId| -> Result<i64> {
        Ok(transaction
            .prepare_cached("SELECT count(*) FROM work_creator WHERE work_id = ?1")?
            .query_row([work], |row| row.get(0))?)
    };
    if people(keep)? == 0 && people(other)? > 0 {
        transaction
            .prepare_cached("UPDATE work_creator SET work_id = ?1 WHERE work_id = ?2")?
            .execute(params![keep, other])?;
        let origin: Option<String> = transaction
            .prepare_cached(
                "SELECT origin FROM field_provenance WHERE work_id = ?1 AND field = 'creators'",
            )?
            .query_row([other], |row| row.get(0))
            .optional()?;
        if let Some(origin) = origin {
            set_provenance(transaction, keep, "creators", &origin, now)?;
        }
    }
    Ok(())
}

pub(crate) fn merge_into(
    transaction: &Transaction<'_>,
    keep: WorkId,
    others: &[WorkId],
) -> Result<()> {
    let now = now_ms();
    let exists = |work: WorkId| -> Result<bool> {
        Ok(transaction
            .prepare_cached("SELECT 1 FROM work WHERE id = ?1")?
            .query_row([work], |_| Ok(()))
            .optional()?
            .is_some())
    };
    if !exists(keep)? {
        return Err(Error::NotFound(tr!(NoWork, id = keep)));
    }
    for &other in others {
        if other == keep {
            continue;
        }
        if !exists(other)? {
            return Err(Error::NotFound(tr!(NoWork, id = other)));
        }
        fill_gaps(transaction, keep, other, now)?;
        // Each document goes whole: its revisions, annotations and reading state are its own.
        transaction
            .prepare_cached("UPDATE document SET work_id = ?1 WHERE work_id = ?2")?
            .execute(params![keep, other])?;
        merge_empty_work(transaction, other, keep, now)?;
    }
    transaction
        .prepare_cached("UPDATE work SET updated_at = ?2 WHERE id = ?1")?
        .execute(params![keep, now])?;
    search::refresh_entry(transaction, keep)?;
    Ok(())
}

impl Library {
    /// Works that may be one book, for a person to decide on.
    pub fn duplicate_works(&self, limit: usize) -> Result<Vec<DuplicatePair>> {
        let limit = limit.min(MAX_DUPLICATES);
        self.database()
            .read(move |connection| duplicates(connection, limit))
    }

    /// Everything waiting for a person's word: open suggestions about documents, and works that
    /// may be one book.
    pub fn review(&self) -> Result<Review> {
        let suggestions = self.suggestions(true)?;
        self.database().read(move |connection| {
            let mut reviews = Vec::with_capacity(suggestions.len());
            for suggestion in suggestions {
                let work_of = |document| -> Result<Option<WorkId>> {
                    Ok(connection
                        .prepare_cached("SELECT work_id FROM document WHERE id = ?1")?
                        .query_row([document], |row| row.get(0))
                        .optional()?)
                };
                let (Some(work_id), Some(candidate_work_id)) = (
                    work_of(suggestion.document_id)?,
                    work_of(suggestion.candidate_document_id)?,
                ) else {
                    continue;
                };
                reviews.push(SuggestionReview {
                    suggestion,
                    work_id,
                    candidate_work_id,
                });
            }
            Ok(Review {
                suggestions: reviews,
                duplicates: duplicates(connection, MAX_DUPLICATES)?,
            })
        })
    }

    /// These two works are different books: they are not suggested as one again.
    pub fn dismiss_duplicate(&self, a: WorkId, b: WorkId) -> Result<()> {
        if a == b {
            return Err(Error::Refused(tr!(MergeNotAPair)));
        }
        let (a, b) = pair(a, b);
        self.database().write(move |transaction| {
            for work in [a, b] {
                if transaction
                    .prepare_cached("SELECT 1 FROM work WHERE id = ?1")?
                    .query_row([work], |_| Ok(()))
                    .optional()?
                    .is_none()
                {
                    return Err(Error::NotFound(tr!(NoWork, id = work)));
                }
            }
            transaction
                .prepare_cached(
                    "INSERT OR IGNORE INTO work_pair_dismissed (work_a, work_b, created_at) VALUES (?1, ?2, ?3)",
                )?
                .execute(params![a, b, now_ms()])?;
            Ok(())
        })
    }

    /// The others become `keep`: their documents, with everything on them, and what the person put
    /// on the works. Everything or nothing.
    pub fn merge_works(&self, keep: WorkId, others: &[WorkId]) -> Result<WorkId> {
        let mut seen = HashSet::new();
        let others: Vec<WorkId> = others
            .iter()
            .copied()
            .filter(|&work| work != keep && seen.insert(work))
            .collect();
        if others.is_empty() {
            return Err(Error::Refused(tr!(MergeNothing)));
        }
        self.database()
            .write(move |transaction| merge_into(transaction, keep, &others))?;
        Ok(keep)
    }
}

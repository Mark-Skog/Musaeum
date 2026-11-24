//! Many works changed at once, as the panel does when several books are selected: the same fields
//! set on all of them, categories, tags and collections added to or taken from all of them, in one
//! transaction that changes everything or nothing.
//!
//! A field set here becomes the person's on every work, as a single edit makes it: provenance
//! `user`, which no file read again writes over. A field left out keeps what each work had, and its
//! provenance with it.

use musaeum_i18n::{T, tr};
use std::collections::{BTreeMap, HashSet};

use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::library::Library;
use crate::metadata::{Credit, WorkEdit, apply_edit, check_edit, present};
use crate::model::WorkId;
use crate::sql::now_ms;
use crate::taxonomy::{
    self, CategoryId, CollectionId, MAX_TAG_NAME, Place, TagId, put_in_collection,
};

/// The most works one batch changes.
pub const MAX_BATCH: usize = 20_000;

/// A work's own fields a summary compares: id, series, year, language, rating, status.
type ScalarRow = (
    WorkId,
    Option<String>,
    Option<i64>,
    Option<String>,
    Option<i64>,
    String,
);

/// What a person changes on many works at once. A field left out is left as each work has it;
/// `null` clears it on all of them.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchEdit {
    /// Exactly these people, on every work.
    #[serde(default)]
    pub creators: Option<Vec<Credit>>,
    #[serde(default, deserialize_with = "present")]
    pub series: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    pub year: Option<Option<i64>>,
    #[serde(default, deserialize_with = "present")]
    pub language: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    pub rating: Option<Option<i64>>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub add_categories: Vec<CategoryId>,
    #[serde(default)]
    pub remove_categories: Vec<CategoryId>,
    /// By name; one that does not exist yet is made.
    #[serde(default)]
    pub add_tags: Vec<String>,
    #[serde(default)]
    pub remove_tags: Vec<TagId>,
    /// Joined at the end, in the order the works are given.
    #[serde(default)]
    pub add_collections: Vec<CollectionId>,
    #[serde(default)]
    pub remove_collections: Vec<CollectionId>,
}

/// How many of the works have something.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Shared {
    pub id: i64,
    pub works: usize,
}

/// What a set of works has in common, for the panel to show before changing them.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchSummary {
    pub works: usize,
    /// The fields every work has the same value of (`null` when none of them has one), by name:
    /// `creators`, `series`, `year`, `language`, `rating`, `status`. A field the works differ on is
    /// not here.
    pub same: BTreeMap<String, serde_json::Value>,
    pub categories: Vec<Shared>,
    pub tags: Vec<Shared>,
    pub collections: Vec<Shared>,
}

/// The works, each once, in the order given.
fn unique(works: &[WorkId]) -> Result<Vec<WorkId>> {
    let mut seen = HashSet::new();
    let works: Vec<WorkId> = works
        .iter()
        .copied()
        .filter(|work| seen.insert(*work))
        .collect();
    if works.is_empty() {
        return Err(Error::Refused(tr!(BatchNoWork)));
    }
    if works.len() > MAX_BATCH {
        return Err(Error::Refused(tr!(BatchTooMany, max = MAX_BATCH)));
    }
    Ok(works)
}

/// The works as a JSON array, for `json_each`: one statement for any number of them, with no SQL
/// built from them.
fn json_ids(ids: &[i64]) -> String {
    serde_json::to_string(ids).expect("a list of numbers is JSON")
}

fn exists(transaction: &Transaction<'_>, table: &str, id: i64) -> Result<bool> {
    // The table name comes from this module, never from the edit.
    Ok(transaction
        .prepare_cached(&format!("SELECT 1 FROM {table} WHERE id = ?1"))?
        .query_row([id], |_| Ok(()))
        .optional()?
        .is_some())
}

fn apply_batch(
    transaction: &Transaction<'_>,
    works: &[WorkId],
    scalar: &WorkEdit,
    edit: BatchEdit,
) -> Result<()> {
    let now = now_ms();
    let ids = json_ids(works);
    for &work in works {
        if !exists(transaction, "work", work)? {
            return Err(Error::NotFound(tr!(NoWork, id = work)));
        }
        if *scalar != WorkEdit::default() {
            apply_edit(transaction, work, scalar.clone())?;
        }
    }
    for &category in edit.add_categories.iter().chain(&edit.remove_categories) {
        if !exists(transaction, "category", category)? {
            return Err(Error::NotFound(tr!(NoCategory, id = category)));
        }
    }
    for &category in &edit.add_categories {
        transaction
            .prepare_cached(
                "INSERT OR IGNORE INTO work_category (work_id, category_id, added_at)
                 SELECT value, ?2, ?3 FROM json_each(?1)",
            )?
            .execute(params![ids, category, now])?;
    }
    for &category in &edit.remove_categories {
        transaction
            .prepare_cached(
                "DELETE FROM work_category
                 WHERE category_id = ?2 AND work_id IN (SELECT value FROM json_each(?1))",
            )?
            .execute(params![ids, category])?;
    }
    for name in &edit.add_tags {
        let tag = taxonomy::tag_named(transaction, name)?;
        transaction
            .prepare_cached(
                "INSERT OR IGNORE INTO work_tag (work_id, tag_id, added_at)
                 SELECT value, ?2, ?3 FROM json_each(?1)",
            )?
            .execute(params![ids, tag, now])?;
    }
    for &tag in &edit.remove_tags {
        if !exists(transaction, "tag", tag)? {
            return Err(Error::NotFound(tr!(NoTag, id = tag)));
        }
        transaction
            .prepare_cached(
                "DELETE FROM work_tag WHERE tag_id = ?2 AND work_id IN (SELECT value FROM json_each(?1))",
            )?
            .execute(params![ids, tag])?;
    }
    for &collection in edit.add_collections.iter().chain(&edit.remove_collections) {
        if !exists(transaction, "collection", collection)? {
            return Err(Error::NotFound(tr!(NoCollection, id = collection)));
        }
    }
    for &collection in &edit.add_collections {
        let present: HashSet<WorkId> = transaction
            .prepare_cached("SELECT work_id FROM collection_item WHERE collection_id = ?1")?
            .query_map([collection], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let joining: Vec<WorkId> = works
            .iter()
            .copied()
            .filter(|work| !present.contains(work))
            .collect();
        if !joining.is_empty() {
            put_in_collection(transaction, collection, &joining, Place::Last)?;
        }
    }
    for &collection in &edit.remove_collections {
        transaction
            .prepare_cached(
                "DELETE FROM collection_item
                 WHERE collection_id = ?2 AND work_id IN (SELECT value FROM json_each(?1))",
            )?
            .execute(params![ids, collection])?;
    }
    transaction
        .prepare_cached(
            "UPDATE work SET updated_at = ?2 WHERE id IN (SELECT value FROM json_each(?1))",
        )?
        .execute(params![ids, now])?;
    Ok(())
}

/// Checks what can be judged without the database: the fields as a single edit checks them, the
/// tag names, and nothing both added and taken.
fn check_batch(edit: &BatchEdit) -> Result<WorkEdit> {
    let scalar = check_edit(WorkEdit {
        creators: edit.creators.clone(),
        series: edit.series.clone(),
        year: edit.year,
        language: edit.language.clone(),
        rating: edit.rating,
        status: edit.status.clone(),
        ..WorkEdit::default()
    })?;
    for name in &edit.add_tags {
        taxonomy::clean_name(name, MAX_TAG_NAME, T::ThingTag)?;
    }
    let both = |added: &[i64], taken: &[i64]| added.iter().any(|id| taken.contains(id));
    if both(&edit.add_categories, &edit.remove_categories)
        || both(&edit.add_collections, &edit.remove_collections)
    {
        return Err(Error::Refused(tr!(BatchAddAndTake)));
    }
    Ok(scalar)
}

impl Library {
    /// Changes many works at once: everything or nothing. Returns how many works it changed.
    pub fn edit_works(&self, works: &[WorkId], edit: BatchEdit) -> Result<usize> {
        let works = unique(works)?;
        let scalar = check_batch(&edit)?;
        let count = works.len();
        self.database()
            .write(move |transaction| apply_batch(transaction, &works, &scalar, edit))?;
        Ok(count)
    }

    /// What these works have in common.
    pub fn works_summary(&self, works: &[WorkId]) -> Result<BatchSummary> {
        let works = unique(works)?;
        self.database().read(move |connection| {
            let ids = json_ids(&works);
            let rows: Vec<ScalarRow> = connection
                .prepare_cached(
                    "SELECT id, series, year, language, rating, status FROM work
                     WHERE id IN (SELECT value FROM json_each(?1))",
                )?
                .query_map([&ids], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?))
                })?
                .collect::<rusqlite::Result<_>>()?;
            if rows.len() != works.len() {
                return Err(Error::NotFound(tr!(BatchWorksGone)));
            }
            let mut creators: BTreeMap<WorkId, Vec<Credit>> = works.iter().map(|&work| (work, Vec::new())).collect();
            {
                let mut statement = connection.prepare_cached(
                    "SELECT x.work_id, c.name, x.role FROM work_creator x JOIN creator c ON c.id = x.creator_id
                     WHERE x.work_id IN (SELECT value FROM json_each(?1)) ORDER BY x.work_id, x.position",
                )?;
                let found = statement.query_map([&ids], |row| {
                    Ok((row.get::<_, WorkId>(0)?, Credit { name: row.get(1)?, role: row.get(2)? }))
                })?;
                for row in found {
                    let (work, credit) = row?;
                    creators.entry(work).or_default().push(credit);
                }
            }
            let mut same = BTreeMap::new();
            let mut keep = |name: &str, values: Vec<serde_json::Value>| {
                if let Some(first) = values.first()
                    && values.iter().all(|value| value == first)
                {
                    same.insert(name.to_owned(), first.clone());
                }
            };
            let json = |value: serde_json::Result<serde_json::Value>| value.unwrap_or(serde_json::Value::Null);
            keep("creators", creators.values().map(|list| json(serde_json::to_value(list))).collect());
            keep("series", rows.iter().map(|row| json(serde_json::to_value(&row.1))).collect());
            keep("year", rows.iter().map(|row| json(serde_json::to_value(row.2))).collect());
            keep("language", rows.iter().map(|row| json(serde_json::to_value(&row.3))).collect());
            keep("rating", rows.iter().map(|row| json(serde_json::to_value(row.4))).collect());
            keep("status", rows.iter().map(|row| json(serde_json::to_value(&row.5))).collect());
            let counts = |sql: &str| -> Result<Vec<Shared>> {
                Ok(connection
                    .prepare_cached(sql)?
                    .query_map([&ids], |row| Ok(Shared { id: row.get(0)?, works: row.get::<_, i64>(1)? as usize }))?
                    .collect::<rusqlite::Result<_>>()?)
            };
            Ok(BatchSummary {
                works: works.len(),
                same,
                categories: counts(
                    "SELECT x.category_id, count(*) FROM work_category x JOIN category c ON c.id = x.category_id
                     WHERE x.work_id IN (SELECT value FROM json_each(?1)) GROUP BY x.category_id ORDER BY min(c.path)",
                )?,
                tags: counts(
                    "SELECT x.tag_id, count(*) FROM work_tag x JOIN tag t ON t.id = x.tag_id
                     WHERE x.work_id IN (SELECT value FROM json_each(?1)) GROUP BY x.tag_id ORDER BY min(t.name_key)",
                )?,
                collections: counts(
                    "SELECT x.collection_id, count(*) FROM collection_item x JOIN collection c ON c.id = x.collection_id
                     WHERE x.work_id IN (SELECT value FROM json_each(?1)) GROUP BY x.collection_id
                     ORDER BY min(c.sort_key), x.collection_id",
                )?,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_batch_is_checked_before_it_reaches_the_database() {
        assert!(
            check_batch(&BatchEdit {
                rating: Some(Some(9)),
                ..BatchEdit::default()
            })
            .is_err()
        );
        assert!(
            check_batch(&BatchEdit {
                add_tags: vec!["   ".into()],
                ..BatchEdit::default()
            })
            .is_err()
        );
        assert!(
            check_batch(&BatchEdit {
                add_categories: vec![3],
                remove_categories: vec![3],
                ..BatchEdit::default()
            })
            .is_err()
        );
        let scalar = check_batch(&BatchEdit {
            language: Some(Some("PT-br".into())),
            ..BatchEdit::default()
        })
        .unwrap();
        assert_eq!(scalar.language, Some(Some("pt-BR".into())));
        assert!(unique(&[]).is_err());
        assert_eq!(unique(&[3, 1, 3, 2]).unwrap(), [3, 1, 2]);
    }

    #[test]
    fn absent_and_null_mean_different_things_in_a_batch_too() {
        let edit: BatchEdit =
            serde_json::from_str(r#"{"series": null, "addTags": ["Clássico"]}"#).unwrap();
        assert_eq!(edit.series, Some(None));
        assert_eq!(edit.year, None);
        assert!(serde_json::from_str::<BatchEdit>(r#"{"title": "x"}"#).is_err());
    }
}

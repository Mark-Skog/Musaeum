//! How a person organizes the library beside the folders on the disk: a tree of categories, tags,
//! collections kept by hand in an order of their own, smart collections kept by a rule, and the
//! custom fields a work may have. All of it is the person's own: nothing read from a file writes
//! here, and nothing here is ever written to a file.
//!
//! A category keeps the ids of the path from the top of the tree down to itself, `/3/14/15/`, so a
//! category and everything under it are the rows whose path starts with its own, and a subtree
//! moves by rewriting that prefix in one statement. A category never moves under itself or under
//! one of its own. Siblings, collections, the books of a collection and custom fields keep the
//! order the person gave them by a fractional key ([`crate::order`]), so moving one changes one
//! row. Names are compared folded ([`crate::sql::fold`]): "Física" and "fisica" are the same name,
//! and two siblings, two tags or two custom fields may not share one.

use musaeum_i18n::{T, text, tr};
use std::collections::BTreeMap;

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::library::Library;
use crate::model::WorkId;
use crate::order;
use crate::smart::{self, Rules};
use crate::sql::{fold, now_ms};

pub type CategoryId = i64;
pub type TagId = i64;
pub type CollectionId = i64;
pub type SmartCollectionId = i64;
pub type CustomFieldId = i64;

/// The most levels of categories under the top.
pub const MAX_CATEGORY_DEPTH: usize = 16;
/// The longest names.
pub const MAX_NAME: usize = 200;
pub const MAX_TAG_NAME: usize = 100;
/// The longest description of a collection.
pub const MAX_DESCRIPTION: usize = 4000;
/// The most choices of a custom field.
pub const MAX_CHOICES: usize = 200;

/// The colors a tag may have, as the annotations have them.
pub const TAG_COLORS: [&str; 7] = [
    "yellow", "green", "blue", "pink", "purple", "orange", "gray",
];
/// The kinds a custom field may be of.
pub const FIELD_KINDS: [&str; 6] = ["text", "number", "date", "boolean", "choice", "url"];

/// Where an item goes among the others of its list: first, last, or just before or after one of
/// them, by id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "at", content = "id", rename_all = "camelCase")]
pub enum Place {
    First,
    Last,
    Before(i64),
    After(i64),
}

/// A category, as the tree lists it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Category {
    pub id: CategoryId,
    pub parent_id: Option<CategoryId>,
    pub name: String,
    /// How deep it is: 0 at the top.
    pub depth: usize,
    /// Works put in it directly.
    pub works: u64,
    /// Works in it or in a category under it, each once.
    pub total: u64,
}

/// A tag, with how many works have it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tag {
    pub id: TagId,
    pub name: String,
    pub color: Option<String>,
    pub works: u64,
}

/// A collection kept by hand.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Collection {
    pub id: CollectionId,
    pub name: String,
    pub description: String,
    pub works: u64,
}

/// A collection kept by a rule.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SmartCollection {
    pub id: SmartCollectionId,
    pub name: String,
    pub rules: Rules,
    /// How many works match it now; none if it cannot be worked out.
    pub works: u64,
    /// Why its rule cannot be worked out now: a custom field it names was removed, say.
    pub broken: Option<String>,
}

/// A field a person defined.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomField {
    pub id: CustomFieldId,
    pub name: String,
    pub kind: String,
    /// For a `choice` field, its choices, in order.
    pub choices: Vec<String>,
}

/// A name as typed, trimmed, checked.
pub(crate) fn clean_name(name: &str, max: usize, what: T) -> Result<String> {
    let name = name.trim();
    let what = text(what, &[]);
    if name.is_empty() {
        return Err(Error::Refused(tr!(NameNeeded, what = what)));
    }
    if name.chars().count() > max {
        return Err(Error::Refused(tr!(NameTooLong, what = what, max = max)));
    }
    if name.chars().any(char::is_control) {
        return Err(Error::Refused(tr!(NameControl, what = what)));
    }
    Ok(name.to_owned())
}

/// A key for an item put at `place` among `siblings`, in their order, which do not hold the item.
pub(crate) fn key_for(siblings: &[(i64, String)], place: Place) -> Result<String> {
    let at = |id: i64| -> Result<usize> {
        siblings
            .iter()
            .position(|(sibling, _)| *sibling == id)
            .ok_or_else(|| Error::Refused(tr!(NotInList, id = id)))
    };
    let key = |index: usize| siblings.get(index).map(|(_, key)| key.as_str());
    let result = match place {
        Place::First => order::between(None, key(0)),
        Place::Last => order::between(siblings.last().map(|(_, key)| key.as_str()), None),
        Place::Before(id) => {
            let index = at(id)?;
            order::between(index.checked_sub(1).and_then(key), key(index))
        }
        Place::After(id) => {
            let index = at(id)?;
            order::between(key(index), key(index + 1))
        }
    };
    result.map_err(|error| Error::Refused(error.to_string()))
}

/// The rows of a query `(id, sort_key)`, in order.
fn ordered(
    connection: &Connection,
    sql: &str,
    parameter: Option<i64>,
) -> Result<Vec<(i64, String)>> {
    let mut statement = connection.prepare_cached(sql)?;
    let rows = statement
        .query_map([parameter], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn constraint_to(error: rusqlite::Error, message: impl FnOnce() -> String) -> Error {
    match &error {
        rusqlite::Error::SqliteFailure(failure, _)
            if failure.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            Error::Refused(message())
        }
        _ => Error::Database(error),
    }
}

fn exists(connection: &Connection, table: &str, id: i64) -> Result<bool> {
    // `table` is one of the literals of this module.
    Ok(connection
        .prepare_cached(&format!("SELECT 1 FROM {table} WHERE id = ?1"))?
        .query_row([id], |_| Ok(()))
        .optional()?
        .is_some())
}

fn must_exist(connection: &Connection, table: &str, id: i64, missing: T) -> Result<()> {
    if exists(connection, table, id)? {
        Ok(())
    } else {
        Err(Error::NotFound(text(missing, &[("id", &id)])))
    }
}

fn works_exist(connection: &Connection, works: &[WorkId]) -> Result<()> {
    for &work in works {
        must_exist(connection, "work", work, T::NoWork)?;
    }
    Ok(())
}

/// The path of a category, and how deep it is.
fn path_of(connection: &Connection, id: CategoryId) -> Result<String> {
    connection
        .prepare_cached("SELECT path FROM category WHERE id = ?1")?
        .query_row([id], |row| row.get(0))
        .optional()?
        .ok_or_else(|| Error::NotFound(tr!(NoCategory, id = id)))
}

fn depth_of(path: &str) -> usize {
    path.matches('/').count().saturating_sub(2)
}

fn sibling_categories(
    connection: &Connection,
    parent: Option<CategoryId>,
    leaving: Option<CategoryId>,
) -> Result<Vec<(i64, String)>> {
    let mut siblings = ordered(
        connection,
        "SELECT id, sort_key FROM category WHERE parent_id IS ?1 ORDER BY sort_key, id",
        parent,
    )?;
    siblings.retain(|(id, _)| Some(*id) != leaving);
    Ok(siblings)
}

pub(crate) fn categories(connection: &Connection) -> Result<Vec<Category>> {
    struct Row {
        id: CategoryId,
        parent_id: Option<CategoryId>,
        name: String,
        path: String,
        sort_key: String,
        works: u64,
        total: u64,
    }
    let mut rows: Vec<Row> = connection
        .prepare_cached("SELECT id, parent_id, name, path, sort_key FROM category")?
        .query_map([], |row| {
            Ok(Row {
                id: row.get(0)?,
                parent_id: row.get(1)?,
                name: row.get(2)?,
                path: row.get(3)?,
                sort_key: row.get(4)?,
                works: 0,
                total: 0,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    // Each work counts once in a category and in every category above it: one pass over the
    // links, each walking up its category's path.
    let index: std::collections::HashMap<CategoryId, usize> = rows
        .iter()
        .enumerate()
        .map(|(at, row)| (row.id, at))
        .collect();
    let mut below: Vec<std::collections::HashSet<WorkId>> =
        vec![std::collections::HashSet::new(); rows.len()];
    let links: Vec<(CategoryId, WorkId)> = connection
        .prepare_cached("SELECT category_id, work_id FROM work_category")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (category, work) in links {
        let Some(&at) = index.get(&category) else {
            continue;
        };
        rows[at].works += 1;
        let path = rows[at].path.clone();
        for ancestor in path
            .split('/')
            .filter_map(|part| part.parse::<CategoryId>().ok())
        {
            if let Some(&up) = index.get(&ancestor) {
                below[up].insert(work);
            }
        }
    }
    for (row, works) in rows.iter_mut().zip(&below) {
        row.total = works.len() as u64;
    }
    // Depth first, each level in the person's order.
    let mut children: BTreeMap<Option<CategoryId>, Vec<usize>> = BTreeMap::new();
    for (index, row) in rows.iter().enumerate() {
        children.entry(row.parent_id).or_default().push(index);
    }
    for list in children.values_mut() {
        list.sort_by(|&a, &b| {
            rows[a]
                .sort_key
                .cmp(&rows[b].sort_key)
                .then(rows[a].id.cmp(&rows[b].id))
        });
    }
    let mut out = Vec::with_capacity(rows.len());
    let mut stack: Vec<usize> = children.get(&None).cloned().unwrap_or_default();
    stack.reverse();
    while let Some(index) = stack.pop() {
        let row = &rows[index];
        out.push(Category {
            id: row.id,
            parent_id: row.parent_id,
            name: row.name.clone(),
            depth: depth_of(&row.path),
            works: row.works,
            total: row.total,
        });
        if let Some(list) = children.get(&Some(row.id)) {
            stack.extend(list.iter().rev());
        }
    }
    Ok(out)
}

fn insert_category(
    transaction: &Transaction<'_>,
    parent: Option<CategoryId>,
    name: &str,
    place: Place,
) -> Result<CategoryId> {
    let name = clean_name(name, MAX_NAME, T::ThingCategory)?;
    let parent_path = match parent {
        Some(parent) => path_of(transaction, parent)?,
        None => "/".to_owned(),
    };
    if depth_of(&parent_path) + usize::from(parent.is_some()) >= MAX_CATEGORY_DEPTH {
        return Err(Error::Refused(tr!(
            CategoriesTooDeep,
            max = MAX_CATEGORY_DEPTH
        )));
    }
    let siblings = sibling_categories(transaction, parent, None)?;
    let key = key_for(&siblings, place)?;
    let id: CategoryId = transaction
        .prepare_cached("SELECT coalesce(max(id), 0) + 1 FROM category")?
        .query_row([], |row| row.get(0))?;
    let now = now_ms();
    transaction
        .prepare_cached(
            "INSERT INTO category (id, parent_id, name, name_key, path, sort_key, created_at,
                 updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
        )?
        .execute(params![
            id,
            parent,
            name,
            fold(&name),
            format!("{parent_path}{id}/"),
            key,
            now
        ])
        .map_err(|error| constraint_to(error, || tr!(CategoryNameTaken, name = name)))?;
    Ok(id)
}

/// Removes works from the categories in `ids`, and those categories, children before parents.
fn remove_categories(transaction: &Transaction<'_>, path: &str) -> Result<()> {
    transaction
        .prepare_cached(
            "DELETE FROM work_category WHERE category_id IN
                 (SELECT id FROM category WHERE substr(path, 1, length(?1)) = ?1)",
        )?
        .execute([path])?;
    let doomed: Vec<CategoryId> = transaction
        .prepare_cached(
            "SELECT id FROM category WHERE substr(path, 1, length(?1)) = ?1
             ORDER BY length(path) DESC",
        )?
        .query_map([path], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for id in doomed {
        transaction
            .prepare_cached("DELETE FROM category WHERE id = ?1")?
            .execute([id])?;
    }
    Ok(())
}

pub(crate) fn tags(connection: &Connection) -> Result<Vec<Tag>> {
    let mut tags: Vec<(String, Tag)> = connection
        .prepare_cached(
            "SELECT t.id, t.name, t.color, t.name_key,
                    (SELECT count(*) FROM work_tag x WHERE x.tag_id = t.id)
             FROM tag t",
        )?
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(3)?,
                Tag {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    color: row.get(2)?,
                    works: row.get::<_, i64>(4)?.max(0) as u64,
                },
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    tags.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(tags.into_iter().map(|(_, tag)| tag).collect())
}

fn check_color(color: Option<&str>) -> Result<()> {
    match color {
        Some(color) if !TAG_COLORS.contains(&color) => {
            Err(Error::Refused(tr!(TagColorUnknown, color = color)))
        }
        _ => Ok(()),
    }
}

/// The tag named `name`, made if there is none.
pub(crate) fn tag_named(transaction: &Transaction<'_>, name: &str) -> Result<TagId> {
    let name = clean_name(name, MAX_TAG_NAME, T::ThingTag)?;
    let key = fold(&name);
    if let Some(id) = transaction
        .prepare_cached("SELECT id FROM tag WHERE name_key = ?1")?
        .query_row([&key], |row| row.get(0))
        .optional()?
    {
        return Ok(id);
    }
    transaction
        .prepare_cached("INSERT INTO tag (name, name_key, created_at) VALUES (?1, ?2, ?3)")?
        .execute(params![name, key, now_ms()])?;
    Ok(transaction.last_insert_rowid())
}

fn collection_items(
    connection: &Connection,
    collection: CollectionId,
) -> Result<Vec<(i64, String)>> {
    ordered(
        connection,
        "SELECT work_id, sort_key FROM collection_item WHERE collection_id = ?1
         ORDER BY sort_key, work_id",
        Some(collection),
    )
}

/// Puts works in a collection at `place`, in the order given; works already in it move there.
pub(crate) fn put_in_collection(
    transaction: &Transaction<'_>,
    collection: CollectionId,
    works: &[WorkId],
    place: Place,
) -> Result<()> {
    let mut items = collection_items(transaction, collection)?;
    items.retain(|(work, _)| !works.contains(work));
    let now = now_ms();
    // Each after the one before it.
    let mut place = place;
    for &work in works {
        let key = key_for(&items, place)?;
        let at = match place {
            Place::First => 0,
            Place::Last => items.len(),
            Place::Before(id) => items.iter().position(|(item, _)| *item == id).unwrap_or(0),
            Place::After(id) => items
                .iter()
                .position(|(item, _)| *item == id)
                .map_or(items.len(), |index| index + 1),
        };
        transaction
            .prepare_cached(
                "INSERT INTO collection_item (collection_id, work_id, sort_key, added_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (collection_id, work_id) DO UPDATE SET sort_key = excluded.sort_key",
            )?
            .execute(params![collection, work, key, now])?;
        items.insert(at, (work, key));
        place = Place::After(work);
    }
    transaction
        .prepare_cached("UPDATE collection SET updated_at = ?2 WHERE id = ?1")?
        .execute(params![collection, now])?;
    Ok(())
}

pub(crate) fn collections(connection: &Connection) -> Result<Vec<Collection>> {
    Ok(connection
        .prepare_cached(
            "SELECT c.id, c.name, c.description,
                    (SELECT count(*) FROM collection_item i WHERE i.collection_id = c.id)
             FROM collection c ORDER BY c.sort_key, c.id",
        )?
        .query_map([], |row| {
            Ok(Collection {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                works: row.get::<_, i64>(3)?.max(0) as u64,
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
}

/// The kind of every custom field, for checking and compiling rules.
pub(crate) fn custom_kinds(connection: &Connection) -> Result<smart::CustomKinds> {
    let rows: Vec<(i64, String)> = connection
        .prepare_cached("SELECT id, kind FROM custom_field")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, kind)| {
            FIELD_KINDS
                .iter()
                .find(|known| **known == kind)
                .map(|known| (id, *known))
        })
        .collect())
}

/// The works a smart collection's rule matches now, as SQL over `work w` and its parameters.
pub(crate) fn smart_condition(
    connection: &Connection,
    id: SmartCollectionId,
) -> Result<(String, Vec<rusqlite::types::Value>)> {
    let rules: String = connection
        .prepare_cached("SELECT rules FROM smart_collection WHERE id = ?1")?
        .query_row([id], |row| row.get(0))
        .optional()?
        .ok_or_else(|| Error::NotFound(tr!(NoSmartCollection, id = id)))?;
    let rules: Rules = serde_json::from_str(&rules)?;
    smart::compile(&rules, &custom_kinds(connection)?, now_ms())
}

fn smart_count(connection: &Connection, id: SmartCollectionId) -> Result<u64> {
    let (condition, values) = smart_condition(connection, id)?;
    let count: i64 = connection
        .prepare(&format!("SELECT count(*) FROM work w WHERE {condition}"))?
        .query_row(rusqlite::params_from_iter(values), |row| row.get(0))?;
    Ok(count.max(0) as u64)
}

pub(crate) fn smart_collections(connection: &Connection) -> Result<Vec<SmartCollection>> {
    let rows: Vec<(SmartCollectionId, String, String)> = connection
        .prepare_cached("SELECT id, name, rules FROM smart_collection ORDER BY sort_key, id")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    rows.into_iter()
        .map(|(id, name, rules)| {
            let rules: Rules = serde_json::from_str(&rules)?;
            let (works, broken) = match smart_count(connection, id) {
                Ok(count) => (count, None),
                Err(Error::Refused(why)) => (0, Some(why)),
                Err(error) => return Err(error),
            };
            Ok(SmartCollection {
                id,
                name,
                rules,
                works,
                broken,
            })
        })
        .collect()
}

pub(crate) fn custom_fields(connection: &Connection) -> Result<Vec<CustomField>> {
    let rows: Vec<(CustomFieldId, String, String, String)> = connection
        .prepare_cached("SELECT id, name, kind, choices FROM custom_field ORDER BY sort_key, id")?
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    rows.into_iter()
        .map(|(id, name, kind, choices)| {
            Ok(CustomField {
                id,
                name,
                kind,
                choices: serde_json::from_str(&choices)?,
            })
        })
        .collect()
}

/// Makes a custom field, last in the order, with a name and choices already checked.
pub(crate) fn insert_custom_field(
    transaction: &Transaction<'_>,
    name: &str,
    kind: &str,
    choices: &[String],
) -> Result<CustomFieldId> {
    let siblings = ordered(
        transaction,
        "SELECT id, sort_key FROM custom_field WHERE ?1 IS NULL ORDER BY sort_key, id",
        None,
    )?;
    let key = key_for(&siblings, Place::Last)?;
    transaction
        .prepare_cached(
            "INSERT INTO custom_field (name, name_key, kind, choices, sort_key, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?
        .execute(params![
            name,
            fold(name),
            kind,
            serde_json::to_string(choices)?,
            key,
            now_ms()
        ])
        .map_err(|error| constraint_to(error, || tr!(FieldNameTaken, name = name)))?;
    Ok(transaction.last_insert_rowid())
}

pub(crate) fn clean_choices(kind: &str, choices: &[String]) -> Result<Vec<String>> {
    if kind != "choice" {
        if choices.is_empty() {
            return Ok(Vec::new());
        }
        return Err(Error::Refused(tr!(FieldNoChoices, kind = kind)));
    }
    if choices.is_empty() || choices.len() > MAX_CHOICES {
        return Err(Error::Refused(tr!(ChoiceCount, max = MAX_CHOICES)));
    }
    let mut seen = std::collections::HashSet::new();
    choices
        .iter()
        .map(|choice| {
            let choice = clean_name(choice, MAX_NAME, T::ThingChoice)?;
            if !seen.insert(fold(&choice)) {
                return Err(Error::Refused(tr!(ChoiceTwice, choice = choice)));
            }
            Ok(choice)
        })
        .collect()
}

impl Library {
    /// Every category, depth first, each level in the person's order.
    pub fn categories(&self) -> Result<Vec<Category>> {
        self.database().read(categories)
    }

    /// Makes a category under `parent`, or at the top.
    pub fn create_category(
        &self,
        parent: Option<CategoryId>,
        name: &str,
        place: Place,
    ) -> Result<CategoryId> {
        let name = name.to_owned();
        self.database()
            .write(move |transaction| insert_category(transaction, parent, &name, place))
    }

    /// Renames a category.
    pub fn rename_category(&self, id: CategoryId, name: &str) -> Result<()> {
        let name = clean_name(name, MAX_NAME, T::ThingCategory)?;
        self.database().write(move |transaction| {
            let changed = transaction
                .prepare_cached(
                    "UPDATE category SET name = ?2, name_key = ?3, updated_at = ?4 WHERE id = ?1",
                )?
                .execute(params![id, name, fold(&name), now_ms()])
                .map_err(|error| constraint_to(error, || tr!(CategoryNameTaken, name = name)))?;
            if changed == 0 {
                return Err(Error::NotFound(tr!(NoCategory, id = id)));
            }
            Ok(())
        })
    }

    /// Moves a category, with everything under it, under `parent` (or to the top) at `place`
    /// among its new siblings. A category never goes under itself or under one of its own.
    pub fn move_category(
        &self,
        id: CategoryId,
        parent: Option<CategoryId>,
        place: Place,
    ) -> Result<()> {
        self.database().write(move |transaction| {
            let old = path_of(transaction, id)?;
            let parent_path = match parent {
                Some(parent) => path_of(transaction, parent)?,
                None => "/".to_owned(),
            };
            if parent_path.starts_with(&old) {
                return Err(Error::Refused(
                    tr!(CategoryUnderItself),
                ));
            }
            let new = format!("{parent_path}{id}/");
            // How deep the subtree reaches below the category.
            let below: i64 = transaction
                .prepare_cached(
                    "SELECT coalesce(max(length(path) - length(replace(path, '/', ''))), 0)
                     FROM category WHERE substr(path, 1, length(?1)) = ?1",
                )?
                .query_row([&old], |row| row.get(0))?;
            let below = (below as usize).saturating_sub(old.matches('/').count());
            if depth_of(&new) + below >= MAX_CATEGORY_DEPTH {
                return Err(Error::Refused(tr!(CategoriesTooDeep, max = MAX_CATEGORY_DEPTH)));
            }
            let siblings = sibling_categories(transaction, parent, Some(id))?;
            let key = key_for(&siblings, place)?;
            let now = now_ms();
            transaction
                .prepare_cached(
                    "UPDATE category SET parent_id = ?2, sort_key = ?3, updated_at = ?4 WHERE id = ?1",
                )?
                .execute(params![id, parent, key, now])
                .map_err(|error| {
                    constraint_to(error, || {
                        tr!(CategoryNameTakenThere)
                    })
                })?;
            if new != old {
                transaction
                    .prepare_cached(
                        "UPDATE category SET path = ?2 || substr(path, length(?1) + 1), updated_at = ?3
                         WHERE substr(path, 1, length(?1)) = ?1",
                    )?
                    .execute(params![old, new, now])?;
            }
            Ok(())
        })
    }

    /// Removes a category and every category under it. The works in them stay in the library,
    /// and in their other categories.
    pub fn delete_category(&self, id: CategoryId) -> Result<()> {
        self.database().write(move |transaction| {
            let path = path_of(transaction, id)?;
            remove_categories(transaction, &path)
        })
    }

    /// Puts works in a category, or takes them out of it.
    pub fn set_category(&self, works: &[WorkId], category: CategoryId, member: bool) -> Result<()> {
        let works = works.to_vec();
        self.database().write(move |transaction| {
            must_exist(transaction, "category", category, T::NoCategory)?;
            works_exist(transaction, &works)?;
            let now = now_ms();
            for work in works {
                if member {
                    transaction
                        .prepare_cached(
                            "INSERT OR IGNORE INTO work_category (work_id, category_id, added_at)
                             VALUES (?1, ?2, ?3)",
                        )?
                        .execute(params![work, category, now])?;
                } else {
                    transaction
                        .prepare_cached(
                            "DELETE FROM work_category WHERE work_id = ?1 AND category_id = ?2",
                        )?
                        .execute(params![work, category])?;
                }
            }
            Ok(())
        })
    }

    /// Every tag, by name.
    pub fn tags(&self) -> Result<Vec<Tag>> {
        self.database().read(tags)
    }

    /// Makes a tag.
    pub fn create_tag(&self, name: &str, color: Option<&str>) -> Result<TagId> {
        check_color(color)?;
        let name = clean_name(name, MAX_TAG_NAME, T::ThingTag)?;
        let color = color.map(str::to_owned);
        self.database().write(move |transaction| {
            transaction
                .prepare_cached(
                    "INSERT INTO tag (name, name_key, color, created_at) VALUES (?1, ?2, ?3, ?4)",
                )?
                .execute(params![name, fold(&name), color, now_ms()])
                .map_err(|error| constraint_to(error, || tr!(TagNameTaken, name = name)))?;
            Ok(transaction.last_insert_rowid())
        })
    }

    /// Renames a tag, or changes its color; `Some(None)` takes its color away.
    pub fn update_tag(
        &self,
        id: TagId,
        name: Option<&str>,
        color: Option<Option<&str>>,
    ) -> Result<()> {
        if let Some(color) = color {
            check_color(color)?;
        }
        let name = name
            .map(|name| clean_name(name, MAX_TAG_NAME, T::ThingTag))
            .transpose()?;
        let color = color.map(|color| color.map(str::to_owned));
        self.database().write(move |transaction| {
            must_exist(transaction, "tag", id, T::NoTag)?;
            if let Some(name) = &name {
                transaction
                    .prepare_cached("UPDATE tag SET name = ?2, name_key = ?3 WHERE id = ?1")?
                    .execute(params![id, name, fold(name)])
                    .map_err(|error| constraint_to(error, || tr!(TagNameTaken, name = name)))?;
            }
            if let Some(color) = color {
                transaction
                    .prepare_cached("UPDATE tag SET color = ?2 WHERE id = ?1")?
                    .execute(params![id, color])?;
            }
            Ok(())
        })
    }

    /// Removes a tag from every work, and the tag.
    pub fn delete_tag(&self, id: TagId) -> Result<()> {
        self.database().write(move |transaction| {
            must_exist(transaction, "tag", id, T::NoTag)?;
            transaction
                .prepare_cached("DELETE FROM work_tag WHERE tag_id = ?1")?
                .execute([id])?;
            transaction
                .prepare_cached("DELETE FROM tag WHERE id = ?1")?
                .execute([id])?;
            Ok(())
        })
    }

    /// Gives works a tag, or takes it away.
    pub fn set_tag(&self, works: &[WorkId], tag: TagId, member: bool) -> Result<()> {
        let works = works.to_vec();
        self.database().write(move |transaction| {
            must_exist(transaction, "tag", tag, T::NoTag)?;
            works_exist(transaction, &works)?;
            let now = now_ms();
            for work in works {
                if member {
                    transaction
                        .prepare_cached(
                            "INSERT OR IGNORE INTO work_tag (work_id, tag_id, added_at)
                             VALUES (?1, ?2, ?3)",
                        )?
                        .execute(params![work, tag, now])?;
                } else {
                    transaction
                        .prepare_cached("DELETE FROM work_tag WHERE work_id = ?1 AND tag_id = ?2")?
                        .execute(params![work, tag])?;
                }
            }
            Ok(())
        })
    }

    /// Every collection, in the person's order.
    pub fn collections(&self) -> Result<Vec<Collection>> {
        self.database().read(collections)
    }

    /// Makes a collection at `place` among the others.
    pub fn create_collection(
        &self,
        name: &str,
        description: &str,
        place: Place,
    ) -> Result<CollectionId> {
        let name = clean_name(name, MAX_NAME, T::ThingCollection)?;
        let description = clean_description(description)?;
        self.database().write(move |transaction| {
            let siblings = ordered(
                transaction,
                "SELECT id, sort_key FROM collection WHERE ?1 IS NULL ORDER BY sort_key, id",
                None,
            )?;
            let key = key_for(&siblings, place)?;
            let now = now_ms();
            transaction
                .prepare_cached(
                    "INSERT INTO collection (name, description, sort_key, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?4)",
                )?
                .execute(params![name, description, key, now])?;
            Ok(transaction.last_insert_rowid())
        })
    }

    /// Renames a collection, or describes it again.
    pub fn update_collection(
        &self,
        id: CollectionId,
        name: Option<&str>,
        description: Option<&str>,
    ) -> Result<()> {
        let name = name
            .map(|name| clean_name(name, MAX_NAME, T::ThingCollection))
            .transpose()?;
        let description = description.map(clean_description).transpose()?;
        self.database().write(move |transaction| {
            must_exist(transaction, "collection", id, T::NoCollection)?;
            transaction
                .prepare_cached(
                    "UPDATE collection SET name = coalesce(?2, name),
                         description = coalesce(?3, description), updated_at = ?4
                     WHERE id = ?1",
                )?
                .execute(params![id, name, description, now_ms()])?;
            Ok(())
        })
    }

    /// Moves a collection among the others.
    pub fn move_collection(&self, id: CollectionId, place: Place) -> Result<()> {
        self.database().write(move |transaction| {
            must_exist(transaction, "collection", id, T::NoCollection)?;
            let mut siblings = ordered(
                transaction,
                "SELECT id, sort_key FROM collection WHERE ?1 IS NULL ORDER BY sort_key, id",
                None,
            )?;
            siblings.retain(|(sibling, _)| *sibling != id);
            let key = key_for(&siblings, place)?;
            transaction
                .prepare_cached("UPDATE collection SET sort_key = ?2 WHERE id = ?1")?
                .execute(params![id, key])?;
            Ok(())
        })
    }

    /// Removes a collection. Its works stay in the library.
    pub fn delete_collection(&self, id: CollectionId) -> Result<()> {
        self.database().write(move |transaction| {
            must_exist(transaction, "collection", id, T::NoCollection)?;
            transaction
                .prepare_cached("DELETE FROM collection_item WHERE collection_id = ?1")?
                .execute([id])?;
            transaction
                .prepare_cached("DELETE FROM collection WHERE id = ?1")?
                .execute([id])?;
            Ok(())
        })
    }

    /// Puts works in a collection at `place` among its works, in the order given; a work already
    /// in it moves there.
    pub fn add_to_collection(
        &self,
        collection: CollectionId,
        works: &[WorkId],
        place: Place,
    ) -> Result<()> {
        let works = works.to_vec();
        self.database().write(move |transaction| {
            must_exist(transaction, "collection", collection, T::NoCollection)?;
            works_exist(transaction, &works)?;
            put_in_collection(transaction, collection, &works, place)
        })
    }

    /// Takes works out of a collection.
    pub fn remove_from_collection(&self, collection: CollectionId, works: &[WorkId]) -> Result<()> {
        let works = works.to_vec();
        self.database().write(move |transaction| {
            must_exist(transaction, "collection", collection, T::NoCollection)?;
            for work in works {
                transaction
                    .prepare_cached(
                        "DELETE FROM collection_item WHERE collection_id = ?1 AND work_id = ?2",
                    )?
                    .execute(params![collection, work])?;
            }
            Ok(())
        })
    }

    /// The works of a collection, in its order.
    pub fn collection_works(&self, collection: CollectionId) -> Result<Vec<WorkId>> {
        self.database().read(move |connection| {
            must_exist(connection, "collection", collection, T::NoCollection)?;
            Ok(collection_items(connection, collection)?
                .into_iter()
                .map(|(work, _)| work)
                .collect())
        })
    }

    /// Every smart collection, in the person's order, with how many works each matches now.
    pub fn smart_collections(&self) -> Result<Vec<SmartCollection>> {
        self.database().read(smart_collections)
    }

    /// Makes a smart collection. Its rule is checked first.
    pub fn create_smart_collection(
        &self,
        name: &str,
        rules: &Rules,
        place: Place,
    ) -> Result<SmartCollectionId> {
        let name = clean_name(name, MAX_NAME, T::ThingSmartCollection)?;
        let rules = rules.clone();
        self.database().write(move |transaction| {
            let json = smart::check(&rules, &custom_kinds(transaction)?)?;
            let siblings = ordered(
                transaction,
                "SELECT id, sort_key FROM smart_collection WHERE ?1 IS NULL ORDER BY sort_key, id",
                None,
            )?;
            let key = key_for(&siblings, place)?;
            let now = now_ms();
            transaction
                .prepare_cached(
                    "INSERT INTO smart_collection (name, rules, sort_key, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?4)",
                )?
                .execute(params![name, json, key, now])?;
            Ok(transaction.last_insert_rowid())
        })
    }

    /// Renames a smart collection, or gives it another rule.
    pub fn update_smart_collection(
        &self,
        id: SmartCollectionId,
        name: Option<&str>,
        rules: Option<&Rules>,
    ) -> Result<()> {
        let name = name
            .map(|name| clean_name(name, MAX_NAME, T::ThingSmartCollection))
            .transpose()?;
        let rules = rules.cloned();
        self.database().write(move |transaction| {
            must_exist(transaction, "smart_collection", id, T::NoSmartCollection)?;
            let json = rules
                .map(|rules| smart::check(&rules, &custom_kinds(transaction)?))
                .transpose()?;
            transaction
                .prepare_cached(
                    "UPDATE smart_collection SET name = coalesce(?2, name),
                         rules = coalesce(?3, rules), updated_at = ?4
                     WHERE id = ?1",
                )?
                .execute(params![id, name, json, now_ms()])?;
            Ok(())
        })
    }

    /// Removes a smart collection; the works it matched are untouched.
    pub fn delete_smart_collection(&self, id: SmartCollectionId) -> Result<()> {
        self.database().write(move |transaction| {
            must_exist(transaction, "smart_collection", id, T::NoSmartCollection)?;
            transaction
                .prepare_cached("DELETE FROM smart_collection WHERE id = ?1")?
                .execute([id])?;
            Ok(())
        })
    }

    /// The works a rule matches now, by title, without keeping it: for a person writing one.
    pub fn smart_preview(&self, rules: &Rules) -> Result<Vec<WorkId>> {
        let rules = rules.clone();
        self.database().read(move |connection| {
            let (condition, values) =
                smart::compile(&rules, &custom_kinds(connection)?, now_ms())?;
            let works = connection
                .prepare(&format!(
                    "SELECT w.id FROM work w WHERE {condition} ORDER BY w.title COLLATE NOCASE, w.id"
                ))?
                .query_map(rusqlite::params_from_iter(values), |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            Ok(works)
        })
    }

    /// Every custom field, in the person's order.
    pub fn custom_fields(&self) -> Result<Vec<CustomField>> {
        self.database().read(custom_fields)
    }

    /// Defines a custom field of a kind (`text`, `number`, `date`, `boolean`, `choice`, `url`);
    /// a `choice` field with its choices.
    pub fn create_custom_field(
        &self,
        name: &str,
        kind: &str,
        choices: &[String],
    ) -> Result<CustomFieldId> {
        if !FIELD_KINDS.contains(&kind) {
            return Err(Error::Refused(tr!(FieldKindUnknown, kind = kind)));
        }
        let name = clean_name(name, MAX_TAG_NAME, T::ThingField)?;
        let choices = clean_choices(kind, choices)?;
        let kind = kind.to_owned();
        self.database()
            .write(move |transaction| insert_custom_field(transaction, &name, &kind, &choices))
    }

    /// Renames a custom field, or gives a `choice` field other choices. A choice some work has
    /// cannot be taken away.
    pub fn update_custom_field(
        &self,
        id: CustomFieldId,
        name: Option<&str>,
        choices: Option<&[String]>,
    ) -> Result<()> {
        let name = name
            .map(|name| clean_name(name, MAX_TAG_NAME, T::ThingField))
            .transpose()?;
        let choices = choices.map(<[String]>::to_vec);
        self.database().write(move |transaction| {
            let kind: String = transaction
                .prepare_cached("SELECT kind FROM custom_field WHERE id = ?1")?
                .query_row([id], |row| row.get(0))
                .optional()?
                .ok_or_else(|| Error::NotFound(tr!(NoField, id = id)))?;
            if let Some(name) = &name {
                transaction
                    .prepare_cached(
                        "UPDATE custom_field SET name = ?2, name_key = ?3 WHERE id = ?1",
                    )?
                    .execute(params![id, name, fold(name)])
                    .map_err(|error| constraint_to(error, || tr!(FieldNameTaken, name = name)))?;
            }
            if let Some(choices) = choices {
                let choices = clean_choices(&kind, &choices)?;
                let used: Vec<String> = transaction
                    .prepare_cached(
                        "SELECT DISTINCT value FROM work_custom_value WHERE field_id = ?1",
                    )?
                    .query_map([id], |row| row.get(0))?
                    .collect::<rusqlite::Result<_>>()?;
                if let Some(missing) = used.iter().find(|value| !choices.contains(value)) {
                    return Err(Error::Refused(tr!(ChoiceInUse, choice = missing)));
                }
                transaction
                    .prepare_cached("UPDATE custom_field SET choices = ?2 WHERE id = ?1")?
                    .execute(params![id, serde_json::to_string(&choices)?])?;
            }
            Ok(())
        })
    }

    /// Removes a custom field and every work's value of it. A smart collection that names it no
    /// longer matches anything, and says so.
    pub fn delete_custom_field(&self, id: CustomFieldId) -> Result<()> {
        self.database().write(move |transaction| {
            must_exist(transaction, "custom_field", id, T::NoField)?;
            transaction
                .prepare_cached("DELETE FROM work_custom_value WHERE field_id = ?1")?
                .execute([id])?;
            transaction
                .prepare_cached("DELETE FROM custom_field WHERE id = ?1")?
                .execute([id])?;
            Ok(())
        })
    }
}

fn clean_description(description: &str) -> Result<String> {
    let description = description.trim();
    if description.chars().count() > MAX_DESCRIPTION {
        return Err(Error::Refused(tr!(
            DescriptionTooLong,
            max = MAX_DESCRIPTION
        )));
    }
    Ok(description.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_goes_where_it_is_put() {
        let list = vec![
            (1, "F".to_owned()),
            (2, "V".to_owned()),
            (3, "k".to_owned()),
        ];
        let first = key_for(&list, Place::First).unwrap();
        assert!(first.as_str() < "F");
        let last = key_for(&list, Place::Last).unwrap();
        assert!(last.as_str() > "k");
        let before = key_for(&list, Place::Before(2)).unwrap();
        assert!("F" < before.as_str() && before.as_str() < "V");
        let after = key_for(&list, Place::After(2)).unwrap();
        assert!("V" < after.as_str() && after.as_str() < "k");
        assert!(key_for(&list, Place::After(9)).is_err());
        assert_eq!(key_for(&[], Place::Last).unwrap(), order::first());
    }

    #[test]
    fn names_are_trimmed_and_checked() {
        assert_eq!(clean_name("  Física ", 10, T::ThingTag).unwrap(), "Física");
        assert!(clean_name("   ", 10, T::ThingTag).is_err());
        assert!(clean_name("abcdefghijk", 10, T::ThingTag).is_err());
        assert!(clean_name("a\u{7}b", 10, T::ThingTag).is_err());
    }

    #[test]
    fn depth_counts_the_ancestors() {
        assert_eq!(depth_of("/3/"), 0);
        assert_eq!(depth_of("/3/14/15/"), 2);
    }
}

//! What the library knows of a work, as a person reads and edits it: its title, who made it and in
//! what role, its series, year, language and identifiers, the person's rating and reading status,
//! and where the person put it (categories, tags, collections, custom fields).
//!
//! Every field a person sets here is theirs from then on: its provenance becomes `user`, and
//! nothing read from a file again (a reimport, a new revision of the file, the same book found
//! elsewhere) writes over it (`content::apply_extraction`). Where the person put a work has no
//! provenance: no file says anything about it.

use std::collections::BTreeMap;

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Deserializer, Serialize};

use crate::content::set_provenance;
use crate::error::{Error, Result};
use crate::library::Library;
use crate::model::{DocumentId, WorkId};
use crate::search;
use crate::sql::{date_ms, now_ms};
use crate::taxonomy::{
    self, CategoryId, CollectionId, CustomFieldId, Place, TagId, put_in_collection,
};

/// The roles a person may have in making a work.
pub const ROLES: [&str; 6] = [
    "author",
    "editor",
    "translator",
    "illustrator",
    "introduction",
    "contributor",
];
/// The schemes of identifiers.
pub const SCHEMES: [&str; 8] = [
    "isbn", "issn", "doi", "oclc", "lccn", "asin", "uri", "other",
];
/// The most creators and identifiers of a work.
pub const MAX_CREATORS: usize = 100;
pub const MAX_IDENTIFIERS: usize = 32;
/// The longest text of a custom field.
pub const MAX_CUSTOM_TEXT: usize = 4000;
/// The longest description of a work.
pub const MAX_DESCRIPTION: usize = 100_000;

/// Someone who made a work, and in what role.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credit {
    pub name: String,
    #[serde(default = "author")]
    pub role: String,
}

fn author() -> String {
    "author".to_owned()
}

impl Credit {
    /// An author.
    pub fn author(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            role: author(),
        }
    }
}

/// An identifier of a work, by its scheme.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identifier {
    pub scheme: String,
    pub value: String,
}

/// A work's value of a custom field; `null` in an edit takes it away.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomValue {
    pub field_id: CustomFieldId,
    pub value: serde_json::Value,
}

/// A work, as the editor shows it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkMetadata {
    pub id: WorkId,
    pub title: String,
    pub creators: Vec<Credit>,
    pub series: Option<String>,
    pub series_position: Option<f64>,
    pub year: Option<i64>,
    pub language: Option<String>,
    pub identifiers: Vec<Identifier>,
    pub rating: Option<i64>,
    pub status: String,
    /// What the book is about, as plain text.
    pub description: Option<String>,
    pub categories: Vec<CategoryId>,
    pub tags: Vec<TagId>,
    /// The tags' names, in the same order.
    pub tag_names: Vec<String>,
    pub collections: Vec<CollectionId>,
    pub custom: Vec<CustomValue>,
    /// Where each field it has a provenance for came from: `filename`, `embedded` (read from the
    /// file), `calibre` (brought from a Calibre library) or `user`.
    pub provenance: BTreeMap<String, String>,
    pub documents: Vec<DocumentId>,
    /// The formats of its documents, as their files are named.
    pub formats: Vec<String>,
}

/// For a field that may be set to nothing: absent leaves it, `null` clears it.
pub(crate) fn present<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(deserializer).map(Some)
}

/// What a person changes about a work. `None` leaves a field as it is; `Some(None)` clears it.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkEdit {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub creators: Option<Vec<Credit>>,
    #[serde(default, deserialize_with = "present")]
    pub series: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    pub series_position: Option<Option<f64>>,
    #[serde(default, deserialize_with = "present")]
    pub year: Option<Option<i64>>,
    #[serde(default, deserialize_with = "present")]
    pub language: Option<Option<String>>,
    #[serde(default)]
    pub identifiers: Option<Vec<Identifier>>,
    /// From 1 to 5.
    #[serde(default, deserialize_with = "present")]
    pub rating: Option<Option<i64>>,
    /// `unread`, `reading`, `read` or `abandoned`.
    #[serde(default)]
    pub status: Option<String>,
    /// What the book is about, as plain text.
    #[serde(default, deserialize_with = "present")]
    pub description: Option<Option<String>>,
    /// Exactly these categories.
    #[serde(default)]
    pub categories: Option<Vec<CategoryId>>,
    /// Exactly these tags, by name; one that does not exist yet is made.
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    /// Exactly these collections; one it joins puts it last.
    #[serde(default)]
    pub collections: Option<Vec<CollectionId>>,
    /// These custom fields' values; the others stay.
    #[serde(default)]
    pub custom: Option<Vec<CustomValue>>,
}

/// A description: its lines trimmed at the end, runs of blank lines made one, nothing for empty.
pub(crate) fn clean_description(value: Option<String>) -> Result<Option<String>> {
    let Some(text) = value else {
        return Ok(None);
    };
    let mut out = String::with_capacity(text.len());
    let mut blank = 0;
    for line in text.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            blank += 1;
            continue;
        }
        if !out.is_empty() {
            out.push_str(if blank > 0 { "\n\n" } else { "\n" });
        }
        blank = 0;
        out.push_str(line);
    }
    if out.is_empty() {
        return Ok(None);
    }
    if out.chars().count() > MAX_DESCRIPTION {
        return Err(Error::Refused(format!(
            "a description is {MAX_DESCRIPTION} characters at most"
        )));
    }
    Ok(Some(out))
}

/// A text field: trimmed, nothing for empty, `max` characters at most.
fn optional_text(value: Option<String>, max: usize, what: &str) -> Result<Option<String>> {
    match value.map(|text| text.trim().to_owned()) {
        Some(text) if text.is_empty() => Ok(None),
        Some(text) if text.chars().count() > max => Err(Error::Refused(format!(
            "{what} is {max} characters at most"
        ))),
        other => Ok(other),
    }
}

/// A language tag as BCP 47 writes it, `pt`, `pt-BR`, `zh-Hant-TW`: its language in lower case,
/// a region in upper case, a script with a capital.
pub fn language_tag(text: &str) -> Option<String> {
    let mut parts = text.trim().split(['-', '_']);
    let language = parts.next()?;
    if !(2..=3).contains(&language.len()) || !language.bytes().all(|b| b.is_ascii_alphabetic()) {
        return None;
    }
    let mut tag = language.to_ascii_lowercase();
    for part in parts {
        if !(2..=8).contains(&part.len()) || !part.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return None;
        }
        tag.push('-');
        if part.len() == 2 && part.bytes().all(|b| b.is_ascii_alphabetic()) {
            tag.push_str(&part.to_ascii_uppercase());
        } else if part.len() == 4 && part.bytes().all(|b| b.is_ascii_alphabetic()) {
            tag.push_str(&part[..1].to_ascii_uppercase());
            tag.push_str(&part[1..].to_ascii_lowercase());
        } else {
            tag.push_str(&part.to_ascii_lowercase());
        }
    }
    Some(tag)
}

/// An ISBN without its spaces and hyphens, if its check digit is right: 10 digits (the last may
/// be X) or 13.
pub fn isbn(text: &str) -> Option<String> {
    let digits: String = text
        .chars()
        .filter(|c| !matches!(c, '-' | ' ' | '\u{2010}' | '\u{2011}'))
        .map(|c| c.to_ascii_uppercase())
        .collect();
    let value = |c: char| c.to_digit(10);
    match digits.len() {
        10 => {
            let mut sum = 0;
            for (index, c) in digits.chars().enumerate() {
                let digit = match (index, c) {
                    (9, 'X') => 10,
                    _ => value(c)?,
                };
                sum += digit * (10 - index as u32);
            }
            (sum % 11 == 0).then_some(digits)
        }
        13 => {
            let mut sum = 0;
            for (index, c) in digits.chars().enumerate() {
                sum += value(c)? * if index % 2 == 0 { 1 } else { 3 };
            }
            (sum % 10 == 0).then_some(digits)
        }
        _ => None,
    }
}

fn clean_identifiers(identifiers: Vec<Identifier>) -> Result<Vec<Identifier>> {
    if identifiers.len() > MAX_IDENTIFIERS {
        return Err(Error::Refused(format!(
            "a work has {MAX_IDENTIFIERS} identifiers at most"
        )));
    }
    let mut out: Vec<Identifier> = Vec::with_capacity(identifiers.len());
    for identifier in identifiers {
        if !SCHEMES.contains(&identifier.scheme.as_str()) {
            return Err(Error::Refused(format!(
                "{} is not a scheme of identifiers",
                identifier.scheme
            )));
        }
        let value = identifier.value.trim();
        if value.is_empty() || value.chars().count() > 400 {
            return Err(Error::Refused(
                "an identifier is 1 to 400 characters".to_owned(),
            ));
        }
        let value = if identifier.scheme == "isbn" {
            isbn(value).ok_or_else(|| Error::Refused(format!("{value} is not a valid ISBN")))?
        } else {
            value.to_owned()
        };
        let cleaned = Identifier {
            scheme: identifier.scheme,
            value,
        };
        if !out.contains(&cleaned) {
            out.push(cleaned);
        }
    }
    Ok(out)
}

fn clean_credits(credits: Vec<Credit>) -> Result<Vec<Credit>> {
    if credits.len() > MAX_CREATORS {
        return Err(Error::Refused(format!(
            "a work has {MAX_CREATORS} creators at most"
        )));
    }
    credits
        .into_iter()
        .map(|credit| {
            if !ROLES.contains(&credit.role.as_str()) {
                return Err(Error::Refused(format!("{} is not a role", credit.role)));
            }
            let name = taxonomy::clean_name(&credit.name, 300, "a creator")?;
            Ok(Credit {
                name,
                role: credit.role,
            })
        })
        .collect()
}

/// The custom field's value, checked against its kind, as it is kept.
pub(crate) fn custom_sql(
    kind: &str,
    choices: &[String],
    value: &serde_json::Value,
) -> Result<rusqlite::types::Value> {
    use rusqlite::types::Value;
    let refused = |why: &str| Error::Refused(format!("a {kind} field's value {why}"));
    Ok(match kind {
        "number" => Value::Real(
            value
                .as_f64()
                .filter(|number| number.is_finite())
                .ok_or_else(|| refused("is a number"))?,
        ),
        "boolean" => Value::Integer(i64::from(
            value.as_bool().ok_or_else(|| refused("is true or false"))?,
        )),
        _ => {
            let text = value
                .as_str()
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .ok_or_else(|| refused("is a text"))?;
            match kind {
                "date" if date_ms(text).is_none() => return Err(refused("is a date, YYYY-MM-DD")),
                "choice" if !choices.iter().any(|choice| choice == text) => {
                    return Err(refused("is one of its choices"));
                }
                "url"
                    if !(text.starts_with("https://") || text.starts_with("http://"))
                        || text.contains(char::is_whitespace) =>
                {
                    return Err(refused("is an http or https address"));
                }
                _ if text.chars().count() > MAX_CUSTOM_TEXT => {
                    return Err(refused("is too long"));
                }
                _ => Value::Text(text.to_owned()),
            }
        }
    })
}

fn custom_from_sql(kind: &str, value: rusqlite::types::Value) -> serde_json::Value {
    use rusqlite::types::Value;
    match (kind, value) {
        ("boolean", Value::Integer(number)) => serde_json::Value::Bool(number != 0),
        (_, Value::Integer(number)) => serde_json::json!(number),
        (_, Value::Real(number)) => serde_json::json!(number),
        (_, Value::Text(text)) => serde_json::Value::String(text),
        _ => serde_json::Value::Null,
    }
}

/// Writes a work's creators, in order, each in their role.
pub(crate) fn set_credits(
    transaction: &Transaction<'_>,
    work: WorkId,
    credits: &[Credit],
) -> Result<()> {
    transaction
        .prepare_cached("DELETE FROM work_creator WHERE work_id = ?1")?
        .execute([work])?;
    for (position, credit) in credits.iter().enumerate() {
        transaction
            .prepare_cached("INSERT OR IGNORE INTO creator (name) VALUES (?1)")?
            .execute([&credit.name])?;
        let creator: i64 = transaction
            .prepare_cached("SELECT id FROM creator WHERE name = ?1")?
            .query_row([&credit.name], |row| row.get(0))?;
        transaction
            .prepare_cached(
                "INSERT INTO work_creator (work_id, creator_id, role, position)
                 VALUES (?1, ?2, ?3, ?4)",
            )?
            .execute(params![work, creator, credit.role, position as i64])?;
    }
    Ok(())
}

pub(crate) fn write_identifiers(
    transaction: &Transaction<'_>,
    work: WorkId,
    identifiers: &[Identifier],
) -> Result<()> {
    transaction
        .prepare_cached("DELETE FROM work_identifier WHERE work_id = ?1")?
        .execute([work])?;
    for (position, identifier) in identifiers.iter().enumerate() {
        transaction
            .prepare_cached(
                "INSERT INTO work_identifier (work_id, position, scheme, value)
                 VALUES (?1, ?2, ?3, ?4)",
            )?
            .execute(params![
                work,
                position as i64,
                identifier.scheme,
                identifier.value
            ])?;
    }
    Ok(())
}

/// An identifier a file gave (an ISBN, or a `dc:identifier`), by its scheme.
pub(crate) fn identifier_of_file(isbn_text: Option<&str>, dc: Option<&str>) -> Vec<Identifier> {
    let mut out = Vec::new();
    if let Some(text) = isbn_text.map(str::trim).filter(|text| !text.is_empty()) {
        out.push(Identifier {
            scheme: "isbn".to_owned(),
            value: isbn(text).unwrap_or_else(|| text.to_owned()),
        });
    }
    if let Some(text) = dc.map(str::trim).filter(|text| !text.is_empty()) {
        let lower = text.to_ascii_lowercase();
        let (scheme, value) = if let Some(rest) = lower.strip_prefix("urn:isbn:") {
            (
                "isbn",
                isbn(rest).unwrap_or_else(|| text[9..].trim().to_owned()),
            )
        } else if lower.starts_with("urn:doi:") {
            ("doi", text[8..].trim().to_owned())
        } else if lower.starts_with("doi:") {
            ("doi", text[4..].trim().to_owned())
        } else if text.starts_with("10.") && text.contains('/') {
            ("doi", text.to_owned())
        } else if text.contains(':') {
            ("uri", text.to_owned())
        } else {
            ("other", text.to_owned())
        };
        out.push(Identifier {
            scheme: scheme.to_owned(),
            value,
        });
    }
    out
}

/// The identifiers a work's files give, when the person has not set them: those of its documents,
/// each once, in the order the documents came.
pub(crate) fn refresh_identifiers(transaction: &Transaction<'_>, work: WorkId) -> Result<()> {
    let origin: Option<String> = transaction
        .prepare_cached(
            "SELECT origin FROM field_provenance WHERE work_id = ?1 AND field = 'identifiers'",
        )?
        .query_row([work], |row| row.get(0))
        .optional()?;
    // The person's list, or Calibre's, stays as it is.
    if matches!(origin.as_deref(), Some("user" | "calibre")) {
        return Ok(());
    }
    let documents: Vec<(Option<String>, Option<String>)> = transaction
        .prepare_cached("SELECT isbn, dc_identifier FROM document WHERE work_id = ?1 ORDER BY id")?
        .query_map([work], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut identifiers: Vec<Identifier> = Vec::new();
    for (isbn_text, dc) in documents {
        for identifier in identifier_of_file(isbn_text.as_deref(), dc.as_deref()) {
            if identifier.value.chars().count() <= 400 && !identifiers.contains(&identifier) {
                identifiers.push(identifier);
            }
        }
    }
    identifiers.truncate(MAX_IDENTIFIERS);
    write_identifiers(transaction, work, &identifiers)
}

/// When a work goes because it turned out to be another, where the person put it goes with it:
/// its categories, tags, collections (after the other's books) and custom values, where the other
/// has none of them.
pub(crate) fn move_placement(
    transaction: &Transaction<'_>,
    from: WorkId,
    into: WorkId,
) -> Result<()> {
    for (copy, clear) in [
        (
            "INSERT OR IGNORE INTO work_category (work_id, category_id, added_at)
             SELECT ?2, category_id, added_at FROM work_category WHERE work_id = ?1",
            "DELETE FROM work_category WHERE work_id = ?1",
        ),
        (
            "INSERT OR IGNORE INTO work_tag (work_id, tag_id, added_at)
             SELECT ?2, tag_id, added_at FROM work_tag WHERE work_id = ?1",
            "DELETE FROM work_tag WHERE work_id = ?1",
        ),
        (
            "INSERT OR IGNORE INTO work_custom_value (work_id, field_id, value, updated_at)
             SELECT ?2, field_id, value, updated_at FROM work_custom_value WHERE work_id = ?1",
            "DELETE FROM work_custom_value WHERE work_id = ?1",
        ),
    ] {
        transaction
            .prepare_cached(copy)?
            .execute(params![from, into])?;
        transaction.prepare_cached(clear)?.execute([from])?;
    }
    let collections: Vec<CollectionId> = transaction
        .prepare_cached(
            "SELECT collection_id FROM collection_item WHERE work_id = ?1
             AND collection_id NOT IN (SELECT collection_id FROM collection_item WHERE work_id = ?2)",
        )?
        .query_map(params![from, into], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    transaction
        .prepare_cached("DELETE FROM collection_item WHERE work_id = ?1")?
        .execute([from])?;
    for collection in collections {
        put_in_collection(transaction, collection, &[into], Place::Last)?;
    }
    transaction
        .prepare_cached("DELETE FROM work_identifier WHERE work_id = ?1")?
        .execute([from])?;
    Ok(())
}

pub(crate) fn work_metadata(connection: &Connection, work: WorkId) -> Result<WorkMetadata> {
    let row = connection
        .prepare_cached(
            "SELECT title, series, series_position, year, language, rating, status, description
             FROM work WHERE id = ?1",
        )?
        .query_row([work], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<f64>>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, Option<String>>(7)?,
            ))
        })
        .optional()?
        .ok_or_else(|| Error::NotFound(format!("there is no work {work}")))?;
    let (title, series, series_position, year, language, rating, status, description) = row;
    let list = |sql: &str| -> Result<Vec<i64>> {
        Ok(connection
            .prepare_cached(sql)?
            .query_map([work], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    };
    let creators = connection
        .prepare_cached(
            "SELECT c.name, x.role FROM work_creator x JOIN creator c ON c.id = x.creator_id
             WHERE x.work_id = ?1 ORDER BY x.position",
        )?
        .query_map([work], |row| {
            Ok(Credit {
                name: row.get(0)?,
                role: row.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let identifiers = connection
        .prepare_cached(
            "SELECT scheme, value FROM work_identifier WHERE work_id = ?1 ORDER BY position",
        )?
        .query_map([work], |row| {
            Ok(Identifier {
                scheme: row.get(0)?,
                value: row.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let custom = connection
        .prepare_cached(
            "SELECT v.field_id, f.kind, v.value FROM work_custom_value v
             JOIN custom_field f ON f.id = v.field_id
             WHERE v.work_id = ?1 ORDER BY f.sort_key, f.id",
        )?
        .query_map([work], |row| {
            let kind: String = row.get(1)?;
            Ok(CustomValue {
                field_id: row.get(0)?,
                value: custom_from_sql(&kind, row.get(2)?),
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let provenance = connection
        .prepare_cached("SELECT field, origin FROM field_provenance WHERE work_id = ?1")?
        .query_map([work], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let tags: Vec<(TagId, String)> = connection
        .prepare_cached(
            "SELECT x.tag_id, t.name FROM work_tag x JOIN tag t ON t.id = x.tag_id
             WHERE x.work_id = ?1 ORDER BY t.name_key",
        )?
        .query_map([work], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let formats: Vec<String> = connection
        .prepare_cached("SELECT DISTINCT format FROM document WHERE work_id = ?1 ORDER BY format")?
        .query_map([work], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(WorkMetadata {
        id: work,
        title,
        creators,
        series,
        series_position,
        year,
        language,
        identifiers,
        rating,
        status,
        description,
        categories: list(
            "SELECT x.category_id FROM work_category x JOIN category c ON c.id = x.category_id
             WHERE x.work_id = ?1 ORDER BY c.path",
        )?,
        tag_names: tags.iter().map(|(_, name)| name.clone()).collect(),
        tags: tags.into_iter().map(|(id, _)| id).collect(),
        collections: list(
            "SELECT x.collection_id FROM collection_item x JOIN collection c
                 ON c.id = x.collection_id
             WHERE x.work_id = ?1 ORDER BY c.sort_key, c.id",
        )?,
        custom,
        provenance,
        documents: list("SELECT id FROM document WHERE work_id = ?1 ORDER BY id")?,
        formats,
    })
}

/// Checks an edit before it reaches the writer: what can be judged without the database.
pub(crate) fn check_edit(edit: WorkEdit) -> Result<WorkEdit> {
    let mut edit = edit;
    if let Some(title) = &edit.title {
        edit.title = Some(taxonomy::clean_name(title, 1000, "a work")?);
    }
    if let Some(credits) = edit.creators.take() {
        edit.creators = Some(clean_credits(credits)?);
    }
    if let Some(series) = edit.series.take() {
        edit.series = Some(optional_text(series, 300, "a series")?);
    }
    if let Some(Some(position)) = edit.series_position
        && !(position.is_finite() && (0.0..=1_000_000.0).contains(&position))
    {
        return Err(Error::Refused(
            "a position in a series goes from 0 to 1000000".to_owned(),
        ));
    }
    if let Some(Some(year)) = edit.year
        && !(-9999..=9999).contains(&year)
    {
        return Err(Error::Refused("a year goes from -9999 to 9999".to_owned()));
    }
    if let Some(language) = edit.language.take() {
        edit.language = Some(match optional_text(language, 35, "a language")? {
            None => None,
            Some(text) => Some(
                language_tag(&text)
                    .ok_or_else(|| Error::Refused(format!("{text} is not a language tag")))?,
            ),
        });
    }
    if let Some(identifiers) = edit.identifiers.take() {
        edit.identifiers = Some(clean_identifiers(identifiers)?);
    }
    if let Some(Some(rating)) = edit.rating
        && !(1..=5).contains(&rating)
    {
        return Err(Error::Refused("a rating goes from 1 to 5".to_owned()));
    }
    if let Some(status) = &edit.status
        && !["unread", "reading", "read", "abandoned"].contains(&status.as_str())
    {
        return Err(Error::Refused(format!("{status} is not a status")));
    }
    if let Some(description) = edit.description.take() {
        edit.description = Some(clean_description(description)?);
    }
    Ok(edit)
}

pub(crate) fn apply_edit(
    transaction: &Transaction<'_>,
    work: WorkId,
    edit: WorkEdit,
) -> Result<()> {
    let now = now_ms();
    let exists: Option<i64> = transaction
        .prepare_cached("SELECT 1 FROM work WHERE id = ?1")?
        .query_row([work], |row| row.get(0))
        .optional()?;
    if exists.is_none() {
        return Err(Error::NotFound(format!("there is no work {work}")));
    }
    let mut changed: Vec<&str> = Vec::new();
    // The columns of the work itself. Each name comes from this list, not from the edit.
    let mut column = |name: &'static str, value: rusqlite::types::Value| -> Result<()> {
        transaction
            .prepare_cached(&format!("UPDATE work SET {name} = ?2 WHERE id = ?1"))?
            .execute(params![work, value])?;
        changed.push(name);
        Ok(())
    };
    use rusqlite::types::Value;
    let text = |value: Option<String>| value.map_or(Value::Null, Value::Text);
    if let Some(title) = edit.title {
        column("title", Value::Text(title))?;
    }
    if let Some(series) = edit.series {
        column("series", text(series))?;
    }
    if let Some(position) = edit.series_position {
        column("series_position", position.map_or(Value::Null, Value::Real))?;
    }
    if let Some(year) = edit.year {
        column("year", year.map_or(Value::Null, Value::Integer))?;
    }
    if let Some(language) = edit.language {
        column("language", text(language))?;
    }
    if let Some(rating) = edit.rating {
        column("rating", rating.map_or(Value::Null, Value::Integer))?;
    }
    if let Some(status) = edit.status {
        column("status", Value::Text(status))?;
    }
    if let Some(description) = edit.description {
        column("description", text(description))?;
    }
    if let Some(credits) = &edit.creators {
        set_credits(transaction, work, credits)?;
        changed.push("creators");
    }
    if let Some(identifiers) = &edit.identifiers {
        write_identifiers(transaction, work, identifiers)?;
        changed.push("identifiers");
    }
    for field in &changed {
        set_provenance(transaction, work, field, "user", now)?;
    }
    if let Some(categories) = edit.categories {
        for &category in &categories {
            if transaction
                .prepare_cached("SELECT 1 FROM category WHERE id = ?1")?
                .query_row([category], |_| Ok(()))
                .optional()?
                .is_none()
            {
                return Err(Error::NotFound(format!("there is no category {category}")));
            }
        }
        transaction
            .prepare_cached("DELETE FROM work_category WHERE work_id = ?1")?
            .execute([work])?;
        for category in categories {
            transaction
                .prepare_cached(
                    "INSERT OR IGNORE INTO work_category (work_id, category_id, added_at)
                     VALUES (?1, ?2, ?3)",
                )?
                .execute(params![work, category, now])?;
        }
    }
    if let Some(names) = edit.tags {
        let mut tags = Vec::with_capacity(names.len());
        for name in &names {
            tags.push(taxonomy::tag_named(transaction, name)?);
        }
        transaction
            .prepare_cached("DELETE FROM work_tag WHERE work_id = ?1")?
            .execute([work])?;
        for tag in tags {
            transaction
                .prepare_cached(
                    "INSERT OR IGNORE INTO work_tag (work_id, tag_id, added_at) VALUES (?1, ?2, ?3)",
                )?
                .execute(params![work, tag, now])?;
        }
    }
    if let Some(collections) = edit.collections {
        let current: Vec<CollectionId> = transaction
            .prepare_cached("SELECT collection_id FROM collection_item WHERE work_id = ?1")?
            .query_map([work], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for &collection in &collections {
            if transaction
                .prepare_cached("SELECT 1 FROM collection WHERE id = ?1")?
                .query_row([collection], |_| Ok(()))
                .optional()?
                .is_none()
            {
                return Err(Error::NotFound(format!(
                    "there is no collection {collection}"
                )));
            }
            if !current.contains(&collection) {
                put_in_collection(transaction, collection, &[work], Place::Last)?;
            }
        }
        for collection in current.into_iter().filter(|c| !collections.contains(c)) {
            transaction
                .prepare_cached(
                    "DELETE FROM collection_item WHERE collection_id = ?1 AND work_id = ?2",
                )?
                .execute(params![collection, work])?;
        }
    }
    if let Some(values) = edit.custom {
        for CustomValue { field_id, value } in values {
            let field: Option<(String, String)> = transaction
                .prepare_cached("SELECT kind, choices FROM custom_field WHERE id = ?1")?
                .query_row([field_id], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()?;
            let Some((kind, choices)) = field else {
                return Err(Error::NotFound(format!("there is no field {field_id}")));
            };
            if value.is_null() {
                transaction
                    .prepare_cached(
                        "DELETE FROM work_custom_value WHERE work_id = ?1 AND field_id = ?2",
                    )?
                    .execute(params![work, field_id])?;
                continue;
            }
            let choices: Vec<String> = serde_json::from_str(&choices)?;
            let value = custom_sql(&kind, &choices, &value)?;
            transaction
                .prepare_cached(
                    "INSERT OR REPLACE INTO work_custom_value (work_id, field_id, value, updated_at)
                     VALUES (?1, ?2, ?3, ?4)",
                )?
                .execute(params![work, field_id, value, now])?;
        }
    }
    transaction
        .prepare_cached("UPDATE work SET updated_at = ?2 WHERE id = ?1")?
        .execute(params![work, now])?;
    if edit.creators.is_some() {
        search::refresh_entry(transaction, work)?;
    }
    Ok(())
}

impl Library {
    /// A work, as the editor shows it.
    pub fn work_metadata(&self, work: WorkId) -> Result<WorkMetadata> {
        self.database()
            .read(move |connection| work_metadata(connection, work))
    }

    /// Changes a work. Every field of the work set here is the person's from now on: nothing read
    /// from a file will ever write over it. Nothing is changed unless all of it can be.
    pub fn edit_work(&self, work: WorkId, edit: WorkEdit) -> Result<()> {
        let edit = check_edit(edit)?;
        self.database()
            .write(move |transaction| apply_edit(transaction, work, edit))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_are_written_as_bcp_47_writes_them() {
        assert_eq!(language_tag("PT-br").as_deref(), Some("pt-BR"));
        assert_eq!(language_tag("zh_hant_tw").as_deref(), Some("zh-Hant-TW"));
        assert_eq!(language_tag("eng").as_deref(), Some("eng"));
        assert_eq!(language_tag("português"), None);
        assert_eq!(language_tag("p"), None);
    }

    #[test]
    fn an_isbn_is_checked_by_its_last_digit() {
        assert_eq!(isbn("978-85-359-0277-8").as_deref(), Some("9788535902778"));
        assert_eq!(isbn("0-306-40615-2").as_deref(), Some("0306406152"));
        assert_eq!(isbn("0 8044 2957 x").as_deref(), Some("080442957X"));
        assert_eq!(isbn("978-85-359-0277-9"), None);
        assert_eq!(isbn("12345"), None);
    }

    #[test]
    fn a_file_s_identifiers_go_by_their_schemes() {
        let found = identifier_of_file(Some("978-85-359-0277-8"), Some("urn:isbn:9788535902778"));
        assert_eq!(found[0].value, "9788535902778");
        assert_eq!(found[1].scheme, "isbn");
        assert_eq!(found[1].value, "9788535902778");
        assert_eq!(
            identifier_of_file(None, Some("doi:10.1000/xyz"))[0].scheme,
            "doi"
        );
        assert_eq!(
            identifier_of_file(None, Some("urn:uuid:1234"))[0].scheme,
            "uri"
        );
        assert_eq!(identifier_of_file(None, Some("abc-123"))[0].scheme, "other");
    }

    #[test]
    fn an_edit_is_checked_before_it_reaches_the_database() {
        let refused = |edit: WorkEdit| check_edit(edit).is_err();
        assert!(refused(WorkEdit {
            rating: Some(Some(6)),
            ..WorkEdit::default()
        }));
        assert!(refused(WorkEdit {
            title: Some("   ".into()),
            ..WorkEdit::default()
        }));
        assert!(refused(WorkEdit {
            creators: Some(vec![Credit {
                name: "Ana".into(),
                role: "boss".into()
            }]),
            ..WorkEdit::default()
        }));
        assert!(refused(WorkEdit {
            identifiers: Some(vec![Identifier {
                scheme: "isbn".into(),
                value: "978-85-359-0277-9".into()
            }]),
            ..WorkEdit::default()
        }));
        let cleaned = check_edit(WorkEdit {
            series: Some(Some("  ".into())),
            language: Some(Some("PT-br".into())),
            ..WorkEdit::default()
        })
        .unwrap();
        assert_eq!(cleaned.series, Some(None), "an empty series is no series");
        assert_eq!(cleaned.language, Some(Some("pt-BR".into())));
    }

    #[test]
    fn absent_and_null_mean_different_things() {
        let edit: WorkEdit = serde_json::from_str(r#"{"series": null, "title": "T"}"#).unwrap();
        assert_eq!(edit.series, Some(None));
        assert_eq!(edit.year, None);
        assert!(serde_json::from_str::<WorkEdit>(r#"{"sql": "x"}"#).is_err());
    }
}

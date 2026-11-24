//! Bringing a Calibre library in, reading it and never writing to it.
//!
//! A Calibre library is a folder: `metadata.db` (SQLite) beside a folder per author with a folder
//! per book, which holds the book's files and a `metadata.opf` Calibre keeps as a backup. The
//! database is opened as an immutable file, read only: SQLite then takes no lock and writes no
//! journal, so nothing in the folder changes. While Calibre is writing to it (its journal or its
//! log is beside it) it is not read. Without a database, each book's `metadata.opf` is read
//! instead.
//!
//! Both are input like any book: the database is read with `trusted_schema` off, from its tables
//! only, with every value's length capped; the OPF goes through the same XML parser, with the same
//! limits, as a normalized book does.
//!
//! The books' files are the library's like any other: the folder becomes a root (or is already in
//! one) and the library finds, hashes and reads them. A book is brought in once its files are known
//! by their bytes, so one whose bytes the library already had elsewhere is that same book, not a
//! second one. Its formats become one work. What Calibre says goes on the work with provenance
//! `calibre`: above what a file says, below what the person sets here, which it never writes over.
//! Calibre's tags are added to the work's; its custom columns become custom fields, and a value the
//! work already has for one is left alone.

use musaeum_i18n::{T, tr};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use musaeum_flow::dom::{Data, Dom, Ns};
use musaeum_flow::xml::{self, Limits};
use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, params};
use serde::Serialize;

use crate::content::set_provenance;
use crate::error::{Error, Result};
use crate::library::{Library, absolute_if_it_exists, locate_instance};
use crate::merge::merge_into;
use crate::metadata::{
    Credit, Identifier, MAX_CUSTOM_TEXT, MAX_DESCRIPTION, MAX_IDENTIFIERS, custom_sql, isbn,
    language_tag, set_credits, write_identifiers,
};
use crate::model::WorkId;
use crate::scan::format_of;
use crate::search;
use crate::sql::{fold, now_ms};
use crate::taxonomy::{self, CustomFieldId, MAX_TAG_NAME, clean_choices, insert_custom_field};

/// The most books one import reads.
pub const MAX_BOOKS: usize = 200_000;
/// The longest value read from the database, in bytes.
const MAX_VALUE: i32 = 1 << 20;

/// A custom column of the Calibre library, and the kind of field it becomes.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalibreColumn {
    /// Calibre's own name for it, the `#label` without the `#`.
    pub label: String,
    /// The name a person sees.
    pub name: String,
    /// Calibre's datatype: `text`, `comments`, `series`, `enumeration`, `float`, `int`, `rating`,
    /// `datetime`, `bool` or `composite`.
    pub datatype: String,
    /// The kind of custom field it becomes, if any: a `composite` is worked out from the others
    /// and is not brought in.
    pub kind: Option<String>,
    pub choices: Vec<String>,
    pub is_multiple: bool,
}

/// A file Calibre lists for a book.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalibreFile {
    pub path: PathBuf,
    /// Whether it is on the disk.
    pub exists: bool,
}

/// One book of the Calibre library, as it will be brought in.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalibreBook {
    pub id: i64,
    pub uuid: Option<String>,
    pub title: String,
    pub authors: Vec<String>,
    pub series: Option<String>,
    pub series_index: Option<f64>,
    pub tags: Vec<String>,
    /// From 1 to 5, as Calibre's half stars round up.
    pub rating: Option<i64>,
    pub identifiers: Vec<Identifier>,
    /// Calibre's comments, as plain text.
    pub description: Option<String>,
    pub language: Option<String>,
    pub year: Option<i64>,
    pub files: Vec<CalibreFile>,
    /// Values of custom columns, by label, as the custom field takes them.
    pub custom: BTreeMap<String, serde_json::Value>,
}

/// A Calibre library as read.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalibreLibrary {
    /// Its folder.
    pub source: PathBuf,
    /// `metadata.db`, or `metadata.opf` when there was no database.
    pub read_from: String,
    pub books: Vec<CalibreBook>,
    pub columns: Vec<CalibreColumn>,
}

impl CalibreLibrary {
    /// The files of its books that are on the disk.
    pub fn present_files(&self) -> usize {
        self.books
            .iter()
            .flat_map(|book| &book.files)
            .filter(|file| file.exists)
            .count()
    }
}

/// What an import did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalibreReport {
    pub books: usize,
    /// Brought in now, with every file it has on the disk.
    pub imported: usize,
    /// Whose files the library has not read yet; another import brings them in.
    pub waiting: usize,
    /// With no file on the disk at all.
    pub missing: usize,
    /// Works joined because they were formats of one Calibre book.
    pub merged: usize,
    /// Custom fields made for Calibre's columns.
    pub fields_created: usize,
}

// --- Reading ---

/// Reads a Calibre library's folder, writing nothing to it.
pub fn read_calibre(folder: &Path) -> Result<CalibreLibrary> {
    let source = absolute_if_it_exists(folder);
    if !source.is_dir() {
        return Err(Error::NotFound(tr!(NoFolderAt, path = source.display())));
    }
    let database = source.join("metadata.db");
    if database.is_file() {
        for busy in ["metadata.db-journal", "metadata.db-wal"] {
            if source.join(busy).exists() {
                return Err(Error::Refused(tr!(CalibreWriting)));
            }
        }
        let connection = open_immutable(&database)?;
        let (books, columns) = read_database(&connection, &source)?;
        return Ok(CalibreLibrary {
            source,
            read_from: "metadata.db".to_owned(),
            books,
            columns,
        });
    }
    let (books, columns) = read_opfs(&source)?;
    if books.is_empty() {
        return Err(Error::NotFound(tr!(
            CalibreNothing,
            path = source.display()
        )));
    }
    Ok(CalibreLibrary {
        source,
        read_from: "metadata.opf".to_owned(),
        books,
        columns,
    })
}

/// A `file:` URI for SQLite, every byte but the plain ones escaped.
fn file_uri(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let mut uri = String::from("file:");
    if !text.starts_with('/') {
        uri.push_str("///");
    }
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                uri.push(byte as char);
            }
            _ => uri.push_str(&format!("%{byte:02X}")),
        }
    }
    uri.push_str("?mode=ro&immutable=1");
    uri
}

fn open_immutable(database: &Path) -> Result<Connection> {
    let connection = Connection::open_with_flags(
        file_uri(database),
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    // Nothing the file declares runs: no function in a view or a trigger, and values are capped.
    connection.pragma_update(None, "trusted_schema", false)?;
    connection.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH, MAX_VALUE)?;
    connection.set_db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
    Ok(connection)
}

fn has_table(connection: &Connection, name: &str) -> Result<bool> {
    Ok(connection
        .prepare_cached("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1")?
        .query_row([name], |_| Ok(()))
        .optional()?
        .is_some())
}

/// Calibre's undefined date is the year 101; anything before it is no date.
fn year_of(date: &str) -> Option<i64> {
    let year: i64 = date.get(..4)?.parse().ok()?;
    (year > 101).then_some(year)
}

fn date_of(date: &str) -> Option<String> {
    year_of(date)?;
    let day = date.get(..10)?;
    crate::sql::date_ms(day).map(|_| day.to_owned())
}

/// Calibre keeps ratings from 0 to 10, two to a star; a half star rounds up.
fn stars(rating: i64) -> Option<i64> {
    (1..=10).contains(&rating).then(|| (rating + 1) / 2)
}

/// Calibre's languages are ISO 639-2 codes; the ones with a two-letter code take it.
fn language_of(code: &str) -> Option<String> {
    let code = code.trim().to_ascii_lowercase();
    let short = match code.as_str() {
        "por" => "pt",
        "eng" => "en",
        "spa" => "es",
        "fra" | "fre" => "fr",
        "deu" | "ger" => "de",
        "ita" => "it",
        "nld" | "dut" => "nl",
        "rus" => "ru",
        "jpn" => "ja",
        "zho" | "chi" => "zh",
        "kor" => "ko",
        "ara" => "ar",
        "heb" => "he",
        "ell" | "gre" => "el",
        "lat" => "la",
        "pol" => "pl",
        "tur" => "tr",
        "swe" => "sv",
        "dan" => "da",
        "nor" => "no",
        "fin" => "fi",
        "ces" | "cze" => "cs",
        "hun" => "hu",
        "ron" | "rum" => "ro",
        "cat" => "ca",
        "glg" => "gl",
        "eus" | "baq" => "eu",
        "ukr" => "uk",
        "hin" => "hi",
        "fas" | "per" => "fa",
        _ => code.as_str(),
    };
    language_tag(short)
}

/// An identifier Calibre keeps, by its type, as the library keeps it. An ISBN whose check digit is
/// wrong is not one.
fn identifier_of(kind: &str, value: &str) -> Option<Identifier> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let kind = kind.trim().to_ascii_lowercase();
    let (scheme, value) = match kind.as_str() {
        "isbn" => ("isbn", isbn(value)?),
        "issn" => ("issn", value.to_owned()),
        "doi" => ("doi", value.to_owned()),
        "oclc" => ("oclc", value.to_owned()),
        "lccn" => ("lccn", value.to_owned()),
        "amazon" | "asin" | "mobi-asin" => ("asin", value.to_owned()),
        "uri" | "url" => ("uri", value.to_owned()),
        _ => ("other", format!("{kind}:{value}")),
    };
    (value.chars().count() <= 400).then(|| Identifier {
        scheme: scheme.to_owned(),
        value,
    })
}

/// HTML (Calibre's comments) as plain text: one line per paragraph, entities resolved, tags gone.
pub(crate) fn html_to_text(html: &str) -> Option<String> {
    let parsed = musaeum_flow::html::parse(html, 200_000)?;
    let mut out = String::new();
    fn walk(dom: &Dom, node: usize, out: &mut String) {
        match dom.data(node) {
            Data::Text(text) => {
                let collapsed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
                if !collapsed.is_empty() {
                    if !out.is_empty()
                        && !out.ends_with('\n')
                        && !out.ends_with(' ')
                        && text.starts_with(char::is_whitespace)
                    {
                        out.push(' ');
                    }
                    out.push_str(&collapsed);
                    if text.ends_with(char::is_whitespace) {
                        out.push(' ');
                    }
                }
            }
            Data::Element { name, .. } => {
                let block = matches!(
                    name.local.as_str(),
                    "p" | "div"
                        | "br"
                        | "li"
                        | "h1"
                        | "h2"
                        | "h3"
                        | "h4"
                        | "h5"
                        | "h6"
                        | "blockquote"
                        | "tr"
                        | "pre"
                );
                if name.local == "script" || name.local == "style" {
                    return;
                }
                for &child in dom.children(node) {
                    walk(dom, child, out);
                }
                if block {
                    while out.ends_with(' ') {
                        out.pop();
                    }
                    if !out.is_empty() && !out.ends_with("\n\n") {
                        out.push_str(if name.local == "br" { "\n" } else { "\n\n" });
                    }
                }
            }
            Data::Document => {
                for &child in dom.children(node) {
                    walk(dom, child, out);
                }
            }
        }
    }
    walk(&parsed.dom, Dom::DOCUMENT, &mut out);
    let text: String = out
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n");
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    Some(text.chars().take(MAX_DESCRIPTION).collect())
}

fn kind_of(datatype: &str) -> Option<&'static str> {
    match datatype {
        "text" | "comments" | "series" => Some("text"),
        "enumeration" => Some("choice"),
        "float" | "int" | "rating" => Some("number"),
        "datetime" => Some("date"),
        "bool" => Some("boolean"),
        _ => None,
    }
}

/// A short text, whole if it fits.
fn short(text: &str) -> String {
    text.chars().take(MAX_CUSTOM_TEXT).collect()
}

fn series_text(name: &str, index: Option<f64>) -> String {
    match index {
        Some(index) if index.fract() == 0.0 => format!("{name} [{}]", index as i64),
        Some(index) => format!("{name} [{index}]"),
        None => name.to_owned(),
    }
}

/// A custom column's value, from what Calibre keeps, as the custom field takes it.
fn custom_value(
    column: &CalibreColumn,
    values: &[rusqlite::types::Value],
    extra: Option<f64>,
) -> Option<serde_json::Value> {
    use rusqlite::types::Value;
    use serde_json::json;
    let text = |value: &Value| match value {
        Value::Text(text) => Some(text.clone()),
        Value::Integer(number) => Some(number.to_string()),
        Value::Real(number) => Some(number.to_string()),
        _ => None,
    };
    let number = |value: &Value| match value {
        Value::Integer(number) => Some(*number as f64),
        Value::Real(number) => Some(*number),
        Value::Text(text) => text.trim().parse().ok(),
        _ => None,
    };
    let first = values.first()?;
    let value = match column.datatype.as_str() {
        "text" => {
            let parts: Vec<String> = values
                .iter()
                .filter_map(text)
                .filter(|part| !part.trim().is_empty())
                .collect();
            json!(short(&parts.join(", ")))
        }
        "comments" => json!(short(&html_to_text(&text(first)?)?)),
        "series" => json!(short(&series_text(&text(first)?, extra))),
        "enumeration" => {
            let choice = text(first)?;
            if !column.choices.contains(&choice) {
                return None;
            }
            json!(choice)
        }
        "float" | "int" => json!(number(first)?),
        "rating" => json!(number(first)? / 2.0),
        "datetime" => json!(date_of(&text(first)?)?),
        "bool" => json!(number(first)? != 0.0),
        _ => return None,
    };
    match &value {
        serde_json::Value::String(text) if text.trim().is_empty() => None,
        _ => Some(value),
    }
}

fn read_columns(connection: &Connection) -> Result<Vec<(i64, CalibreColumn, bool)>> {
    if !has_table(connection, "custom_columns")? {
        return Ok(Vec::new());
    }
    let mut statement = connection.prepare(
        "SELECT id, label, name, datatype, is_multiple, normalized, display FROM custom_columns
         WHERE mark_for_delete = 0 ORDER BY id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<i64>>(4)?.unwrap_or(0) != 0,
            row.get::<_, Option<i64>>(5)?.unwrap_or(0) != 0,
            row.get::<_, Option<String>>(6)?,
        ))
    })?;
    let mut columns = Vec::new();
    for row in rows {
        let (id, label, name, datatype, is_multiple, normalized, display) = row?;
        let choices: Vec<String> = display
            .as_deref()
            .and_then(|display| serde_json::from_str::<serde_json::Value>(display).ok())
            .and_then(|display| display.get("enum_values").cloned())
            .and_then(|values| serde_json::from_value(values).ok())
            .unwrap_or_default();
        let kind = kind_of(&datatype).map(str::to_owned);
        columns.push((
            id,
            CalibreColumn {
                label,
                name,
                datatype,
                kind,
                choices,
                is_multiple,
            },
            normalized,
        ));
    }
    Ok(columns)
}

type Books = (Vec<CalibreBook>, Vec<CalibreColumn>);

fn read_database(connection: &Connection, source: &Path) -> Result<Books> {
    if !has_table(connection, "books")? {
        return Err(Error::Refused(tr!(CalibreNoBooksTable)));
    }
    let mut books: Vec<CalibreBook> = Vec::new();
    let mut paths: Vec<String> = Vec::new();
    {
        let mut statement = connection.prepare(
            "SELECT id, title, path, series_index, pubdate, uuid, isbn FROM books ORDER BY id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<f64>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        })?;
        for row in rows {
            let (id, title, path, series_index, pubdate, uuid, legacy_isbn) = row?;
            if books.len() >= MAX_BOOKS {
                return Err(Error::Refused(tr!(CalibreTooMany, max = MAX_BOOKS)));
            }
            let title = title
                .map(|title| title.trim().to_owned())
                .filter(|title| !title.is_empty());
            books.push(CalibreBook {
                id,
                uuid: uuid.filter(|uuid| uuid.len() <= 64),
                title: title
                    .unwrap_or_else(|| tr!(Untitled))
                    .chars()
                    .take(1000)
                    .collect(),
                authors: Vec::new(),
                series: None,
                series_index,
                tags: Vec::new(),
                rating: None,
                identifiers: legacy_isbn
                    .as_deref()
                    .and_then(|text| identifier_of("isbn", text))
                    .into_iter()
                    .collect(),
                description: None,
                language: None,
                year: pubdate.as_deref().and_then(year_of),
                files: Vec::new(),
                custom: BTreeMap::new(),
            });
            paths.push(path.unwrap_or_default());
        }
    }
    let index: HashMap<i64, usize> = books
        .iter()
        .enumerate()
        .map(|(at, book)| (book.id, at))
        .collect();
    let mut each = |table: &str,
                    sql: &str,
                    apply: &mut dyn FnMut(
        &mut CalibreBook,
        rusqlite::types::Value,
        rusqlite::types::Value,
    )|
     -> Result<()> {
        if !has_table(connection, table)? {
            return Ok(());
        }
        let mut statement = connection.prepare(sql)?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get(1)?, row.get(2)?))
        })?;
        for row in rows {
            let (book, a, b) = row?;
            if let Some(&at) = index.get(&book) {
                apply(&mut books[at], a, b);
            }
        }
        Ok(())
    };
    use rusqlite::types::Value;
    let text = |value: Value| match value {
        Value::Text(text) => Some(text),
        _ => None,
    };
    each(
        "books_authors_link",
        "SELECT l.book, a.name, NULL FROM books_authors_link l JOIN authors a ON a.id = l.author ORDER BY l.book, l.id",
        &mut |book, name, _| {
            if let Some(name) = text(name)
                .map(|name| name.trim().replace('|', ","))
                .filter(|name| !name.is_empty())
                && book.authors.len() < 100
            {
                book.authors.push(name.chars().take(300).collect());
            }
        },
    )?;
    each(
        "books_series_link",
        "SELECT l.book, s.name, NULL FROM books_series_link l JOIN series s ON s.id = l.series",
        &mut |book, name, _| book.series = text(name).map(|name| name.chars().take(300).collect()),
    )?;
    each(
        "books_tags_link",
        "SELECT l.book, t.name, NULL FROM books_tags_link l JOIN tags t ON t.id = l.tag ORDER BY l.book, l.id",
        &mut |book, name, _| {
            if let Some(name) = text(name)
                .map(|name| name.trim().to_owned())
                .filter(|name| !name.is_empty())
                && name.chars().count() <= MAX_TAG_NAME
                && book.tags.len() < 200
            {
                book.tags.push(name);
            }
        },
    )?;
    each(
        "books_ratings_link",
        "SELECT l.book, r.rating, NULL FROM books_ratings_link l JOIN ratings r ON r.id = l.rating",
        &mut |book, rating, _| {
            if let Value::Integer(rating) = rating {
                book.rating = stars(rating);
            }
        },
    )?;
    each(
        "books_languages_link",
        "SELECT l.book, g.lang_code, NULL FROM books_languages_link l JOIN languages g ON g.id = l.lang_code
         ORDER BY l.book, l.item_order",
        &mut |book, code, _| {
            if book.language.is_none() {
                book.language = text(code).as_deref().and_then(language_of);
            }
        },
    )?;
    each(
        "identifiers",
        "SELECT book, type, val FROM identifiers ORDER BY book, id",
        &mut |book, kind, value| {
            if let (Some(kind), Some(value)) = (text(kind), text(value))
                && let Some(identifier) = identifier_of(&kind, &value)
                && !book.identifiers.contains(&identifier)
                && book.identifiers.len() < MAX_IDENTIFIERS
            {
                book.identifiers.push(identifier);
            }
        },
    )?;
    each(
        "comments",
        "SELECT book, text, NULL FROM comments",
        &mut |book, html, _| {
            book.description = text(html).as_deref().and_then(html_to_text);
        },
    )?;
    // The files: the book's folder, and each format's file in it.
    if has_table(connection, "data")? {
        let mut statement =
            connection.prepare("SELECT book, format, name FROM data ORDER BY book, id")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?;
        for row in rows {
            let (book, format, name) = row?;
            let (Some(&at), Some(format), Some(name)) = (index.get(&book), format, name) else {
                continue;
            };
            let Some(path) = book_file(source, &paths[at], &name, &format) else {
                continue;
            };
            if books[at].files.len() < 32 && !books[at].files.iter().any(|file| file.path == path) {
                let exists = path.is_file();
                books[at].files.push(CalibreFile { path, exists });
            }
        }
    }
    let columns = read_columns(connection)?;
    for (id, column, normalized) in &columns {
        if column.kind.is_none() {
            continue;
        }
        let values_table = format!("custom_column_{id}");
        let link_table = format!("books_custom_column_{id}_link");
        let mut found: BTreeMap<i64, (Vec<Value>, Option<f64>)> = BTreeMap::new();
        if *normalized {
            if !has_table(connection, &values_table)? || !has_table(connection, &link_table)? {
                continue;
            }
            let extra = if column.datatype == "series" {
                "l.extra"
            } else {
                "NULL"
            };
            // The tables' names are made from the column's number, never from its text.
            let mut statement = connection.prepare(&format!(
                "SELECT l.book, v.value, {extra} FROM {link_table} l JOIN {values_table} v ON v.id = l.value
                 ORDER BY l.book, l.id"
            ))?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Value>(1)?,
                    row.get::<_, Option<f64>>(2)?,
                ))
            })?;
            for row in rows {
                let (book, value, extra) = row?;
                let entry = found.entry(book).or_default();
                entry.0.push(value);
                entry.1 = entry.1.or(extra);
            }
        } else {
            if !has_table(connection, &values_table)? {
                continue;
            }
            let mut statement = connection.prepare(&format!(
                "SELECT book, value FROM {values_table} ORDER BY book, id"
            ))?;
            let rows = statement.query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Value>(1)?))
            })?;
            for row in rows {
                let (book, value) = row?;
                found.entry(book).or_default().0.push(value);
            }
        }
        for (book, (values, extra)) in found {
            if let Some(&at) = index.get(&book)
                && let Some(value) = custom_value(column, &values, extra)
            {
                books[at].custom.insert(column.label.clone(), value);
            }
        }
    }
    Ok((
        books,
        columns.into_iter().map(|(_, column, _)| column).collect(),
    ))
}

/// The path of a book's file: its folder (Calibre's `path`, with `/` between parts) and the
/// format's file in it. A part that would leave the library's folder is not followed.
fn book_file(source: &Path, folder: &str, name: &str, format: &str) -> Option<PathBuf> {
    let mut path = source.to_path_buf();
    for part in folder.split('/') {
        if part.is_empty() || part == "." || part == ".." || part.contains(['\\', ':']) {
            return None;
        }
        path.push(part);
    }
    let format = format.trim().to_ascii_lowercase();
    if name.contains(['/', '\\', ':'])
        || format.is_empty()
        || !format.chars().all(|c| c.is_ascii_alphanumeric())
    {
        return None;
    }
    path.push(format!("{name}.{format}"));
    Some(path)
}

// --- metadata.opf, when there is no database ---

const DC: &str = "http://purl.org/dc/elements/1.1/";
const OPF: &str = "http://www.idpf.org/2007/opf";

fn elements(dom: &Dom, node: usize, out: &mut Vec<usize>) {
    for &child in dom.children(node) {
        if dom.is_element(child) {
            out.push(child);
            elements(dom, child, out);
        }
    }
}

fn read_opfs(source: &Path) -> Result<Books> {
    let mut books = Vec::new();
    let mut columns: BTreeMap<String, CalibreColumn> = BTreeMap::new();
    let mut next_id = 1_000_000_000;
    let mut folders: Vec<PathBuf> = Vec::new();
    let entries = |dir: &Path| -> Vec<PathBuf> {
        let mut found: Vec<PathBuf> = fs::read_dir(dir)
            .map(|entries| {
                entries
                    .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                    .collect()
            })
            .unwrap_or_default();
        found.sort();
        found
    };
    for author in entries(source).into_iter().filter(|path| path.is_dir()) {
        for book in entries(&author).into_iter().filter(|path| path.is_dir()) {
            if book.join("metadata.opf").is_file() {
                folders.push(book);
            }
            if folders.len() >= MAX_BOOKS {
                break;
            }
        }
    }
    for folder in folders {
        let Ok(bytes) = fs::read(folder.join("metadata.opf")) else {
            continue;
        };
        if bytes.len() > 4 << 20 {
            continue;
        }
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        let Ok(dom) = xml::parse(
            &text,
            &Limits {
                max_nodes: 50_000,
                ..Limits::default()
            },
        ) else {
            continue;
        };
        let dc = Ns::Other(DC.to_owned());
        let opf = Ns::Other(OPF.to_owned());
        let mut all = Vec::new();
        elements(&dom, Dom::DOCUMENT, &mut all);
        let mut book = CalibreBook {
            id: 0,
            uuid: None,
            title: String::new(),
            authors: Vec::new(),
            series: None,
            series_index: None,
            tags: Vec::new(),
            rating: None,
            identifiers: Vec::new(),
            description: None,
            language: None,
            year: None,
            files: Vec::new(),
            custom: BTreeMap::new(),
        };
        for &node in &all {
            let content = || xml::text_of(&dom, node).trim().to_owned();
            if dom.is(node, &dc, "title") && book.title.is_empty() {
                book.title = content().chars().take(1000).collect();
            } else if dom.is(node, &dc, "creator") {
                let role = dom.attr_ns(node, &opf, "role").unwrap_or("aut");
                let name = content();
                if role == "aut" && !name.is_empty() && book.authors.len() < 100 {
                    book.authors.push(name.chars().take(300).collect());
                }
            } else if dom.is(node, &dc, "subject") {
                let tag = content();
                if !tag.is_empty() && tag.chars().count() <= MAX_TAG_NAME && book.tags.len() < 200 {
                    book.tags.push(tag);
                }
            } else if dom.is(node, &dc, "identifier") {
                let scheme = dom
                    .attr_ns(node, &opf, "scheme")
                    .unwrap_or("")
                    .to_ascii_lowercase();
                let value = content();
                match scheme.as_str() {
                    "calibre" => book.id = value.parse().unwrap_or(0),
                    "uuid" => book.uuid = Some(value).filter(|uuid| uuid.len() <= 64),
                    _ => {
                        if let Some(identifier) = identifier_of(&scheme, &value)
                            && !book.identifiers.contains(&identifier)
                        {
                            book.identifiers.push(identifier);
                        }
                    }
                }
            } else if dom.is(node, &dc, "language") {
                book.language = book.language.take().or_else(|| language_of(&content()));
            } else if dom.is(node, &dc, "date") {
                book.year = year_of(&content());
            } else if dom.is(node, &dc, "description") {
                book.description = html_to_text(&content());
            } else if dom.name(node).is_some_and(|name| name.local == "meta") {
                let (Some(name), Some(value)) = (dom.attr(node, "name"), dom.attr(node, "content"))
                else {
                    continue;
                };
                match name {
                    "calibre:series" => {
                        book.series = Some(value.trim().chars().take(300).collect())
                            .filter(|s: &String| !s.is_empty())
                    }
                    "calibre:series_index" => book.series_index = value.trim().parse().ok(),
                    "calibre:rating" => {
                        book.rating = value
                            .trim()
                            .parse::<f64>()
                            .ok()
                            .and_then(|rating| stars(rating.round() as i64))
                    }
                    _ => {
                        if let Some(label) = name.strip_prefix("calibre:user_metadata:#")
                            && let Ok(meta) = serde_json::from_str::<serde_json::Value>(value)
                        {
                            opf_custom(&mut columns, &mut book, label, &meta);
                        }
                    }
                }
            }
        }
        if book.title.is_empty() {
            continue;
        }
        if book.id == 0 {
            next_id += 1;
            book.id = next_id;
        }
        for path in entries(&folder) {
            if path.is_file()
                && path
                    .file_name()
                    .is_some_and(|name| format_of(name).is_some())
                && book.files.len() < 32
            {
                book.files.push(CalibreFile { path, exists: true });
            }
        }
        books.push(book);
    }
    Ok((books, columns.into_values().collect()))
}

fn opf_custom(
    columns: &mut BTreeMap<String, CalibreColumn>,
    book: &mut CalibreBook,
    label: &str,
    meta: &serde_json::Value,
) {
    use rusqlite::types::Value;
    let datatype = meta
        .get("datatype")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_owned();
    let column = columns
        .entry(label.to_owned())
        .or_insert_with(|| CalibreColumn {
            label: label.to_owned(),
            name: meta
                .get("name")
                .and_then(|value| value.as_str())
                .unwrap_or(label)
                .to_owned(),
            kind: kind_of(&datatype).map(str::to_owned),
            choices: meta
                .pointer("/display/enum_values")
                .and_then(|values| serde_json::from_value(values.clone()).ok())
                .unwrap_or_default(),
            is_multiple: meta
                .get("is_multiple")
                .is_some_and(|value| !value.is_null() && value != &serde_json::json!({})),
            datatype,
        });
    let to_sql = |value: &serde_json::Value| match value {
        serde_json::Value::String(text) => Some(Value::Text(text.clone())),
        serde_json::Value::Number(number) => number
            .as_i64()
            .map(Value::Integer)
            .or_else(|| number.as_f64().map(Value::Real)),
        serde_json::Value::Bool(flag) => Some(Value::Integer(i64::from(*flag))),
        _ => None,
    };
    let values: Vec<Value> = match meta.get("#value#") {
        Some(serde_json::Value::Array(items)) => items.iter().filter_map(to_sql).collect(),
        Some(value) => to_sql(value).into_iter().collect(),
        None => Vec::new(),
    };
    let extra = meta.get("#extra#").and_then(serde_json::Value::as_f64);
    if !values.is_empty()
        && let Some(value) = custom_value(column, &values, extra)
    {
        book.custom.insert(label.to_owned(), value);
    }
}

// --- Bringing it in ---

fn origin(transaction: &Transaction<'_>, work: WorkId, field: &str) -> Result<Option<String>> {
    Ok(transaction
        .prepare_cached("SELECT origin FROM field_provenance WHERE work_id = ?1 AND field = ?2")?
        .query_row(params![work, field], |row| row.get(0))
        .optional()?)
}

/// A column's custom field: its id, kind and choices.
type ColumnField = (CustomFieldId, String, Vec<String>);

/// The custom field for each column, made if there is none of that name and kind.
fn fields_for(
    transaction: &Transaction<'_>,
    columns: &[CalibreColumn],
    report: &mut CalibreReport,
) -> Result<HashMap<String, ColumnField>> {
    let mut fields = HashMap::new();
    for column in columns {
        let Some(kind) = &column.kind else { continue };
        let Ok(name) = taxonomy::clean_name(&column.name, MAX_TAG_NAME, T::ThingField) else {
            continue;
        };
        let choices = clean_choices(kind, &column.choices).unwrap_or_default();
        let existing: Option<(CustomFieldId, String, String)> = transaction
            .prepare_cached("SELECT id, kind, choices FROM custom_field WHERE name_key = ?1")?
            .query_row([fold(&name)], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .optional()?;
        let field = match existing {
            Some((id, existing_kind, existing_choices)) if existing_kind == *kind => (
                id,
                existing_kind,
                serde_json::from_str(&existing_choices).unwrap_or_default(),
            ),
            // A field of that name and another kind is the person's; Calibre's gets a name of its own.
            Some(_) => {
                let other = format!("{name} (Calibre)");
                let found: Option<CustomFieldId> = transaction
                    .prepare_cached(
                        "SELECT id FROM custom_field WHERE name_key = ?1 AND kind = ?2",
                    )?
                    .query_row(params![fold(&other), kind], |row| row.get(0))
                    .optional()?;
                match found {
                    Some(id) => (id, kind.clone(), choices.clone()),
                    None => {
                        report.fields_created += 1;
                        (
                            insert_custom_field(transaction, &other, kind, &choices)?,
                            kind.clone(),
                            choices.clone(),
                        )
                    }
                }
            }
            None => {
                report.fields_created += 1;
                (
                    insert_custom_field(transaction, &name, kind, &choices)?,
                    kind.clone(),
                    choices.clone(),
                )
            }
        };
        fields.insert(column.label.clone(), field);
    }
    Ok(fields)
}

fn apply_book(
    transaction: &Transaction<'_>,
    work: WorkId,
    book: &CalibreBook,
    fields: &HashMap<String, (CustomFieldId, String, Vec<String>)>,
    now: i64,
) -> Result<()> {
    use rusqlite::types::Value;
    let free = |field: &str| -> Result<bool> {
        Ok(origin(transaction, work, field)?.as_deref() != Some("user"))
    };
    let set = |field: &'static str, column: &'static str, value: Value| -> Result<()> {
        if free(field)? {
            // The column comes from this list, not from the library being read.
            transaction.execute(
                &format!("UPDATE work SET {column} = ?2 WHERE id = ?1"),
                params![work, value],
            )?;
            set_provenance(transaction, work, field, "calibre", now)?;
        }
        Ok(())
    };
    let text = |value: &Option<String>| value.clone().map_or(Value::Null, Value::Text);
    set("title", "title", Value::Text(book.title.clone()))?;
    if book.series.is_some() {
        set("series", "series", text(&book.series))?;
        set(
            "series_position",
            "series_position",
            book.series_index
                .filter(|index| index.is_finite() && (0.0..=1_000_000.0).contains(index))
                .map_or(Value::Null, Value::Real),
        )?;
    }
    if let Some(year) = book.year.filter(|year| (-9999..=9999).contains(year)) {
        set("year", "year", Value::Integer(year))?;
    }
    if book.language.is_some() {
        set("language", "language", text(&book.language))?;
    }
    if let Some(rating) = book.rating {
        set("rating", "rating", Value::Integer(rating))?;
    }
    if book.description.is_some() {
        set("description", "description", text(&book.description))?;
    }
    if !book.authors.is_empty() && free("creators")? {
        let credits: Vec<Credit> = book
            .authors
            .iter()
            .map(|name| Credit::author(name.clone()))
            .collect();
        set_credits(transaction, work, &credits)?;
        set_provenance(transaction, work, "creators", "calibre", now)?;
    }
    if !book.identifiers.is_empty() && free("identifiers")? {
        write_identifiers(transaction, work, &book.identifiers)?;
        set_provenance(transaction, work, "identifiers", "calibre", now)?;
    }
    for name in &book.tags {
        let tag = taxonomy::tag_named(transaction, name)?;
        transaction
            .prepare_cached(
                "INSERT OR IGNORE INTO work_tag (work_id, tag_id, added_at) VALUES (?1, ?2, ?3)",
            )?
            .execute(params![work, tag, now])?;
    }
    for (label, value) in &book.custom {
        let Some((field, kind, choices)) = fields.get(label) else {
            continue;
        };
        let Ok(value) = custom_sql(kind, choices, value) else {
            continue;
        };
        transaction
            .prepare_cached(
                "INSERT OR IGNORE INTO work_custom_value (work_id, field_id, value, updated_at) VALUES (?1, ?2, ?3, ?4)",
            )?
            .execute(params![work, field, value, now])?;
    }
    transaction
        .prepare_cached("UPDATE work SET updated_at = ?2 WHERE id = ?1")?
        .execute(params![work, now])?;
    search::refresh_entry(transaction, work)?;
    Ok(())
}

fn import(transaction: &Transaction<'_>, calibre: &CalibreLibrary) -> Result<CalibreReport> {
    let now = now_ms();
    let source = calibre.source.to_string_lossy().into_owned();
    let mut report = CalibreReport {
        books: calibre.books.len(),
        ..CalibreReport::default()
    };
    let fields = fields_for(transaction, &calibre.columns, &mut report)?;
    for book in &calibre.books {
        let present: Vec<&CalibreFile> = book.files.iter().filter(|file| file.exists).collect();
        if present.is_empty() {
            report.missing += 1;
            continue;
        }
        // Every file on the disk known and hashed, or the book waits: half a book is not brought in.
        let mut works: Vec<WorkId> = Vec::new();
        let mut ready = true;
        for file in &present {
            let Some(instance) = locate_instance(transaction, &file.path)? else {
                ready = false;
                break;
            };
            let row: Option<(WorkId, Option<Vec<u8>>)> = transaction
                .prepare_cached(
                    "SELECT d.work_id, r.blake3 FROM file_instance i JOIN revision r ON r.id = i.revision_id
                     JOIN document d ON d.id = r.document_id WHERE i.id = ?1 AND i.state = 'present'",
                )?
                .query_row([instance], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()?;
            match row {
                Some((work, Some(_))) => {
                    if !works.contains(&work) {
                        works.push(work);
                    }
                }
                _ => {
                    ready = false;
                    break;
                }
            }
        }
        if !ready {
            report.waiting += 1;
            continue;
        }
        // The work it was brought in as before, if it is one of these; else the oldest.
        let before: Option<WorkId> = transaction
            .prepare_cached(
                "SELECT work_id FROM calibre_book WHERE source = ?1 AND calibre_id = ?2",
            )?
            .query_row(params![source, book.id], |row| row.get(0))
            .optional()?;
        works.sort_unstable();
        let keep = before
            .filter(|work| works.contains(work))
            .unwrap_or(works[0]);
        let others: Vec<WorkId> = works.iter().copied().filter(|&work| work != keep).collect();
        if !others.is_empty() {
            merge_into(transaction, keep, &others)?;
            report.merged += others.len();
        }
        apply_book(transaction, keep, book, &fields, now)?;
        transaction
            .prepare_cached(
                "INSERT INTO calibre_book (source, calibre_id, calibre_uuid, work_id, imported_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (source, calibre_id) DO UPDATE SET calibre_uuid = excluded.calibre_uuid,
                     work_id = excluded.work_id, imported_at = excluded.imported_at",
            )?
            .execute(params![source, book.id, book.uuid, keep, now])?;
        report.imported += 1;
    }
    Ok(report)
}

impl Library {
    /// Brings in what Calibre says about the books of a library read with [`read_calibre`], for
    /// those whose files this library already knows by their bytes. Run again once the rest are
    /// read; nothing is brought in twice.
    pub fn import_calibre(&self, calibre: &CalibreLibrary) -> Result<CalibreReport> {
        let calibre = calibre.clone();
        self.database()
            .write(move |transaction| import(transaction, &calibre))
    }

    /// The Calibre books brought in from a folder before, by Calibre's id, and their works now.
    pub fn calibre_books(&self, source: &Path) -> Result<Vec<(i64, WorkId)>> {
        let source = absolute_if_it_exists(source).to_string_lossy().into_owned();
        self.database().read(move |connection| {
            Ok(connection
                .prepare_cached("SELECT calibre_id, work_id FROM calibre_book WHERE source = ?1 ORDER BY calibre_id")?
                .query_map([source], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calibre_s_values_become_the_library_s() {
        assert_eq!(stars(10), Some(5));
        assert_eq!(stars(9), Some(5));
        assert_eq!(stars(4), Some(2));
        assert_eq!(stars(1), Some(1));
        assert_eq!(stars(0), None);
        assert_eq!(year_of("1990-06-01 00:00:00+00:00"), Some(1990));
        assert_eq!(year_of("0101-01-01 00:00:00+00:00"), None);
        assert_eq!(
            date_of("2021-03-04 10:00:00+00:00").as_deref(),
            Some("2021-03-04")
        );
        assert_eq!(language_of("por").as_deref(), Some("pt"));
        assert_eq!(language_of("gre").as_deref(), Some("el"));
        assert_eq!(language_of("tlh").as_deref(), Some("tlh"));
        assert_eq!(
            identifier_of("isbn", "978-85-359-0277-8").unwrap().value,
            "9788535902778"
        );
        assert!(identifier_of("isbn", "978-85-359-0277-9").is_none());
        assert_eq!(identifier_of("amazon", "B00ABC").unwrap().scheme, "asin");
        assert_eq!(
            identifier_of("goodreads", "123").unwrap().value,
            "goodreads:123"
        );
        assert_eq!(series_text("Fundação", Some(2.0)), "Fundação [2]");
        assert_eq!(series_text("Fundação", Some(2.5)), "Fundação [2.5]");
    }

    #[test]
    fn comments_become_plain_text() {
        assert_eq!(
            html_to_text("<div><p>Um livro <b>sobre</b> fósseis.</p><p>Segunda&nbsp;parte &amp; fim.</p></div>").as_deref(),
            // A non-breaking space is a space in plain text.
            Some("Um livro sobre fósseis.\n\nSegunda parte & fim.")
        );
        assert_eq!(html_to_text("<p> </p>"), None);
        assert_eq!(
            html_to_text("<script>alert(1)</script><p>texto</p>").as_deref(),
            Some("texto")
        );
    }

    #[test]
    fn a_book_s_file_stays_in_the_library_s_folder() {
        let source = Path::new("C:/Calibre");
        assert_eq!(
            book_file(
                source,
                "Machado de Assis/Dom Casmurro (3)",
                "Dom Casmurro - Machado de Assis",
                "EPUB"
            ),
            Some(PathBuf::from(
                "C:/Calibre/Machado de Assis/Dom Casmurro (3)/Dom Casmurro - Machado de Assis.epub"
            ))
        );
        assert_eq!(book_file(source, "../fora/x", "x", "epub"), None);
        assert_eq!(book_file(source, "a/b", "..\\x", "epub"), None);
        assert_eq!(book_file(source, "a/b", "x", "ep/ub"), None);
    }

    #[test]
    fn the_database_is_named_as_sqlite_reads_a_uri() {
        assert_eq!(
            file_uri(Path::new(r"C:\Livros do Calibre\metadata.db")),
            "file:///C:/Livros%20do%20Calibre/metadata.db?mode=ro&immutable=1"
        );
        assert!(file_uri(Path::new(r"C:\a#b?c\metadata.db")).starts_with("file:///C:/a%23b%3Fc/"));
    }
}

//! What the library shows a person: its books, one per document, in a folder or everywhere, by
//! title or as a search finds them, in a category, with a tag, in a collection (in its order) or
//! as a smart collection's rule finds them; and its folders.
//!
//! A document can be in several places at once. The one listed, and the one opened, is the best of
//! them: a file that is there before one only in the cloud, before one that could not be read, and
//! the newest revision first. A document whose files are all gone is not listed.

use std::collections::HashMap;
use std::path::PathBuf;

use rusqlite::{Connection, params};
use serde::Serialize;

use crate::error::Result;
use crate::fingerprint::Hash;
use crate::identity::{folder_path, root_path};
use crate::library::Library;
use crate::model::{DocumentId, FolderId, InstanceId, RevisionId, RootId, WorkId, hex_of};
use crate::search;
use crate::sql::StoredName;
use crate::taxonomy::{CategoryId, CollectionId, SmartCollectionId, TagId, smart_condition};

/// Most books one listing returns.
pub const MAX_BROWSE: usize = 5000;
/// Most works a search puts in a listing, best first.
const MAX_SEARCH_HITS: usize = 5000;

/// Which books to list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BrowseQuery {
    /// What was typed in the search field. Empty lists every book by title.
    pub text: String,
    /// Only books in this folder or below it.
    pub folder: Option<FolderId>,
    /// Only books in this category or in one under it.
    pub category: Option<CategoryId>,
    /// Only books with this tag.
    pub tag: Option<TagId>,
    /// Only books in this collection, in its order.
    pub collection: Option<CollectionId>,
    /// Only books this smart collection's rule finds now.
    pub smart: Option<SmartCollectionId>,
    /// Books to skip, for a listing read in pages.
    pub offset: usize,
    /// Books to return, at most [`MAX_BROWSE`].
    pub limit: usize,
}

/// One book as the library lists it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BookRow {
    pub document_id: DocumentId,
    pub work_id: WorkId,
    pub title: String,
    /// Its authors, in order, separated by `; `.
    pub creators: Option<String>,
    /// `fixed` or `flow`.
    pub layout: String,
    /// `pdf`, `epub`, `cbz`… by the file's name.
    pub format: String,
    /// The file listed for it, and what is known about that file.
    pub instance_id: InstanceId,
    pub revision_id: RevisionId,
    pub folder_id: FolderId,
    pub path: String,
    /// `present`, `cloud_only` or `unreadable`.
    pub state: String,
    /// The file's BLAKE3 in hex, once it was hashed; thumbnails and covers are kept by it.
    pub blake3: Option<String>,
    pub size: u64,
    /// How far the reader got, if they opened it.
    pub progress: Option<f64>,
    /// When they last read it, in milliseconds since the Unix epoch.
    pub last_read_at: Option<i64>,
}

/// A page of a listing.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowsePage {
    /// How many books match, in all pages.
    pub total: usize,
    pub offset: usize,
    pub rows: Vec<BookRow>,
}

/// A folder of a root, as the filter shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderEntry {
    pub id: FolderId,
    pub parent_id: Option<FolderId>,
    pub root_id: RootId,
    /// Its name; for a root's own folder, the root's whole path.
    pub name: String,
    /// Books directly in it.
    pub books: u64,
}

/// The file to open for a document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BookFile {
    pub document_id: DocumentId,
    pub revision_id: RevisionId,
    pub instance_id: InstanceId,
    pub path: PathBuf,
    /// `present`, `cloud_only` or `unreadable`.
    pub state: String,
    pub layout: String,
    pub format: String,
    pub title: String,
    /// `None` until the file was hashed.
    pub blake3: Option<Hash>,
}

/// Every document's best file, in the folder `?1` and below when it is not NULL.
const CANDIDATES: &str = "
    WITH RECURSIVE scope(id) AS (
        SELECT id FROM folder WHERE id = ?1
        UNION ALL
        SELECT f.id FROM folder f JOIN scope ON f.parent_id = scope.id
    ),
    candidates AS (
        SELECT r.document_id, i.id AS instance_id,
               row_number() OVER (
                   PARTITION BY r.document_id
                   ORDER BY CASE i.state WHEN 'present' THEN 0 WHEN 'cloud_only' THEN 1 ELSE 2 END,
                            r.id DESC, i.id
               ) AS choice
        FROM file_instance i JOIN revision r ON r.id = i.revision_id
        WHERE i.state <> 'absent' AND (?1 IS NULL OR i.folder_id IN (SELECT id FROM scope))
    ),
    hits(work_id, rank) AS (
        SELECT CAST(value AS INTEGER), CAST(key AS INTEGER) FROM json_each(?2)
    ),
    chosen(work_id, rank) AS (
        SELECT CAST(value AS INTEGER), CAST(key AS INTEGER) FROM json_each(?5)
    )";

impl Library {
    /// Lists books: every one by title, or those a search finds, best match first.
    pub fn browse(&self, query: &BrowseQuery) -> Result<BrowsePage> {
        let query = query.clone();
        self.database()
            .read(move |connection| browse(connection, &query))
    }

    /// Every folder of every root, with how many books each holds directly.
    pub fn folders(&self) -> Result<Vec<FolderEntry>> {
        self.database().read(folders)
    }

    /// The file to open for a document, if it still has one.
    pub fn book_file(&self, document: DocumentId) -> Result<Option<BookFile>> {
        self.database()
            .read(move |connection| book_files(connection, Some(document)))
            .map(|mut files| files.pop())
    }

    /// The file to open for every document that has one there, hashed, for work that is kept by
    /// the bytes, such as covers.
    pub fn hashed_book_files(&self) -> Result<Vec<BookFile>> {
        self.database().read(|connection| {
            Ok(book_files(connection, None)?
                .into_iter()
                .filter(|file| file.state == "present" && file.blake3.is_some())
                .collect())
        })
    }
}

/// The works the taxonomy's filters of `query` leave, in a collection's order if it names one;
/// None if it names none.
fn chosen_works(connection: &Connection, query: &BrowseQuery) -> Result<Option<Vec<WorkId>>> {
    let mut sets: Vec<Vec<WorkId>> = Vec::new();
    let list = |sql: &str, id: i64| -> Result<Vec<WorkId>> {
        Ok(connection
            .prepare_cached(sql)?
            .query_map([id], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    };
    // The collection first: its order is the listing's.
    if let Some(collection) = query.collection {
        sets.push(list(
            "SELECT work_id FROM collection_item WHERE collection_id = ?1
             ORDER BY sort_key, work_id",
            collection,
        )?);
    }
    if let Some(category) = query.category {
        sets.push(list(
            "SELECT DISTINCT wc.work_id FROM category top
             JOIN category c ON substr(c.path, 1, length(top.path)) = top.path
             JOIN work_category wc ON wc.category_id = c.id
             WHERE top.id = ?1",
            category,
        )?);
    }
    if let Some(tag) = query.tag {
        sets.push(list("SELECT work_id FROM work_tag WHERE tag_id = ?1", tag)?);
    }
    if let Some(smart) = query.smart {
        let (condition, values) = smart_condition(connection, smart)?;
        sets.push(
            connection
                .prepare(&format!("SELECT w.id FROM work w WHERE {condition}"))?
                .query_map(rusqlite::params_from_iter(values), |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?,
        );
    }
    let mut sets = sets.into_iter();
    let Some(mut works) = sets.next() else {
        return Ok(None);
    };
    for other in sets {
        let other: std::collections::HashSet<WorkId> = other.into_iter().collect();
        works.retain(|work| other.contains(work));
    }
    Ok(Some(works))
}

fn browse(connection: &Connection, query: &BrowseQuery) -> Result<BrowsePage> {
    let limit = query.limit.min(MAX_BROWSE);
    let chosen = match chosen_works(connection, query)? {
        Some(works) if works.is_empty() => {
            return Ok(BrowsePage {
                total: 0,
                offset: query.offset,
                rows: Vec::new(),
            });
        }
        Some(works) => Some(serde_json::to_string(&works).expect("numbers are JSON")),
        None => None,
    };
    let in_order = query.collection.is_some();
    let text = query.text.trim();
    let hits = if text.is_empty() {
        None
    } else {
        let found = search::search(connection, text, MAX_SEARCH_HITS)?;
        if found.is_empty() {
            return Ok(BrowsePage {
                total: 0,
                offset: query.offset,
                rows: Vec::new(),
            });
        }
        let ids: Vec<WorkId> = found.iter().map(|hit| hit.work_id).collect();
        Some(serde_json::to_string(&ids).expect("numbers are JSON"))
    };

    let total: i64 = connection
        .prepare_cached(&format!(
            "{CANDIDATES}
             SELECT count(*) FROM candidates k
             JOIN document d ON d.id = k.document_id
             LEFT JOIN hits h ON h.work_id = d.work_id
             LEFT JOIN chosen c ON c.work_id = d.work_id
             WHERE k.choice = 1 AND (?2 IS NULL OR h.work_id IS NOT NULL)
                 AND (?5 IS NULL OR c.work_id IS NOT NULL)"
        ))?
        .query_row(
            params![query.folder, hits, None::<i64>, None::<i64>, chosen],
            |row| row.get(0),
        )?;

    let mut statement = connection.prepare_cached(&format!(
        "{CANDIDATES}
         SELECT d.id, d.work_id, w.title, d.layout, d.format, i.id, r.id, i.folder_id, i.name,
                i.state, r.blake3, r.size, s.progress, s.updated_at,
                (SELECT group_concat(name, '; ') FROM (
                    SELECT c.name FROM work_creator wc JOIN creator c ON c.id = wc.creator_id
                    WHERE wc.work_id = w.id ORDER BY wc.position))
         FROM candidates k
         JOIN document d ON d.id = k.document_id
         JOIN work w ON w.id = d.work_id
         JOIN file_instance i ON i.id = k.instance_id
         JOIN revision r ON r.id = i.revision_id
         LEFT JOIN reading_state s ON s.document_id = d.id
         LEFT JOIN hits h ON h.work_id = d.work_id
         LEFT JOIN chosen c ON c.work_id = d.work_id
         WHERE k.choice = 1 AND (?2 IS NULL OR h.work_id IS NOT NULL)
             AND (?5 IS NULL OR c.work_id IS NOT NULL)
         ORDER BY CASE WHEN ?6 THEN c.rank ELSE 0 END,
                  CASE WHEN ?2 IS NULL THEN 0 ELSE h.rank END, w.title COLLATE NOCASE, d.id
         LIMIT ?3 OFFSET ?4"
    ))?;
    let rows = statement
        .query_map(
            params![
                query.folder,
                hits,
                limit as i64,
                query.offset as i64,
                chosen,
                in_order
            ],
            |row| {
                Ok((
                    BookRow {
                        document_id: row.get(0)?,
                        work_id: row.get(1)?,
                        title: row.get(2)?,
                        layout: row.get(3)?,
                        format: row.get(4)?,
                        instance_id: row.get(5)?,
                        revision_id: row.get(6)?,
                        folder_id: row.get(7)?,
                        path: String::new(),
                        state: row.get(9)?,
                        blake3: hex_of(row.get(10)?),
                        size: row.get::<_, i64>(11)?.max(0) as u64,
                        progress: row.get(12)?,
                        last_read_at: row.get(13)?,
                        creators: row.get(14)?,
                    },
                    row.get::<_, StoredName>(8)?.0,
                ))
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut folders: HashMap<FolderId, PathBuf> = HashMap::new();
    let mut listed = Vec::with_capacity(rows.len());
    for (mut row, name) in rows {
        let folder = match folders.get(&row.folder_id) {
            Some(path) => path.clone(),
            None => {
                let path = folder_path(connection, row.folder_id)?;
                folders.insert(row.folder_id, path.clone());
                path
            }
        };
        row.path = folder.join(name).to_string_lossy().into_owned();
        listed.push(row);
    }
    Ok(BrowsePage {
        total: total.max(0) as usize,
        offset: query.offset,
        rows: listed,
    })
}

fn folders(connection: &Connection) -> Result<Vec<FolderEntry>> {
    let mut statement = connection.prepare_cached(
        "SELECT f.id, f.parent_id, f.root_id, f.name,
                (SELECT count(DISTINCT r.document_id) FROM file_instance i
                 JOIN revision r ON r.id = i.revision_id
                 WHERE i.folder_id = f.id AND i.state <> 'absent')
         FROM folder f JOIN library_root lr ON lr.id = f.root_id
         WHERE f.state = 'present' AND lr.status <> 'removed'
         ORDER BY f.root_id, f.id",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, FolderId>(0)?,
                row.get::<_, Option<FolderId>>(1)?,
                row.get::<_, RootId>(2)?,
                row.get::<_, StoredName>(3)?.0,
                row.get::<_, i64>(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut entries = Vec::with_capacity(rows.len());
    for (id, parent_id, root_id, name, books) in rows {
        let name = if parent_id.is_none() {
            root_path(connection, root_id)?
                .to_string_lossy()
                .into_owned()
        } else {
            name.to_string_lossy().into_owned()
        };
        entries.push(FolderEntry {
            id,
            parent_id,
            root_id,
            name,
            books: books.max(0) as u64,
        });
    }
    Ok(entries)
}

fn book_files(connection: &Connection, document: Option<DocumentId>) -> Result<Vec<BookFile>> {
    let mut statement = connection.prepare_cached(&format!(
        "{CANDIDATES}
         SELECT d.id, r.id, i.id, i.folder_id, i.name, i.state, d.layout, d.format, w.title,
                r.blake3
         FROM candidates k
         JOIN document d ON d.id = k.document_id
         JOIN work w ON w.id = d.work_id
         JOIN file_instance i ON i.id = k.instance_id
         JOIN revision r ON r.id = i.revision_id
         WHERE k.choice = 1 AND (?3 IS NULL OR d.id = ?3) AND ?2 IS NULL AND ?5 IS NULL
         ORDER BY d.id"
    ))?;
    let rows = statement
        .query_map(
            params![
                None::<FolderId>,
                None::<String>,
                document,
                None::<i64>,
                None::<String>
            ],
            |row| {
                Ok((
                    row.get::<_, DocumentId>(0)?,
                    row.get::<_, RevisionId>(1)?,
                    row.get::<_, InstanceId>(2)?,
                    row.get::<_, FolderId>(3)?,
                    row.get::<_, StoredName>(4)?.0,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, Option<Vec<u8>>>(9)?,
                ))
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut folders: HashMap<FolderId, PathBuf> = HashMap::new();
    let mut files = Vec::with_capacity(rows.len());
    for (document_id, revision_id, instance_id, folder, name, state, layout, format, title, hash) in
        rows
    {
        let directory = match folders.get(&folder) {
            Some(path) => path.clone(),
            None => {
                let path = folder_path(connection, folder)?;
                folders.insert(folder, path.clone());
                path
            }
        };
        files.push(BookFile {
            document_id,
            revision_id,
            instance_id,
            path: directory.join(name),
            state,
            layout,
            format,
            title,
            blake3: hash.and_then(|bytes| bytes.try_into().ok()),
        });
    }
    Ok(files)
}

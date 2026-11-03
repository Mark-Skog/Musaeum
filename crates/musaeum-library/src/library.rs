//! The library as the rest of the program sees it.

use std::ffi::OsString;
use std::fs::{self, File};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use musaeum_fs::{OpenError, ReadPattern};
use musaeum_ingest::EXTRACTOR_VERSION;
use musaeum_ingest::parse::Extracted;
use musaeum_ingest::protocol::Format;
use rusqlite::{Connection, OptionalExtension, params};

use crate::content::{
    self, ContentReport, apply_extraction, merge_empty_document, set_creators, set_provenance,
};
use crate::db::backup::{self, BackupInfo, BackupPolicy};
use crate::db::migrate;
use crate::db::{CACHE_SPEC, Database, LIBRARY_APPLICATION_ID, LIBRARY_MIGRATIONS, LIBRARY_SPEC};
use crate::error::{Error, Result, io_error};
use crate::fingerprint::ReadCounters;
use crate::identity::{ReconcileReport, folder_path, mark_subtree_absent};
use crate::ingest::{IngestClient, IngestConfig, IngestOutcome};
use crate::journal::{self, RecoveryReport};
use crate::model::{
    AnnotationId, AnnotationInfo, DocumentId, DocumentInfo, FolderId, InstanceId, InstanceInfo,
    InstanceState, LibraryStatus, OperationId, OperationInfo, RevisionId, RevisionInfo,
    RevisionOrigin, RootId, RootInfo, SuggestionId, SuggestionInfo, SuggestionKind, WorkEdit,
    WorkId, WorkInfo, hex_of,
};
use crate::probe::{FsProbe, RealFileSystem};
use crate::reconcile::{self, Context, ReconcileScope};
use crate::search::{self, SearchHit};
use crate::sql::{StoredName, name_to_sql, now_ms};

/// Where a library's files are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibraryPaths {
    /// What cannot be replaced: `library.db` and its backups.
    pub data_dir: PathBuf,
    /// What can: `cache.db`.
    pub cache_dir: PathBuf,
}

impl LibraryPaths {
    /// The usual place for the current user: `%LOCALAPPDATA%\Musaeum`. Not the roaming profile,
    /// because SQLite's log must not be on anything that syncs or goes over a network.
    pub fn default_location() -> Result<Self> {
        let base = std::env::var_os("LOCALAPPDATA").ok_or_else(|| {
            Error::Refused(
                "LOCALAPPDATA is not set, so give a library folder explicitly".to_owned(),
            )
        })?;
        let base = PathBuf::from(base).join("Musaeum");
        Ok(Self {
            data_dir: base.join("library"),
            cache_dir: base.join("cache"),
        })
    }

    /// Everything inside one folder.
    pub fn in_directory(directory: &Path) -> Self {
        Self {
            data_dir: directory.to_owned(),
            cache_dir: directory.join("cache"),
        }
    }

    /// The database that cannot be replaced.
    pub fn library_db(&self) -> PathBuf {
        self.data_dir.join("library.db")
    }

    /// The database that can.
    pub fn cache_db(&self) -> PathBuf {
        self.cache_dir.join("cache.db")
    }

    /// Where copies of `library.db` go.
    pub fn backups_dir(&self) -> PathBuf {
        self.data_dir.join("backups")
    }

    fn lock_file(&self) -> PathBuf {
        self.data_dir.join("library.lock")
    }
}

/// How to open a library.
#[derive(Clone)]
pub struct LibraryOptions {
    /// How it looks at the disk.
    pub probe: Arc<dyn FsProbe>,
    /// How it starts the isolated worker.
    pub ingest: IngestConfig,
    /// How many backups it keeps.
    pub backup_policy: BackupPolicy,
    /// How many threads read fingerprints during a scan.
    pub fingerprint_threads: usize,
    /// How many threads calculate full hashes, each with a worker of its own.
    pub content_threads: usize,
    /// Whether reading files runs at background priority, which is what keeps it from getting in the
    /// way of anything else, at the cost of taking longer.
    pub low_priority: bool,
}

impl Default for LibraryOptions {
    fn default() -> Self {
        Self {
            probe: Arc::new(RealFileSystem),
            ingest: IngestConfig::default(),
            backup_policy: BackupPolicy::default(),
            fingerprint_threads: 4,
            content_threads: 2,
            low_priority: true,
        }
    }
}

/// A library.
pub struct Library {
    paths: LibraryPaths,
    database: Database,
    cache: Option<Database>,
    options: LibraryOptions,
    counters: ReadCounters,
    stop: AtomicBool,
    recovery: RecoveryReport,
    // Held for as long as this is open, so that only one process at a time can be the writer.
    _lock: Option<File>,
}

impl std::fmt::Debug for Library {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Library")
            .field("paths", &self.paths)
            .field("database", &self.database)
            .finish_non_exhaustive()
    }
}

impl Library {
    /// Opens a library for writing, creating it if there is none, upgrading it if it is old and
    /// finishing whatever a crash interrupted. Anything else writing to it makes this fail.
    pub fn open(paths: LibraryPaths, options: LibraryOptions) -> Result<Self> {
        fs::create_dir_all(&paths.data_dir).map_err(io_error("creating the library's folder"))?;
        fs::create_dir_all(&paths.cache_dir).map_err(io_error("creating the cache's folder"))?;
        let lock = acquire_lock(&paths.lock_file())?;

        let library_db = paths.library_db();
        back_up_before_upgrading(
            &library_db,
            &paths.backups_dir(),
            migrate::latest(LIBRARY_MIGRATIONS),
            &options.backup_policy,
        )?;

        let database = Database::open(&library_db, &LIBRARY_SPEC)?;
        let cache = open_cache(&paths.cache_db());
        let recovery = journal::recover(&database, options.probe.as_ref())?;
        Ok(Self {
            paths,
            database,
            cache,
            options,
            counters: ReadCounters::default(),
            stop: AtomicBool::new(false),
            recovery,
            _lock: Some(lock),
        })
    }

    /// Opens a library only to read it. This works while another process is writing to it, and
    /// nothing done through it can change anything.
    pub fn open_read_only(paths: LibraryPaths) -> Result<Self> {
        let library_db = paths.library_db();
        if !library_db.exists() {
            return Err(Error::NotFound(format!(
                "there is no library at {}",
                library_db.display()
            )));
        }
        let database = Database::open_read_only(&library_db, &LIBRARY_SPEC)?;
        Ok(Self {
            paths,
            database,
            cache: None,
            options: LibraryOptions::default(),
            counters: ReadCounters::default(),
            stop: AtomicBool::new(false),
            recovery: RecoveryReport::default(),
            _lock: None,
        })
    }

    /// Where this library's files are.
    pub fn paths(&self) -> &LibraryPaths {
        &self.paths
    }

    /// What the journal found and finished on opening.
    pub fn recovery_report(&self) -> RecoveryReport {
        self.recovery
    }

    /// Asks the scan and the hashing running right now to end as soon as they can.
    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// Lets scans and hashing run again.
    pub fn clear_stop(&self) {
        self.stop.store(false, Ordering::Relaxed);
    }

    pub(crate) fn probe(&self) -> &Arc<dyn FsProbe> {
        &self.options.probe
    }

    pub(crate) fn database(&self) -> &Database {
        &self.database
    }

    pub(crate) fn cache(&self) -> Option<&Database> {
        self.cache.as_ref()
    }

    fn context(&self) -> Context<'_> {
        Context {
            database: &self.database,
            probe: &self.options.probe,
            counters: &self.counters,
            threads: self.options.fingerprint_threads,
            low_priority: self.options.low_priority,
            stop: &self.stop,
        }
    }

    // --- Roots ---

    /// Starts following a folder. A folder inside another root, or containing one, is refused: the
    /// same file would be found twice, by two roots with different opinions about it. Adding one that
    /// is already followed changes what it leaves out, if this says anything about that.
    pub fn add_root(&self, path: &Path, exclusions: &[String]) -> Result<RootInfo> {
        let path = absolute_existing(path)?;
        if !self
            .options
            .probe
            .stat(&path)
            .is_ok_and(|stat| stat.is_directory())
        {
            return Err(Error::Refused(format!(
                "{} is not a folder",
                path.display()
            )));
        }
        let lowered = lowercase_path(&path);
        for root in self.roots()? {
            let other = lowercase_path(&root.path);
            if lowered == other {
                if exclusions.is_empty() {
                    return Ok(root);
                }
                let id = root.id;
                let exclusions =
                    serde_json::to_string(exclusions).unwrap_or_else(|_| "[]".to_owned());
                self.database.write(move |transaction| {
                    transaction
                        .prepare_cached("UPDATE library_root SET exclusions = ?2 WHERE id = ?1")?
                        .execute(params![id, exclusions])?;
                    Ok(())
                })?;
                return self
                    .database
                    .read(|connection| reconcile::load_root(connection, id));
            }
            if lowered.starts_with(&other) || other.starts_with(&lowered) {
                return Err(Error::Refused(format!(
                    "{} overlaps the root at {}",
                    path.display(),
                    root.path.display()
                )));
            }
        }
        let stored = name_to_sql(path.as_os_str());
        let exclusions = serde_json::to_string(exclusions).unwrap_or_else(|_| "[]".to_owned());
        let id = self.database.write(move |transaction| {
            transaction
                .prepare_cached(
                    "INSERT INTO library_root (path, exclusions, added_at) VALUES (?1, ?2, ?3)
                     ON CONFLICT (path) DO UPDATE SET status = 'available', exclusions = ?2
                     RETURNING id",
                )?
                .query_row(params![stored, exclusions, now_ms()], |row| {
                    row.get::<_, RootId>(0)
                })
                .map_err(Error::from)
        })?;
        self.database
            .read(|connection| reconcile::load_root(connection, id))
    }

    /// Stops following a folder. What was in it goes absent, and everything else stays, so adding
    /// it again finds every file just as it was.
    pub fn remove_root(&self, id: RootId) -> Result<()> {
        self.database.write(move |transaction| {
            let top: Option<FolderId> = transaction
                .prepare_cached("SELECT id FROM folder WHERE root_id = ?1 AND parent_id IS NULL")?
                .query_row([id], |row| row.get(0))
                .optional()?;
            if let Some(top) = top {
                let mut report = ReconcileReport::default();
                mark_subtree_absent(transaction, top, now_ms(), &mut report)?;
            }
            let changed = transaction
                .prepare_cached("UPDATE library_root SET status = 'removed' WHERE id = ?1")?
                .execute([id])?;
            if changed == 0 {
                return Err(Error::NotFound(format!("there is no root {id}")));
            }
            Ok(())
        })
    }

    /// Tells the library that a root's folder is somewhere else now. Changes one row, and the next
    /// scan recognizes everything in it by File ID, without reading anything.
    pub fn relocate_root(&self, id: RootId, path: &Path) -> Result<RootInfo> {
        let path = absolute_existing(path)?;
        let stored = name_to_sql(path.as_os_str());
        self.database.write(move |transaction| {
            let changed = transaction
                .prepare_cached(
                    "UPDATE library_root SET path = ?2, status = 'available' WHERE id = ?1",
                )?
                .execute(params![id, stored])?;
            if changed == 0 {
                return Err(Error::NotFound(format!("there is no root {id}")));
            }
            Ok(())
        })?;
        self.database
            .read(|connection| reconcile::load_root(connection, id))
    }

    /// The folders being followed.
    pub fn roots(&self) -> Result<Vec<RootInfo>> {
        self.database.read(reconcile::list_roots)
    }

    // --- Scans ---

    /// Compares a root with what the library knows.
    pub fn reconcile(&self, root: RootId, scope: &ReconcileScope) -> Result<ReconcileReport> {
        let info = self
            .database
            .read(|connection| reconcile::load_root(connection, root))?;
        let report = reconcile::reconcile_root(&self.context(), &info, scope)?;
        self.remember_run(&report);
        Ok(report)
    }

    /// Compares every root with what the library knows.
    pub fn reconcile_all(&self) -> Result<Vec<ReconcileReport>> {
        let mut reports = Vec::new();
        for root in self.roots()? {
            reports.push(self.reconcile(root.id, &ReconcileScope::Everything)?);
        }
        Ok(reports)
    }

    /// For inspecting later. If it fails, the history just has one entry less.
    fn remember_run(&self, report: &ReconcileReport) {
        let Some(cache) = &self.cache else {
            return;
        };
        let root = report.root_id;
        let duration = report.duration_ms as i64;
        let Ok(json) = serde_json::to_string(report) else {
            return;
        };
        let _ = cache.write(move |transaction| {
            transaction
                .prepare_cached(
                    "INSERT INTO reconcile_run (root_id, finished_at, duration_ms, report)
                     VALUES (?1, ?2, ?3, ?4)",
                )?
                .execute(params![root, now_ms(), duration, json])?;
            // A history, and not a log that grows forever.
            transaction
                .prepare_cached(
                    "DELETE FROM reconcile_run WHERE id NOT IN
                     (SELECT id FROM reconcile_run ORDER BY id DESC LIMIT 200)",
                )?
                .execute([])?;
            Ok(())
        });
    }

    /// Goes through what is waiting for a full hash or for its metadata, until nothing is. This is
    /// what confirms or refutes what scans could only guess.
    pub fn process_content(&self) -> Result<ContentReport> {
        content::process_pending(
            &self.database,
            self.cache.as_ref(),
            self.options.probe.as_ref(),
            &self.options.ingest,
            &self.counters,
            self.options.content_threads,
            self.options.low_priority,
            &self.stop,
        )
    }

    pub(crate) fn has_pending_content(&self) -> Result<bool> {
        self.database.read(content::has_pending)
    }

    // --- What is in it ---

    /// How many of each thing there are.
    pub fn status(&self) -> Result<LibraryStatus> {
        self.database.read(|connection| {
            let count = |sql: &str| -> Result<u64> {
                Ok(connection
                    .prepare_cached(sql)?
                    .query_row([], |row| row.get::<_, i64>(0))? as u64)
            };
            let in_state = |state: InstanceState| -> Result<u64> {
                Ok(connection
                    .prepare_cached("SELECT count(*) FROM file_instance WHERE state = ?1")?
                    .query_row([state], |row| row.get::<_, i64>(0))? as u64)
            };
            Ok(LibraryStatus {
                works: count("SELECT count(*) FROM work")?,
                documents: count("SELECT count(*) FROM document")?,
                revisions: count("SELECT count(*) FROM revision")?,
                unconfirmed_revisions: count("SELECT count(*) FROM revision WHERE blake3 IS NULL")?,
                present: in_state(InstanceState::Present)?,
                absent: in_state(InstanceState::Absent)?,
                cloud_only: in_state(InstanceState::CloudOnly)?,
                unreadable: in_state(InstanceState::Unreadable)?,
                folders: count("SELECT count(*) FROM folder WHERE state = 'present'")?,
                annotations: count("SELECT count(*) FROM annotation WHERE deleted_at IS NULL")?,
                open_suggestions: count(
                    "SELECT count(*) FROM reconcile_suggestion WHERE state = 'open'",
                )?,
                unfinished_operations: count(
                    "SELECT count(*) FROM fs_operation WHERE state = 'intent'",
                )?,
                roots: reconcile::list_roots(connection)?,
            })
        })
    }

    /// The file instance recorded at a path, including one no longer there.
    pub fn instance_at(&self, path: &Path) -> Result<Option<InstanceInfo>> {
        let path = absolute_if_it_exists(path);
        self.database.read(|connection| {
            let Some(id) = locate_instance(connection, &path)? else {
                return Ok(None);
            };
            load_instance(connection, id).map(Some)
        })
    }

    /// The folder at a path, if the library has one there.
    pub fn folder_at(&self, path: &Path) -> Result<Option<FolderId>> {
        let path = absolute_if_it_exists(path);
        self.database
            .read(|connection| Ok(journal::folder_at(connection, &path)?.map(|(folder, _)| folder)))
    }

    /// A file instance by its ID.
    pub fn instance(&self, id: InstanceId) -> Result<InstanceInfo> {
        self.database
            .read(|connection| load_instance(connection, id))
    }

    /// The first file instances in the library, in order, optionally only those in one state.
    pub fn instances(
        &self,
        state: Option<InstanceState>,
        limit: usize,
    ) -> Result<Vec<InstanceInfo>> {
        self.database.read(|connection| {
            let ids = {
                let mut statement = connection.prepare_cached(
                    "SELECT id FROM file_instance WHERE ?1 IS NULL OR state = ?1 ORDER BY id LIMIT ?2",
                )?;
                statement
                    .query_map(params![state, limit as i64], |row| row.get::<_, InstanceId>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            };
            ids.into_iter()
                .map(|id| load_instance(connection, id))
                .collect()
        })
    }

    /// A document, with its revisions and every place each one is or was.
    pub fn document(&self, id: DocumentId) -> Result<DocumentInfo> {
        self.database
            .read(|connection| load_document(connection, id))
    }

    /// A work.
    pub fn work(&self, id: WorkId) -> Result<WorkInfo> {
        self.database.read(|connection| load_work(connection, id))
    }

    /// Finds works as someone types.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        self.database
            .read(|connection| search::search(connection, query, limit))
    }

    /// Writes the search indexes again from the works themselves.
    pub fn rebuild_search(&self) -> Result<()> {
        self.database.write(search::rebuild)
    }

    // --- What a person decides ---

    /// What the library noticed and did not act on.
    pub fn suggestions(&self, only_open: bool) -> Result<Vec<SuggestionInfo>> {
        self.database.read(|connection| {
            let mut statement = connection.prepare_cached(
                "SELECT id, kind, document_id, candidate_document_id, evidence, state
                 FROM reconcile_suggestion WHERE (?1 = 0 OR state = 'open') ORDER BY id",
            )?;
            let rows = statement.query_map([only_open], |row| {
                Ok(SuggestionInfo {
                    id: row.get(0)?,
                    kind: row.get::<_, SuggestionKind>(1)?,
                    document_id: row.get(2)?,
                    candidate_document_id: row.get(3)?,
                    evidence: row.get(4)?,
                    state: row.get(5)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// They are not the same, and it will not be suggested again.
    pub fn dismiss_suggestion(&self, id: SuggestionId) -> Result<()> {
        self.database.write(move |transaction| {
            let changed = transaction
                .prepare_cached(
                    "UPDATE reconcile_suggestion SET state = 'dismissed' WHERE id = ?1",
                )?
                .execute([id])?;
            if changed == 0 {
                return Err(Error::NotFound(format!("there is no suggestion {id}")));
            }
            Ok(())
        })
    }

    /// They are the same: the newer document's revisions become revisions of the older one, and
    /// what a person put on either stays. This is the only way two documents ever become one.
    pub fn accept_suggestion(&self, id: SuggestionId) -> Result<DocumentId> {
        self.database.write(move |transaction| {
            let row: Option<(DocumentId, DocumentId)> = transaction
                .prepare_cached(
                    "SELECT document_id, candidate_document_id FROM reconcile_suggestion
                     WHERE id = ?1 AND state = 'open'",
                )?
                .query_row([id], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()?;
            let Some((from, into)) = row else {
                return Err(Error::NotFound(format!("there is no open suggestion {id}")));
            };
            let newest: Option<RevisionId> = transaction
                .prepare_cached("SELECT max(id) FROM revision WHERE document_id = ?1")?
                .query_row([into], |row| row.get(0))?;
            transaction
                .prepare_cached(
                    "UPDATE revision SET previous_revision_id = ?2
                     WHERE document_id = ?1 AND previous_revision_id IS NULL",
                )?
                .execute(params![from, newest])?;
            transaction
                .prepare_cached("UPDATE revision SET document_id = ?2 WHERE document_id = ?1")?
                .execute(params![from, into])?;
            merge_empty_document(transaction, from, into, now_ms())?;
            Ok(into)
        })
    }

    /// Changes a work. Everything set here is the user's from now on: nothing read from a file will
    /// ever write over it.
    pub fn edit_work(&self, id: WorkId, edit: WorkEdit) -> Result<()> {
        if let Some(rating) = edit.rating
            && !(1..=5).contains(&rating)
        {
            return Err(Error::Refused("a rating goes from 1 to 5".to_owned()));
        }
        if let Some(status) = &edit.status
            && !["unread", "reading", "read", "abandoned"].contains(&status.as_str())
        {
            return Err(Error::Refused(format!("{status} is not a status")));
        }
        self.database.write(move |transaction| {
            let now = now_ms();
            let exists: Option<i64> = transaction
                .prepare_cached("SELECT 1 FROM work WHERE id = ?1")?
                .query_row([id], |row| row.get(0))
                .optional()?;
            if exists.is_none() {
                return Err(Error::NotFound(format!("there is no work {id}")));
            }
            if let Some(creators) = &edit.creators {
                set_creators(transaction, id, creators)?;
                set_provenance(transaction, id, "creators", "user", now)?;
            }
            transaction
                .prepare_cached(
                    "UPDATE work SET
                         title = coalesce(?2, title), rating = coalesce(?3, rating),
                         status = coalesce(?4, status), series = coalesce(?5, series),
                         series_position = coalesce(?6, series_position), updated_at = ?7
                     WHERE id = ?1",
                )?
                .execute(params![
                    id,
                    edit.title,
                    edit.rating,
                    edit.status,
                    edit.series,
                    edit.series_position,
                    now,
                ])?;
            let fields = [
                ("title", edit.title.is_some()),
                ("rating", edit.rating.is_some()),
                ("status", edit.status.is_some()),
                ("series", edit.series.is_some()),
                ("series_position", edit.series_position.is_some()),
            ];
            for (field, changed) in fields {
                if changed {
                    set_provenance(transaction, id, field, "user", now)?;
                }
            }
            if edit.creators.is_some() {
                search::refresh_entry(transaction, id)?;
            }
            Ok(())
        })
    }

    /// Adds a note to a document, on the revision it has now.
    pub fn add_annotation(&self, document: DocumentId, note: &str) -> Result<AnnotationId> {
        let note = note.to_owned();
        self.database.write(move |transaction| {
            let revision: Option<RevisionId> = transaction
                .prepare_cached(
                    "SELECT r.id FROM revision r
                     WHERE r.document_id = ?1
                     ORDER BY EXISTS (SELECT 1 FROM file_instance i
                                      WHERE i.revision_id = r.id AND i.state <> 'absent') DESC,
                              r.id DESC
                     LIMIT 1",
                )?
                .query_row([document], |row| row.get(0))
                .optional()?;
            let Some(revision) = revision else {
                return Err(Error::NotFound(format!("there is no document {document}")));
            };
            let now = now_ms();
            transaction
                .prepare_cached(
                    "INSERT INTO annotation (document_id, revision_id, note, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?4)",
                )?
                .execute(params![document, revision, note, now])?;
            Ok(transaction.last_insert_rowid())
        })
    }

    /// Reads a document's metadata again, from the cache if it is there and from the file through
    /// the isolated worker if not. Whatever a person edited stays as they left it.
    pub fn reimport(&self, document: DocumentId) -> Result<ContentReport> {
        let (revision, hash, format, path) = self.database.read(|connection| {
            type Stored = (RevisionId, Option<Vec<u8>>, String, FolderId, StoredName);
            let row: Option<Stored> = connection
                .prepare_cached(
                    "SELECT r.id, r.blake3, d.format, i.folder_id, i.name
                     FROM revision r
                     JOIN document d ON d.id = r.document_id
                     JOIN file_instance i ON i.revision_id = r.id AND i.state = 'present'
                     WHERE r.document_id = ?1 ORDER BY r.id DESC LIMIT 1",
                )?
                .query_row([document], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                })
                .optional()?;
            let Some((revision, hash, format, folder, name)) = row else {
                return Err(Error::Refused(format!(
                    "document {document} has no file available to read"
                )));
            };
            Ok((
                revision,
                hash,
                format,
                folder_path(connection, folder)?.join(name.0),
            ))
        })?;

        let extracted = match self.cached_extraction(hash.as_deref())? {
            Some(extracted) => extracted,
            None => self.extract_from_file(&path, &format)?,
        };
        let Some(extracted) = extracted else {
            return Ok(ContentReport::default());
        };
        self.database.write(move |transaction| {
            let mut report = ContentReport::default();
            apply_extraction(transaction, revision, &extracted, now_ms(), &mut report)?;
            report.extracted = 1;
            Ok(report)
        })
    }

    fn cached_extraction(&self, hash: Option<&[u8]>) -> Result<Option<Option<Extracted>>> {
        let (Some(cache), Some(hash)) = (&self.cache, hash) else {
            return Ok(None);
        };
        cache.read(|connection| {
            let row: Option<Option<String>> = connection
                .prepare_cached(
                    "SELECT data FROM extraction WHERE blake3 = ?1 AND extractor_version = ?2",
                )?
                .query_row(params![hash, EXTRACTOR_VERSION], |row| row.get(0))
                .optional()?;
            Ok(row.map(|data| data.and_then(|text| serde_json::from_str(&text).ok())))
        })
    }

    fn extract_from_file(&self, path: &Path, format: &str) -> Result<Option<Extracted>> {
        let Some(format) = Format::from_extension(format) else {
            return Ok(None);
        };
        let content = self
            .options
            .probe
            .open_content(path, ReadPattern::Samples)
            .map_err(|error: OpenError| Error::Refused(format!("{}: {error}", path.display())))?;
        let mut client = IngestClient::new(self.options.ingest.clone());
        match client.extract(&content.file, format) {
            IngestOutcome::Extracted(extracted) => Ok(Some(extracted)),
            IngestOutcome::Nothing(_) => Ok(None),
            IngestOutcome::Unavailable(reason) => Err(Error::Refused(reason)),
        }
    }

    // --- The disk ---

    /// Moves or renames a file. Refuses to overwrite anything or to leave the monitored roots.
    pub fn move_file(&self, instance: InstanceId, target: &Path) -> Result<OperationId> {
        journal::move_file(
            &self.database,
            self.options.probe.as_ref(),
            instance,
            &absolute_target(target)?,
        )
    }

    /// Moves or renames a folder.
    pub fn move_folder(&self, folder: FolderId, target: &Path) -> Result<OperationId> {
        journal::move_folder(
            &self.database,
            self.options.probe.as_ref(),
            folder,
            &absolute_target(target)?,
        )
    }

    /// Sends a file to the system's Recycle Bin. The library keeps all it knows about it.
    pub fn recycle_file(&self, instance: InstanceId) -> Result<OperationId> {
        journal::recycle(&self.database, self.options.probe.as_ref(), instance)
    }

    /// Takes back what an operation did.
    pub fn undo(&self, operation: OperationId) -> Result<OperationId> {
        journal::undo(&self.database, self.options.probe.as_ref(), operation)
    }

    /// The journal, newest first.
    pub fn operations(&self, limit: usize) -> Result<Vec<OperationInfo>> {
        self.database
            .read(|connection| journal::list(connection, limit))
    }

    // --- Backups ---

    /// Makes a copy of `library.db` now.
    pub fn backup_now(&self) -> Result<BackupInfo> {
        let info = backup::create(&self.paths.library_db(), &self.paths.backups_dir(), None)?;
        backup::rotate(&self.paths.backups_dir(), &self.options.backup_policy)?;
        Ok(info)
    }

    /// Makes a copy of `library.db` if the newest one is old enough, and returns it if it did.
    pub fn backup_if_due(&self) -> Result<Option<BackupInfo>> {
        if backup::is_due(
            &self.paths.backups_dir(),
            self.options.backup_policy.interval,
        )? {
            self.backup_now().map(Some)
        } else {
            Ok(None)
        }
    }

    /// The copies there are, newest first.
    pub fn backups(&self) -> Result<Vec<BackupInfo>> {
        backup::list(&self.paths.backups_dir())
    }

    /// Replaces a library with a backup, setting the current one aside instead of deleting it.
    /// Returns where. The library must not be open anywhere.
    pub fn restore_backup(paths: &LibraryPaths, backup_file: &Path) -> Result<PathBuf> {
        let _lock = acquire_lock(&paths.lock_file())?;
        backup::restore(&paths.library_db(), &paths.backups_dir(), backup_file)
    }
}

/// The way back from an upgrade that goes wrong, or that turns out to be a mistake. A copy is only
/// made if there is something to copy and it is about to change.
fn back_up_before_upgrading(
    library_db: &Path,
    backups: &Path,
    latest: i64,
    policy: &BackupPolicy,
) -> Result<Option<BackupInfo>> {
    let Some(version) = migrate::peek_version(library_db, LIBRARY_APPLICATION_ID)? else {
        return Ok(None);
    };
    if !(1..latest).contains(&version) {
        return Ok(None);
    }
    let info = backup::create(library_db, backups, Some(version))?;
    backup::rotate(backups, policy)?;
    Ok(Some(info))
}

fn acquire_lock(path: &Path) -> Result<File> {
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .map_err(io_error("opening the library's lock"))?;
    file.try_lock().map_err(|error| match error {
        fs::TryLockError::WouldBlock => Error::LockedByAnotherProcess,
        fs::TryLockError::Error(source) => Error::Io {
            context: "locking the library".to_owned(),
            source,
        },
    })?;
    Ok(file)
}

/// `cache.db` holds nothing that cannot be read again, so whatever stops it from opening, the answer
/// is a new one. If not even that works, the library runs without one, and only loses speed.
fn open_cache(path: &Path) -> Option<Database> {
    if let Ok(cache) = Database::open(path, &CACHE_SPEC) {
        return Some(cache);
    }
    for suffix in ["", "-wal", "-shm"] {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        let _ = fs::remove_file(PathBuf::from(name));
    }
    Database::open(path, &CACHE_SPEC).ok()
}

/// The real, absolute path of something that must exist, without the `\\?\` that
/// `canonicalize` adds, which nobody wants to see and which would make it unequal to the paths
/// people type.
fn absolute_existing(path: &Path) -> Result<PathBuf> {
    match fs::canonicalize(path) {
        Ok(canonical) => Ok(without_verbatim_prefix(canonical)),
        // A volume with no drive letter has no such form, and is a perfectly good place for a root.
        Err(_) if path.exists() => {
            std::path::absolute(path).map_err(io_error(format!("finding {}", path.display())))
        }
        Err(error) => Err(io_error(format!("finding {}", path.display()))(error)),
    }
}

/// Where something is about to go. It does not exist yet, but the folder it goes in should, and that
/// folder is the one to get right: paths are compared exactly, and people type them in any case.
fn absolute_target(path: &Path) -> Result<PathBuf> {
    let absolute = std::path::absolute(path).map_err(io_error("resolving the target"))?;
    match (absolute.parent(), absolute.file_name()) {
        (Some(parent), Some(name)) if parent.exists() => Ok(absolute_existing(parent)?.join(name)),
        _ => Ok(absolute),
    }
}

fn absolute_if_it_exists(path: &Path) -> PathBuf {
    fs::canonicalize(path)
        .map(without_verbatim_prefix)
        .or_else(|_| std::path::absolute(path))
        .unwrap_or_else(|_| path.to_owned())
}

fn without_verbatim_prefix(path: PathBuf) -> PathBuf {
    // In UTF-16 and not as text, because a path need not be valid Unicode.
    let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    let verbatim: Vec<u16> = r"\\?\".encode_utf16().collect();
    let verbatim_unc: Vec<u16> = r"\\?\UNC\".encode_utf16().collect();
    if wide.starts_with(&verbatim_unc) {
        let mut plain: Vec<u16> = r"\\".encode_utf16().collect();
        plain.extend_from_slice(&wide[verbatim_unc.len()..]);
        return PathBuf::from(OsString::from_wide(&plain));
    }
    // A long or unusual path needs the prefix to mean the same thing, and the file system code adds
    // it back wherever it matters, so it is only dropped from what looks like a plain drive path.
    if wide.starts_with(&verbatim) && wide.get(verbatim.len() + 1) == Some(&u16::from(b':')) {
        return PathBuf::from(OsString::from_wide(&wide[verbatim.len()..]));
    }
    path
}

fn lowercase_path(path: &Path) -> PathBuf {
    PathBuf::from(path.to_string_lossy().to_lowercase())
}

fn locate_instance(connection: &Connection, path: &Path) -> Result<Option<InstanceId>> {
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return Ok(None);
    };
    for root in reconcile::list_roots(connection)? {
        let Ok(relative) = parent.strip_prefix(&root.path) else {
            continue;
        };
        let top: Option<FolderId> = connection
            .prepare_cached("SELECT id FROM folder WHERE root_id = ?1 AND parent_id IS NULL")?
            .query_row([root.id], |row| row.get(0))
            .optional()?;
        let Some(mut current) = top else {
            continue;
        };
        let mut complete = true;
        for component in relative {
            let child: Option<FolderId> = connection
                .prepare_cached(
                    "SELECT id FROM folder WHERE parent_id = ?1 AND name = ?2
                     ORDER BY state = 'present' DESC, id DESC LIMIT 1",
                )?
                .query_row(params![current, name_to_sql(component)], |row| row.get(0))
                .optional()?;
            match child {
                Some(child) => current = child,
                None => {
                    complete = false;
                    break;
                }
            }
        }
        if !complete {
            continue;
        }
        let instance: Option<InstanceId> = connection
            .prepare_cached(
                "SELECT id FROM file_instance WHERE folder_id = ?1 AND name = ?2
                 ORDER BY state <> 'absent' DESC, id DESC LIMIT 1",
            )?
            .query_row(params![current, name_to_sql(name)], |row| row.get(0))
            .optional()?;
        if instance.is_some() {
            return Ok(instance);
        }
    }
    Ok(None)
}

fn load_instance(connection: &Connection, id: InstanceId) -> Result<InstanceInfo> {
    let row = connection
        .prepare_cached(
            "SELECT i.revision_id, r.document_id, d.work_id, i.folder_id, i.name, i.state, i.file_id,
                    i.mtime_ns
             FROM file_instance i
             JOIN revision r ON r.id = i.revision_id
             JOIN document d ON d.id = r.document_id
             WHERE i.id = ?1",
        )?
        .query_row([id], |row| {
            Ok((
                row.get::<_, RevisionId>(0)?,
                row.get::<_, DocumentId>(1)?,
                row.get::<_, WorkId>(2)?,
                row.get::<_, FolderId>(3)?,
                row.get::<_, StoredName>(4)?.0,
                row.get::<_, InstanceState>(5)?,
                row.get::<_, Option<Vec<u8>>>(6)?,
                row.get::<_, i64>(7)?,
            ))
        })
        .optional()?
        .ok_or_else(|| Error::NotFound(format!("there is no file {id}")))?;
    let (revision_id, document_id, work_id, folder_id, name, state, file_id, mtime_ns) = row;
    Ok(InstanceInfo {
        id,
        revision_id,
        document_id,
        work_id,
        folder_id,
        path: folder_path(connection, folder_id)?.join(name),
        state,
        file_id: hex_of(file_id),
        mtime_ns,
    })
}

fn load_document(connection: &Connection, id: DocumentId) -> Result<DocumentInfo> {
    let row = connection
        .prepare_cached(
            "SELECT d.work_id, w.title, d.format, d.layout, d.pdf_id0, d.dc_identifier, d.isbn
             FROM document d JOIN work w ON w.id = d.work_id WHERE d.id = ?1",
        )?
        .query_row([id], |row| {
            Ok((
                row.get::<_, WorkId>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        })
        .optional()?
        .ok_or_else(|| Error::NotFound(format!("there is no document {id}")))?;
    let (work_id, title, format, layout, pdf_id0, dc_identifier, isbn) = row;

    let revisions = {
        let mut statement = connection.prepare_cached(
            "SELECT id, blake3, fast_fingerprint, size, previous_revision_id, origin
             FROM revision WHERE document_id = ?1 ORDER BY id",
        )?;
        statement
            .query_map([id], |row| {
                Ok((
                    row.get::<_, RevisionId>(0)?,
                    row.get::<_, Option<Vec<u8>>>(1)?,
                    row.get::<_, Option<Vec<u8>>>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<RevisionId>>(4)?,
                    row.get::<_, RevisionOrigin>(5)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut revision_infos = Vec::with_capacity(revisions.len());
    for (revision, blake3, fingerprint, size, previous, origin) in revisions {
        let instance_ids = {
            let mut statement = connection.prepare_cached(
                "SELECT id FROM file_instance WHERE revision_id = ?1 ORDER BY id",
            )?;
            statement
                .query_map([revision], |row| row.get::<_, InstanceId>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        revision_infos.push(RevisionInfo {
            id: revision,
            document_id: id,
            blake3: hex_of(blake3),
            fast_fingerprint: hex_of(fingerprint),
            size: size as u64,
            previous_revision_id: previous,
            origin,
            instances: instance_ids
                .into_iter()
                .map(|instance| load_instance(connection, instance))
                .collect::<Result<_>>()?,
        });
    }

    let annotations = {
        let mut statement = connection.prepare_cached(
            "SELECT id, revision_id, note FROM annotation
             WHERE document_id = ?1 AND deleted_at IS NULL ORDER BY id",
        )?;
        statement
            .query_map([id], |row| {
                Ok(AnnotationInfo {
                    id: row.get(0)?,
                    document_id: id,
                    revision_id: row.get(1)?,
                    note: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };

    Ok(DocumentInfo {
        id,
        work_id,
        title,
        format,
        layout,
        pdf_id0,
        dc_identifier,
        isbn,
        revisions: revision_infos,
        annotations,
    })
}

fn load_work(connection: &Connection, id: WorkId) -> Result<WorkInfo> {
    let mut work = connection
        .prepare_cached(
            "SELECT title, language, year, series, series_position, rating, status
             FROM work WHERE id = ?1",
        )?
        .query_row([id], |row| {
            Ok(WorkInfo {
                id,
                title: row.get(0)?,
                language: row.get(1)?,
                year: row.get(2)?,
                series: row.get(3)?,
                series_position: row.get(4)?,
                rating: row.get(5)?,
                status: row.get(6)?,
                creators: Vec::new(),
                document_ids: Vec::new(),
            })
        })
        .optional()?
        .ok_or_else(|| Error::NotFound(format!("there is no work {id}")))?;
    work.creators = {
        let mut statement = connection.prepare_cached(
            "SELECT c.name FROM work_creator wc JOIN creator c ON c.id = wc.creator_id
             WHERE wc.work_id = ?1 ORDER BY wc.position",
        )?;
        statement
            .query_map([id], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    work.document_ids = {
        let mut statement =
            connection.prepare_cached("SELECT id FROM document WHERE work_id = ?1 ORDER BY id")?;
        statement
            .query_map([id], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    Ok(work)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("musaeum-library-{}-{name}", std::process::id()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A library at the current schema, with something in it.
    fn a_library(directory: &Path) -> PathBuf {
        let paths = LibraryPaths::in_directory(directory);
        let library = Library::open(paths.clone(), LibraryOptions::default()).unwrap();
        library.add_root(directory, &[]).unwrap();
        paths.library_db()
    }

    #[test]
    fn a_library_about_to_be_upgraded_is_copied_first() {
        let directory = TempDir::new("upgrade");
        let library_db = a_library(&directory.0);
        let current = migrate::latest(LIBRARY_MIGRATIONS);
        let backups = directory.0.join("backups");
        let policy = BackupPolicy::default();

        let copy = back_up_before_upgrading(&library_db, &backups, current + 1, &policy)
            .unwrap()
            .expect("a newer schema means a copy");
        assert_eq!(copy.before_upgrade_from, Some(current));

        let connection = Connection::open(&copy.path).unwrap();
        let roots: i64 = connection
            .query_row("SELECT count(*) FROM library_root", [], |row| row.get(0))
            .unwrap();
        assert_eq!(roots, 1);
    }

    #[test]
    fn nothing_is_copied_when_there_is_nothing_to_upgrade() {
        let directory = TempDir::new("no-upgrade");
        let library_db = a_library(&directory.0);
        let current = migrate::latest(LIBRARY_MIGRATIONS);
        let backups = directory.0.join("backups");
        let policy = BackupPolicy::default();

        assert_eq!(
            back_up_before_upgrading(&library_db, &backups, current, &policy).unwrap(),
            None
        );
        assert_eq!(
            back_up_before_upgrading(&directory.0.join("none.db"), &backups, current + 1, &policy)
                .unwrap(),
            None,
            "a library that does not exist yet"
        );
        assert!(!backups.exists());
    }

    #[test]
    fn verbatim_prefixes_go_only_where_they_mean_nothing_and_nothing_else_changes() {
        let plain = without_verbatim_prefix(PathBuf::from(r"\\?\C:\Books\a.pdf"));
        assert_eq!(plain, PathBuf::from(r"C:\Books\a.pdf"));
        let unc = without_verbatim_prefix(PathBuf::from(r"\\?\UNC\server\share\a.pdf"));
        assert_eq!(unc, PathBuf::from(r"\\server\share\a.pdf"));
        let volume = PathBuf::from(r"\\?\Volume{01234567-89ab}\a.pdf");
        assert_eq!(without_verbatim_prefix(volume.clone()), volume);

        let not_unicode: Vec<u16> = r"\\?\C:\".encode_utf16().chain([0xD800]).collect();
        let stripped = without_verbatim_prefix(PathBuf::from(OsString::from_wide(&not_unicode)));
        let expected: Vec<u16> = r"C:\".encode_utf16().chain([0xD800]).collect();
        assert_eq!(
            stripped.as_os_str().encode_wide().collect::<Vec<_>>(),
            expected
        );
    }
}

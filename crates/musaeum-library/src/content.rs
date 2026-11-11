//! The slow half of recognizing a file: its full hash and what it says about itself.
//!
//! This is where steps 3, 4 and 5 are finally decided. The library has been working with an
//! unconfirmed revision meanwhile, and now it finds out what those bytes really are:
//!
//! - bytes it already knows mean that the file is that revision, however it got here. If the
//!   revision had gone missing, then it has come back, and if not, this is a duplicate;
//! - new bytes with a known internal ID become a suggestion;
//! - anything else stays what it has been all along, a new document.
//!
//! It is also where a file that took another's place at its path stops being provisional: it stays a
//! new revision of the same document only if it is the same kind of file and names no other internal
//! ID, as the [`crate::identity`] module explains.
//!
//! Each piece of work is on a low-priority thread, and works out what to write before asking for the
//! write lock, so hashing a large file does not hold anything up.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;

use musaeum_fs::{FileId, OpenError, ReadPattern, enter_background};
use musaeum_ingest::EXTRACTOR_VERSION;
use musaeum_ingest::parse::Extracted;
use musaeum_ingest::parse::pdf_catalog::Direction;
use musaeum_ingest::protocol::Format;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Serialize;

use crate::content_kind::ContentKind;
use crate::error::Result;
use crate::fingerprint::{Hash, ReadCounters, WholeFile, fast_fingerprint, full_hash};
use crate::identity::{folder_path, insert_work_and_document, suggest};
use crate::ingest::{IngestClient, IngestConfig, IngestOutcome};
use crate::model::{
    DocumentId, FolderId, InstanceId, InstanceState, RevisionId, RevisionOrigin, SuggestionKind,
    WorkId,
};
use crate::probe::FsProbe;
use crate::sql::{StoredName, file_id_from_sql, volume_from_sql};

const PAGE: usize = 256;
/// More than this and it is a template's identifier, not a book's.
const MAX_SUGGESTIONS_PER_IDENTIFIER: i64 = 8;

/// What a pass over the waiting revisions did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ContentReport {
    /// Revisions whose full hash was calculated.
    pub hashed: u64,
    /// Bytes read for those hashes.
    pub bytes_hashed: u64,
    /// Files that turned out to be a revision that had gone missing.
    pub reattached: u64,
    /// Files that turned out to be a revision that is also elsewhere.
    pub duplicates: u64,
    /// Files that turned out to be the same bytes they were before, such as after a touch.
    pub same_as_previous: u64,
    /// Files that turned out to be new bytes.
    pub new_revisions: u64,
    /// Files that took another's place at its path and turned out to be something else: another kind
    /// of file, or one with a different internal ID. Each is a document of its own, with a suggestion.
    pub replacements_separated: u64,
    /// Things noticed and left for the user to decide.
    pub suggestions: u64,
    /// Files whose metadata was read by the isolated worker.
    pub extracted: u64,
    /// Files the worker refused or died on.
    pub extraction_failed: u64,
    /// Workers that had to be replaced.
    pub worker_restarts: u64,
    /// Files found to be only in the cloud, whose bytes must wait.
    pub cloud_only: u64,
    /// Files that could not be opened.
    pub unreadable: u64,
    /// Files that changed while being read, for the next scan to look at again.
    pub changed_meanwhile: u64,
    /// How long it took.
    pub duration_ms: u64,
}

/// A revision waiting for something, and a place to read it from.
#[derive(Debug)]
struct Job {
    revision: RevisionId,
    instance: InstanceId,
    path: PathBuf,
    volume: Option<u64>,
    file_id: Option<FileId>,
    size: u64,
    mtime_ns: i64,
    format: String,
    needs_hash: bool,
    needs_fingerprint: bool,
    needs_extraction: bool,
    known_hash: Option<Hash>,
}

#[derive(Debug)]
enum Finding {
    /// The bytes were read.
    Read {
        hash: Option<Hash>,
        /// What kind of file the bytes are, known whenever the hash is.
        kind: Option<ContentKind>,
        fingerprint: Option<Hash>,
        bytes: u64,
        extraction: Extraction,
    },
    CloudOnly,
    Missing,
    Unreadable,
    ChangedMeanwhile,
}

#[derive(Debug)]
enum Extraction {
    /// There was nothing to ask, because of the format or because it was asked before.
    NotAsked,
    /// What the worker said.
    Answer(Box<Extracted>),
    /// The worker refused the file or died on it, and asking again would end the same way.
    Nothing,
    /// No worker could be started, so ask again some other time.
    Unavailable,
}

/// One pass over everything waiting, with `threads` threads. Stops early if `stop` is set.
#[expect(clippy::too_many_arguments, reason = "what a pass depends on")]
pub(crate) fn process_pending(
    database: &crate::db::Database,
    cache: Option<&crate::db::Database>,
    probe: &dyn FsProbe,
    ingest: &IngestConfig,
    counters: &ReadCounters,
    threads: usize,
    low_priority: bool,
    stop: &AtomicBool,
) -> Result<ContentReport> {
    let started = std::time::Instant::now();
    let mut report = ContentReport::default();
    // For the whole pass and not for each page, since starting a process is the slow part.
    let mut clients: Vec<IngestClient> = (0..threads.max(1))
        .map(|_| IngestClient::new(ingest.clone()))
        .collect();
    // Candidates first, because those are files the library may already know: until their hash
    // arrives, they show up as new documents, and everything said about them is wrong.
    for with_candidate in [true, false] {
        let mut after = 0;
        while !stop.load(Ordering::Relaxed) {
            let jobs = database.read(|connection| next_jobs(connection, with_candidate, after))?;
            let Some(last) = jobs.last() else {
                break;
            };
            after = last.revision;
            let results = run_jobs(jobs, probe, &mut clients, counters, low_priority, stop);
            store_in_cache(cache, &results);
            let applied = database.write(move |transaction| {
                let now = crate::sql::now_ms();
                let mut applied = ContentReport::default();
                for (job, finding) in results {
                    apply_finding(transaction, &job, finding, now, &mut applied)?;
                }
                Ok(applied)
            })?;
            add_up(&mut report, &applied);
        }
    }
    report.worker_restarts = clients.iter().map(IngestClient::restarts).sum();
    report.duration_ms = started.elapsed().as_millis() as u64;
    Ok(report)
}

fn add_up(total: &mut ContentReport, more: &ContentReport) {
    total.hashed += more.hashed;
    total.bytes_hashed += more.bytes_hashed;
    total.reattached += more.reattached;
    total.duplicates += more.duplicates;
    total.same_as_previous += more.same_as_previous;
    total.new_revisions += more.new_revisions;
    total.replacements_separated += more.replacements_separated;
    total.suggestions += more.suggestions;
    total.extracted += more.extracted;
    total.extraction_failed += more.extraction_failed;
    total.cloud_only += more.cloud_only;
    total.unreadable += more.unreadable;
    total.changed_meanwhile += more.changed_meanwhile;
}

fn next_jobs(connection: &Connection, with_candidate: bool, after: RevisionId) -> Result<Vec<Job>> {
    let sql = if with_candidate {
        "SELECT r.id, r.blake3, r.fast_fingerprint, r.ingested_version, r.size, d.format,
                i.id, i.folder_id, i.name, i.volume_serial, i.file_id, i.mtime_ns
         FROM revision r
         JOIN document d ON d.id = r.document_id
         JOIN file_instance i ON i.id = (
             SELECT o.id FROM file_instance o
             WHERE o.revision_id = r.id AND o.state IN ('present', 'unreadable')
             ORDER BY o.state = 'present' DESC, o.id LIMIT 1)
         WHERE (r.blake3 IS NULL OR r.ingested_version IS NULL)
           AND r.candidate_revision_id IS NOT NULL AND r.id > ?1
         ORDER BY r.id LIMIT ?2"
    } else {
        "SELECT r.id, r.blake3, r.fast_fingerprint, r.ingested_version, r.size, d.format,
                i.id, i.folder_id, i.name, i.volume_serial, i.file_id, i.mtime_ns
         FROM revision r
         JOIN document d ON d.id = r.document_id
         JOIN file_instance i ON i.id = (
             SELECT o.id FROM file_instance o
             WHERE o.revision_id = r.id AND o.state IN ('present', 'unreadable')
             ORDER BY o.state = 'present' DESC, o.id LIMIT 1)
         WHERE (r.blake3 IS NULL OR r.ingested_version IS NULL)
           AND r.candidate_revision_id IS NULL AND r.id > ?1
         ORDER BY r.id LIMIT ?2"
    };
    struct Row {
        revision: RevisionId,
        blake3: Option<Vec<u8>>,
        fingerprint: Option<Vec<u8>>,
        ingested: Option<i64>,
        size: i64,
        format: String,
        instance: InstanceId,
        folder: FolderId,
        name: std::ffi::OsString,
        volume: Option<i64>,
        file_id: Option<Vec<u8>>,
        mtime_ns: i64,
    }
    let rows = {
        let mut statement = connection.prepare_cached(sql)?;
        statement
            .query_map(params![after, PAGE as i64], |row| {
                Ok(Row {
                    revision: row.get(0)?,
                    blake3: row.get(1)?,
                    fingerprint: row.get(2)?,
                    ingested: row.get(3)?,
                    size: row.get(4)?,
                    format: row.get(5)?,
                    instance: row.get(6)?,
                    folder: row.get(7)?,
                    name: row.get::<_, StoredName>(8)?.0,
                    volume: row.get(9)?,
                    file_id: row.get(10)?,
                    mtime_ns: row.get(11)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut folders = std::collections::HashMap::new();
    let mut jobs = Vec::with_capacity(rows.len());
    for row in rows {
        let folder = match folders.get(&row.folder) {
            Some(path) => PathBuf::clone(path),
            None => {
                let path = folder_path(connection, row.folder)?;
                folders.insert(row.folder, path.clone());
                path
            }
        };
        jobs.push(Job {
            revision: row.revision,
            instance: row.instance,
            path: folder.join(row.name),
            volume: row.volume.map(volume_from_sql),
            file_id: file_id_from_sql(row.file_id),
            size: row.size as u64,
            mtime_ns: row.mtime_ns,
            needs_hash: row.blake3.is_none(),
            needs_fingerprint: row.fingerprint.is_none(),
            needs_extraction: row.ingested.is_none(),
            known_hash: row.blake3.and_then(|bytes| bytes.try_into().ok()),
            format: row.format,
        });
    }
    Ok(jobs)
}

fn run_jobs(
    jobs: Vec<Job>,
    probe: &dyn FsProbe,
    clients: &mut [IngestClient],
    counters: &ReadCounters,
    low_priority: bool,
    stop: &AtomicBool,
) -> Vec<(Job, Finding)> {
    let queue = Mutex::new(jobs.into_iter());
    let (sender, receiver) = mpsc::channel();
    thread::scope(|scope| {
        for client in clients.iter_mut() {
            let sender = sender.clone();
            let queue = &queue;
            scope.spawn(move || {
                let _background = low_priority.then(enter_background);
                loop {
                    let job = queue
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .next();
                    let Some(job) = job else {
                        break;
                    };
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let finding = examine(&job, probe, client, counters, stop);
                    if sender.send((job, finding)).is_err() {
                        break;
                    }
                }
            });
        }
    });
    drop(sender);
    receiver.into_iter().collect()
}

/// Reads what a revision is waiting for, at a moment when nothing here holds any lock.
fn examine(
    job: &Job,
    probe: &dyn FsProbe,
    client: &mut IngestClient,
    counters: &ReadCounters,
    stop: &AtomicBool,
) -> Finding {
    let content = match probe.open_content(&job.path, ReadPattern::Whole) {
        Ok(content) => content,
        Err(OpenError::CloudOnly) => return Finding::CloudOnly,
        Err(OpenError::Missing) => return Finding::Missing,
        Err(OpenError::Unreadable(_)) => return Finding::Unreadable,
    };
    counters.files_opened.fetch_add(1, Ordering::Relaxed);

    let is_expected = |volume: u64, file_id: Option<FileId>, size: u64, mtime_ns: i64| {
        job.volume == Some(volume)
            && job.file_id == file_id
            && job.size == size
            && job.mtime_ns == mtime_ns
    };
    let stat = content.stat;
    if !is_expected(stat.volume, stat.file_id, stat.size, stat.mtime_ns) {
        return Finding::ChangedMeanwhile;
    }

    let (hash, kind, bytes) = if job.needs_hash {
        match full_hash(&content.file, stop, counters) {
            Ok(Some(WholeFile { hash, bytes, head })) => {
                (Some(hash), Some(ContentKind::of(&head)), bytes)
            }
            Ok(None) => return Finding::ChangedMeanwhile,
            Err(_) => return Finding::Unreadable,
        }
    } else {
        (None, None, 0)
    };
    let fingerprint = if job.needs_fingerprint {
        match fast_fingerprint(&content.file, stat.size, counters) {
            Ok(fingerprint) => Some(fingerprint),
            Err(_) => return Finding::Unreadable,
        }
    } else {
        None
    };

    // A hash of bytes that changed halfway through describes nothing that ever existed.
    let after_reading = probe.stat(&job.path);
    if !after_reading.is_ok_and(|now| is_expected(now.volume, now.file_id, now.size, now.mtime_ns))
    {
        return Finding::ChangedMeanwhile;
    }

    let extraction = if !job.needs_extraction {
        Extraction::NotAsked
    } else {
        match Format::from_extension(&job.format) {
            None => Extraction::NotAsked,
            Some(format) => match client.extract(&content.file, format) {
                IngestOutcome::Extracted(extracted) => Extraction::Answer(Box::new(extracted)),
                IngestOutcome::Nothing(_) => Extraction::Nothing,
                IngestOutcome::Unavailable(_) => Extraction::Unavailable,
            },
        }
    };
    Finding::Read {
        hash,
        kind,
        fingerprint,
        bytes,
        extraction,
    }
}

/// What the worker read goes in the cache, by hash, so that reading it again needs no worker. If
/// this fails, the cache just has less in it.
fn store_in_cache(cache: Option<&crate::db::Database>, results: &[(Job, Finding)]) {
    let Some(cache) = cache else {
        return;
    };
    let mut entries = Vec::new();
    for (job, finding) in results {
        let Finding::Read {
            hash, extraction, ..
        } = finding
        else {
            continue;
        };
        let Some(hash) = hash.or(job.known_hash) else {
            continue;
        };
        match extraction {
            Extraction::Answer(extracted) => {
                entries.push((hash, serde_json::to_string(extracted).ok()));
            }
            Extraction::Nothing => entries.push((hash, None)),
            Extraction::NotAsked | Extraction::Unavailable => {}
        }
    }
    if entries.is_empty() {
        return;
    }
    let _ = cache.write(move |transaction| {
        for (hash, data) in entries {
            transaction
                .prepare_cached(
                    "INSERT OR REPLACE INTO extraction (blake3, extractor_version, data)
                     VALUES (?1, ?2, ?3)",
                )?
                .execute(params![hash.as_slice(), EXTRACTOR_VERSION, data])?;
        }
        Ok(())
    });
}

fn apply_finding(
    transaction: &Transaction<'_>,
    job: &Job,
    finding: Finding,
    now: i64,
    report: &mut ContentReport,
) -> Result<()> {
    let (hash, kind, fingerprint, bytes, extraction) = match finding {
        Finding::Read {
            hash,
            kind,
            fingerprint,
            bytes,
            extraction,
        } => (hash, kind, fingerprint, bytes, extraction),
        Finding::CloudOnly => {
            set_instance_state(transaction, job.instance, InstanceState::CloudOnly, now)?;
            report.cloud_only += 1;
            return Ok(());
        }
        Finding::Unreadable => {
            set_instance_state(transaction, job.instance, InstanceState::Unreadable, now)?;
            report.unreadable += 1;
            return Ok(());
        }
        Finding::Missing | Finding::ChangedMeanwhile => {
            report.changed_meanwhile += 1;
            return Ok(());
        }
    };
    if !instance_still_matches(transaction, job)? {
        report.changed_meanwhile += 1;
        return Ok(());
    }

    transaction
        .prepare_cached(
            "UPDATE file_instance SET state = 'present', state_since = ?2
             WHERE id = ?1 AND state = 'unreadable'",
        )?
        .execute(params![job.instance, now])?;
    if let Some(fingerprint) = fingerprint {
        transaction
            .prepare_cached("UPDATE revision SET fast_fingerprint = ?2 WHERE id = ?1")?
            .execute(params![job.revision, fingerprint.as_slice()])?;
    }

    if let Some(hash) = hash {
        report.hashed += 1;
        report.bytes_hashed += bytes;
        if let Some(existing) = revision_with_hash(transaction, &hash)?
            && existing != job.revision
        {
            merge_into_existing(transaction, job.revision, existing, now, report)?;
            return Ok(());
        }
        confirm(transaction, job.revision, &hash, kind, now, report)?;
    }

    match extraction {
        Extraction::Answer(extracted) => {
            apply_extraction(transaction, job.revision, &extracted, now, report)?;
            mark_ingested(transaction, job.revision)?;
            report.extracted += 1;
        }
        Extraction::Nothing => {
            mark_ingested(transaction, job.revision)?;
            report.extraction_failed += 1;
        }
        Extraction::NotAsked => mark_ingested(transaction, job.revision)?,
        Extraction::Unavailable => {}
    }
    settle_replacement(transaction, job.revision)
}

/// A replacement whose bytes and identifiers have both been read and found consistent is no longer
/// provisional, and there is nothing left to take back.
fn settle_replacement(transaction: &Transaction<'_>, revision: RevisionId) -> Result<()> {
    transaction
        .prepare_cached(
            "DELETE FROM replaced_file WHERE revision_id = ?1
               AND EXISTS (SELECT 1 FROM revision WHERE id = ?1
                           AND blake3 IS NOT NULL AND ingested_version IS NOT NULL)",
        )?
        .execute([revision])?;
    Ok(())
}

/// What a provisional replacement took the place of.
struct Replaced {
    instance: InstanceId,
    folder: FolderId,
    name: rusqlite::types::Value,
    volume: Option<i64>,
    file_id: Option<Vec<u8>>,
    mtime_ns: i64,
}

fn replaced_by(connection: &Connection, revision: RevisionId) -> Result<Option<Replaced>> {
    Ok(connection
        .prepare_cached(
            "SELECT instance_id, folder_id, name, volume_serial, file_id, mtime_ns
             FROM replaced_file WHERE revision_id = ?1",
        )?
        .query_row([revision], |row| {
            Ok(Replaced {
                instance: row.get(0)?,
                folder: row.get(1)?,
                name: row.get(2)?,
                volume: row.get(3)?,
                file_id: row.get(4)?,
                mtime_ns: row.get(5)?,
            })
        })
        .optional()?)
}

/// Takes back a provisional replacement: the instance becomes again the file it was before, which
/// is gone from its path, and the file that is there now gets an instance of its own on `revision`,
/// which is returned. `None` if there is nothing to take back, or the instance has moved on since.
fn restore_replaced(
    transaction: &Transaction<'_>,
    revision: RevisionId,
    now: i64,
) -> Result<Option<InstanceId>> {
    let Some(replaced) = replaced_by(transaction, revision)? else {
        return Ok(None);
    };
    transaction
        .prepare_cached("DELETE FROM replaced_file WHERE revision_id = ?1")?
        .execute([revision])?;
    let previous: Option<RevisionId> = transaction
        .prepare_cached("SELECT previous_revision_id FROM revision WHERE id = ?1")?
        .query_row([revision], |row| row.get(0))?;
    type Now = (
        RevisionId,
        FolderId,
        rusqlite::types::Value,
        Option<i64>,
        Option<Vec<u8>>,
        i64,
        InstanceState,
    );
    let current: Option<Now> = transaction
        .prepare_cached(
            "SELECT revision_id, folder_id, name, volume_serial, file_id, mtime_ns, state
             FROM file_instance WHERE id = ?1",
        )?
        .query_row([replaced.instance], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
            ))
        })
        .optional()?;
    let (Some(previous), Some((on, folder, name, volume, file_id, mtime_ns, state))) =
        (previous, current)
    else {
        return Ok(None);
    };
    if on != revision {
        // The file changed again in place, and the newer revision has its own story.
        return Ok(None);
    }

    // First out of the way, since only one instance can be at a path without being missing.
    transaction
        .prepare_cached(
            "UPDATE file_instance
             SET revision_id = ?2, folder_id = ?3, name = ?4, volume_serial = ?5, file_id = ?6,
                 mtime_ns = ?7, state = 'absent', state_since = ?8
             WHERE id = ?1",
        )?
        .execute(params![
            replaced.instance,
            previous,
            replaced.folder,
            replaced.name,
            replaced.volume,
            replaced.file_id,
            replaced.mtime_ns,
            now,
        ])?;
    transaction
        .prepare_cached(
            "INSERT INTO file_instance (revision_id, folder_id, name, volume_serial, file_id, mtime_ns,
                                        state, state_since)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?
        .execute(params![
            revision, folder, name, volume, file_id, mtime_ns, state, now
        ])?;
    let arrived = transaction.last_insert_rowid();
    // What the library did to the file since it arrived was done to this one.
    transaction
        .prepare_cached(
            "UPDATE fs_operation SET instance_id = ?2
             WHERE instance_id = ?1
               AND created_at >= (SELECT created_at FROM revision WHERE id = ?3)",
        )?
        .execute(params![replaced.instance, arrived, revision])?;
    Ok(Some(arrived))
}

/// A provisional replacement turned out to be something else. It becomes what it would have been
/// without the shortcut: a document of its own, beside the one whose file it replaced, which is now
/// missing, and a suggestion between the two with the reason. Returns whether that happened.
fn separate_replacement(
    transaction: &Transaction<'_>,
    revision: RevisionId,
    evidence: &str,
    now: i64,
    report: &mut ContentReport,
) -> Result<bool> {
    let Some(arrived) = restore_replaced(transaction, revision, now)? else {
        return Ok(false);
    };
    let (old_document, name): (DocumentId, std::ffi::OsString) = transaction
        .prepare_cached(
            "SELECT r.document_id, i.name FROM file_instance i
             JOIN revision r ON r.id = ?2 WHERE i.id = ?1",
        )?
        .query_row(params![arrived, revision], |row| {
            Ok((row.get(0)?, row.get::<_, StoredName>(1)?.0))
        })?;
    let document = insert_work_and_document(transaction, &name, now)?;
    transaction
        .prepare_cached(
            "UPDATE revision SET document_id = ?2, previous_revision_id = NULL, origin = 'import'
             WHERE id = ?1",
        )?
        .execute(params![revision, document])?;
    // Notes made on these bytes while they passed for the old document's are about these bytes, and
    // so is a position taken on them.
    transaction
        .prepare_cached("UPDATE annotation SET document_id = ?2 WHERE revision_id = ?1")?
        .execute(params![revision, document])?;
    // The old document's annotations were found on these bytes; they are someone else's now, so
    // those annotations are found again on whatever the old document opens with next.
    transaction
        .prepare_cached(
            "UPDATE annotation SET resolved_revision_id = NULL
             WHERE resolved_revision_id = ?1 AND document_id <> ?2",
        )?
        .execute(params![revision, document])?;
    transaction
        .prepare_cached("UPDATE reading_state SET document_id = ?2 WHERE revision_id = ?1")?
        .execute(params![revision, document])?;
    if suggest(
        transaction,
        SuggestionKind::SamePath,
        document,
        old_document,
        evidence,
        now,
    )? {
        report.suggestions += 1;
    }
    report.replacements_separated += 1;
    Ok(true)
}

fn set_instance_state(
    transaction: &Transaction<'_>,
    instance: InstanceId,
    state: InstanceState,
    now: i64,
) -> Result<()> {
    transaction
        .prepare_cached("UPDATE file_instance SET state = ?2, state_since = ?3 WHERE id = ?1")?
        .execute(params![instance, state, now])?;
    Ok(())
}

fn mark_ingested(transaction: &Transaction<'_>, revision: RevisionId) -> Result<()> {
    transaction
        .prepare_cached("UPDATE revision SET ingested_version = ?2 WHERE id = ?1")?
        .execute(params![revision, EXTRACTOR_VERSION])?;
    Ok(())
}

/// Whether the library's record of this file is still what was read. A scan may have run meanwhile.
fn instance_still_matches(transaction: &Transaction<'_>, job: &Job) -> Result<bool> {
    type Stored = (RevisionId, Option<i64>, Option<Vec<u8>>, i64, i64);
    let row: Option<Stored> = transaction
        .prepare_cached(
            "SELECT i.revision_id, i.volume_serial, i.file_id, i.mtime_ns, r.size
             FROM file_instance i JOIN revision r ON r.id = i.revision_id WHERE i.id = ?1",
        )?
        .query_row([job.instance], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .optional()?;
    Ok(
        row.is_some_and(|(revision, volume, file_id, mtime_ns, size)| {
            revision == job.revision
                && volume.map(volume_from_sql) == job.volume
                && file_id_from_sql(file_id) == job.file_id
                && mtime_ns == job.mtime_ns
                && size as u64 == job.size
        }),
    )
}

fn revision_with_hash(connection: &Connection, hash: &Hash) -> Result<Option<RevisionId>> {
    Ok(connection
        .prepare_cached("SELECT id FROM revision WHERE blake3 = ?1")?
        .query_row([hash.as_slice()], |row| row.get(0))
        .optional()?)
}

/// These bytes are new, so this revision stands. What is left to say about it is only ever a
/// suggestion, which is steps 4 and 5.
fn confirm(
    transaction: &Transaction<'_>,
    revision: RevisionId,
    hash: &Hash,
    kind: Option<ContentKind>,
    now: i64,
    report: &mut ContentReport,
) -> Result<()> {
    let (mut document, mut origin, candidate, candidate_hash): (
        DocumentId,
        RevisionOrigin,
        Option<RevisionId>,
        Option<Vec<u8>>,
    ) = transaction
        .prepare_cached(
            "SELECT r.document_id, r.origin, r.candidate_revision_id,
                    (SELECT c.blake3 FROM revision c WHERE c.id = r.candidate_revision_id)
             FROM revision r WHERE r.id = ?1",
        )?
        .query_row([revision], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?;

    transaction
        .prepare_cached(
            "UPDATE revision SET blake3 = ?2, content_kind = ?3, candidate_revision_id = NULL
             WHERE id = ?1",
        )?
        .execute(params![revision, hash.as_slice(), kind])?;
    report.new_revisions += 1;

    if origin == RevisionOrigin::ReplacedAtPath
        && let Some(kind) = kind
        && let Some(was) = kind_replaced_by(transaction, revision)?
        && was != kind
    {
        let evidence = format!(
            "took the path of a file of another kind: {} before, {} now",
            was.as_str(),
            kind.as_str()
        );
        if separate_replacement(transaction, revision, &evidence, now, report)? {
            (document, origin) = transaction
                .prepare_cached("SELECT document_id, origin FROM revision WHERE id = ?1")?
                .query_row([revision], |row| Ok((row.get(0)?, row.get(1)?)))?;
        }
    }
    if origin != RevisionOrigin::Import {
        return Ok(());
    }

    // A candidate that was never hashed cannot be told apart, and the file is gone, so it never
    // will be. Somebody may recognize it, though.
    if let (Some(candidate), None) = (candidate, candidate_hash) {
        let other: DocumentId = transaction
            .prepare_cached("SELECT document_id FROM revision WHERE id = ?1")?
            .query_row([candidate], |row| row.get(0))?;
        if other != document
            && suggest(
                transaction,
                SuggestionKind::SameFingerprint,
                document,
                other,
                "the same fast fingerprint as a missing file that was never fully hashed",
                now,
            )?
        {
            report.suggestions += 1;
        }
    }

    // Something else used to be at this very path, and it is gone.
    let others: Vec<DocumentId> = {
        let mut statement = transaction.prepare_cached(
            "SELECT DISTINCT r.document_id
             FROM file_instance mine
             JOIN file_instance other
               ON other.folder_id = mine.folder_id AND other.name = mine.name
              AND other.state = 'absent' AND other.id <> mine.id
             JOIN revision r ON r.id = other.revision_id
             WHERE mine.revision_id = ?1 AND r.document_id <> ?2",
        )?;
        statement
            .query_map(params![revision, document], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    for other in others {
        if suggest(
            transaction,
            SuggestionKind::SamePath,
            document,
            other,
            "found at the path of a file that had gone missing, with different contents",
            now,
        )? {
            report.suggestions += 1;
        }
    }
    Ok(())
}

/// The kind of file a provisional replacement took the place of, if it is still provisional. A
/// revision hashed before kinds were recorded is taken to be what its document's format says.
fn kind_replaced_by(connection: &Connection, revision: RevisionId) -> Result<Option<ContentKind>> {
    let row: Option<(Option<ContentKind>, String)> = connection
        .prepare_cached(
            "SELECT previous.content_kind, d.format
             FROM replaced_file x
             JOIN revision r ON r.id = x.revision_id
             JOIN revision previous ON previous.id = r.previous_revision_id
             JOIN document d ON d.id = r.document_id
             WHERE x.revision_id = ?1",
        )?
        .query_row([revision], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?;
    Ok(row.map(|(kind, format)| kind.unwrap_or_else(|| ContentKind::expected_for(&format))))
}

/// Why a provisional replacement cannot be a revision of its document, by what it says about itself:
/// an internal ID that both have and that differs.
fn identifier_conflict(
    connection: &Connection,
    revision: RevisionId,
    extracted: &Extracted,
) -> Result<Option<String>> {
    let row: Option<(Option<String>, Option<String>)> = connection
        .prepare_cached(
            "SELECT d.pdf_id0, d.dc_identifier
             FROM replaced_file x
             JOIN revision r ON r.id = x.revision_id
             JOIN document d ON d.id = r.document_id
             WHERE x.revision_id = ?1",
        )?
        .query_row([revision], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?;
    let Some((pdf_id0, dc_identifier)) = row else {
        return Ok(None);
    };
    for (label, known, found) in [
        ("PDF /ID[0]", pdf_id0, &extracted.pdf_id0),
        ("dc:identifier", dc_identifier, &extracted.dc_identifier),
    ] {
        if let (Some(known), Some(found)) = (known, found)
            && known != *found
        {
            return Ok(Some(format!(
                "took the path of a file with another {label}: {known} before, {found} now"
            )));
        }
    }
    Ok(None)
}

/// The file is a revision the library already has. The unconfirmed one was only ever a guess, and
/// everything attached to it moves over before it goes.
fn merge_into_existing(
    transaction: &Transaction<'_>,
    from: RevisionId,
    into: RevisionId,
    now: i64,
    report: &mut ContentReport,
) -> Result<()> {
    let (from_document, previous): (DocumentId, Option<RevisionId>) = transaction
        .prepare_cached("SELECT document_id, previous_revision_id FROM revision WHERE id = ?1")?
        .query_row([from], |row| Ok((row.get(0)?, row.get(1)?)))?;
    let into_document: DocumentId = transaction
        .prepare_cached("SELECT document_id FROM revision WHERE id = ?1")?
        .query_row([into], |row| row.get(0))?;

    // A provisional replacement whose bytes are another document's: the file it took the place of did
    // not become that document, it went missing, and the instance goes back to being it.
    if into_document != from_document && restore_replaced(transaction, from, now)?.is_some() {
        report.replacements_separated += 1;
    }
    transaction
        .prepare_cached("DELETE FROM replaced_file WHERE revision_id = ?1")?
        .execute([from])?;

    let instances: Vec<InstanceId> = {
        let mut statement =
            transaction.prepare_cached("SELECT id FROM file_instance WHERE revision_id = ?1")?;
        statement
            .query_map([from], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    for instance in instances {
        if previous == Some(into) {
            // A touch, or a safe save of the same bytes. The file never stopped being this revision.
            attach(transaction, instance, into)?;
            report.same_as_previous += 1;
        } else if let Some(absent) = absent_instance_of(transaction, into, instance)? {
            take_over(transaction, absent, instance, now)?;
            report.reattached += 1;
        } else {
            attach(transaction, instance, into)?;
            report.duplicates += 1;
        }
    }

    transaction
        .prepare_cached(
            "UPDATE annotation SET revision_id = ?2, document_id = ?3 WHERE revision_id = ?1",
        )?
        .execute(params![from, into, into_document])?;
    // The same bytes, so whatever an annotation's anchors were worked out on holds on the other.
    transaction
        .prepare_cached(
            "UPDATE annotation SET resolved_revision_id = ?2 WHERE resolved_revision_id = ?1",
        )?
        .execute(params![from, into])?;
    transaction
        .prepare_cached("UPDATE anchor_selector SET revision_id = ?2 WHERE revision_id = ?1")?
        .execute(params![from, into])?;
    // The same bytes, so a position taken on one is right on the other.
    transaction
        .prepare_cached("UPDATE reading_state SET revision_id = ?2 WHERE revision_id = ?1")?
        .execute(params![from, into])?;
    transaction
        .prepare_cached(
            "UPDATE revision SET previous_revision_id = ?2 WHERE previous_revision_id = ?1",
        )?
        .execute(params![from, previous])?;
    transaction
        .prepare_cached(
            "UPDATE revision SET candidate_revision_id = NULL WHERE candidate_revision_id = ?1",
        )?
        .execute([from])?;
    transaction
        .prepare_cached("DELETE FROM revision WHERE id = ?1")?
        .execute([from])?;

    let others: i64 = transaction
        .prepare_cached("SELECT count(*) FROM revision WHERE document_id = ?1")?
        .query_row([from_document], |row| row.get(0))?;
    if others == 0 && from_document != into_document {
        merge_empty_document(transaction, from_document, into_document, now)?;
    }
    Ok(())
}

fn attach(transaction: &Transaction<'_>, instance: InstanceId, revision: RevisionId) -> Result<()> {
    transaction
        .prepare_cached("UPDATE file_instance SET revision_id = ?2 WHERE id = ?1")?
        .execute(params![instance, revision])?;
    Ok(())
}

/// An instance of this revision that has gone missing. One last seen at the same path goes first.
fn absent_instance_of(
    connection: &Connection,
    revision: RevisionId,
    arriving: InstanceId,
) -> Result<Option<InstanceId>> {
    Ok(connection
        .prepare_cached(
            "SELECT absent.id
             FROM file_instance absent, file_instance arriving
             WHERE absent.revision_id = ?1 AND absent.state = 'absent' AND arriving.id = ?2
             ORDER BY absent.folder_id = arriving.folder_id AND absent.name = arriving.name DESC,
                      absent.id
             LIMIT 1",
        )?
        .query_row(params![revision, arriving], |row| row.get(0))
        .optional()?)
}

/// The file that went missing is here. Its row keeps its ID and takes over this one's whereabouts.
fn take_over(
    transaction: &Transaction<'_>,
    absent: InstanceId,
    arriving: InstanceId,
    now: i64,
) -> Result<()> {
    type Location = (
        FolderId,
        rusqlite::types::Value,
        Option<i64>,
        Option<Vec<u8>>,
        i64,
        InstanceState,
    );
    let (folder, name, volume, file_id, mtime_ns, state): Location = transaction
        .prepare_cached(
            "SELECT folder_id, name, volume_serial, file_id, mtime_ns, state
             FROM file_instance WHERE id = ?1",
        )?
        .query_row([arriving], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        })?;
    transaction
        .prepare_cached("UPDATE fs_operation SET instance_id = ?2 WHERE instance_id = ?1")?
        .execute(params![arriving, absent])?;
    transaction
        .prepare_cached("DELETE FROM file_instance WHERE id = ?1")?
        .execute([arriving])?;
    transaction
        .prepare_cached(
            "UPDATE file_instance
             SET folder_id = ?2, name = ?3, volume_serial = ?4, file_id = ?5, mtime_ns = ?6,
                 state = ?7, state_since = ?8
             WHERE id = ?1",
        )?
        .execute(params![
            absent, folder, name, volume, file_id, mtime_ns, state, now
        ])?;
    Ok(())
}

/// A document left with nothing in it, because its only revision turned out to be another's. What
/// a person put on it goes to the one it turned out to be.
pub(crate) fn merge_empty_document(
    transaction: &Transaction<'_>,
    from: DocumentId,
    into: DocumentId,
    now: i64,
) -> Result<()> {
    let from_work: WorkId = transaction
        .prepare_cached("SELECT work_id FROM document WHERE id = ?1")?
        .query_row([from], |row| row.get(0))?;
    let into_work: WorkId = transaction
        .prepare_cached("SELECT work_id FROM document WHERE id = ?1")?
        .query_row([into], |row| row.get(0))?;

    transaction
        .prepare_cached("UPDATE annotation SET document_id = ?2 WHERE document_id = ?1")?
        .execute(params![from, into])?;
    // Where the reader was goes along, unless the other one has a more recent position of its own.
    transaction
        .prepare_cached(
            "DELETE FROM reading_state WHERE document_id = ?2
               AND updated_at < (SELECT updated_at FROM reading_state WHERE document_id = ?1)",
        )?
        .execute(params![from, into])?;
    transaction
        .prepare_cached(
            "DELETE FROM reading_state WHERE document_id = ?1
               AND EXISTS (SELECT 1 FROM reading_state WHERE document_id = ?2)",
        )?
        .execute(params![from, into])?;
    transaction
        .prepare_cached("UPDATE reading_state SET document_id = ?2 WHERE document_id = ?1")?
        .execute(params![from, into])?;
    transaction
        .prepare_cached(
            "UPDATE OR IGNORE reconcile_suggestion SET document_id = ?2 WHERE document_id = ?1",
        )?
        .execute(params![from, into])?;
    transaction
        .prepare_cached(
            "UPDATE OR IGNORE reconcile_suggestion SET candidate_document_id = ?2
             WHERE candidate_document_id = ?1",
        )?
        .execute(params![from, into])?;
    transaction
        .prepare_cached(
            "DELETE FROM reconcile_suggestion
             WHERE document_id = ?1 OR candidate_document_id = ?1
                OR document_id = candidate_document_id",
        )?
        .execute([from])?;
    transaction
        .prepare_cached("DELETE FROM document WHERE id = ?1")?
        .execute([from])?;

    let documents_left: i64 = transaction
        .prepare_cached("SELECT count(*) FROM document WHERE work_id = ?1")?
        .query_row([from_work], |row| row.get(0))?;
    if documents_left == 0 && from_work != into_work {
        merge_empty_work(transaction, from_work, into_work, now)?;
    }
    Ok(())
}

/// A work left with no documents. Whatever a person set on it, and is not set on the work it turned
/// out to be, moves there. If both have a value, the one that was already there stays.
fn merge_empty_work(
    transaction: &Transaction<'_>,
    from: WorkId,
    into: WorkId,
    now: i64,
) -> Result<()> {
    let user_fields: Vec<String> = {
        let mut statement = transaction.prepare_cached(
            "SELECT field FROM field_provenance WHERE work_id = ?1 AND origin = 'user'",
        )?;
        statement
            .query_map([from], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    for field in user_fields {
        let taken: Option<String> = transaction
            .prepare_cached(
                "SELECT origin FROM field_provenance WHERE work_id = ?1 AND field = ?2",
            )?
            .query_row(params![into, field], |row| row.get(0))
            .optional()?;
        if taken.as_deref() == Some("user") {
            continue;
        }
        let column = match field.as_str() {
            "title" => "title",
            "language" => "language",
            "year" => "year",
            "series" => "series",
            "series_position" => "series_position",
            "rating" => "rating",
            "status" => "status",
            "creators" => {
                transaction
                    .prepare_cached("DELETE FROM work_creator WHERE work_id = ?1")?
                    .execute([into])?;
                transaction
                    .prepare_cached("UPDATE work_creator SET work_id = ?2 WHERE work_id = ?1")?
                    .execute(params![from, into])?;
                crate::search::refresh_entry(transaction, into)?;
                set_provenance(transaction, into, &field, "user", now)?;
                continue;
            }
            _ => continue,
        };
        // The column comes from the list above, not from the database, so this cannot be injected.
        transaction.execute(
            &format!(
                "UPDATE work SET {column} = (SELECT {column} FROM work WHERE id = ?1), updated_at = ?3
                 WHERE id = ?2"
            ),
            params![from, into, now],
        )?;
        set_provenance(transaction, into, &field, "user", now)?;
    }
    transaction
        .prepare_cached("DELETE FROM work_creator WHERE work_id = ?1")?
        .execute([from])?;
    transaction
        .prepare_cached("DELETE FROM field_provenance WHERE work_id = ?1")?
        .execute([from])?;
    transaction
        .prepare_cached("DELETE FROM work WHERE id = ?1")?
        .execute([from])?;
    Ok(())
}

pub(crate) fn set_provenance(
    transaction: &Transaction<'_>,
    work: WorkId,
    field: &str,
    origin: &str,
    now: i64,
) -> Result<()> {
    transaction
        .prepare_cached(
            "INSERT OR REPLACE INTO field_provenance (work_id, field, origin, updated_at)
             VALUES (?1, ?2, ?3, ?4)",
        )?
        .execute(params![work, field, origin, now])?;
    Ok(())
}

fn field_origin(connection: &Connection, work: WorkId, field: &str) -> Result<Option<String>> {
    Ok(connection
        .prepare_cached("SELECT origin FROM field_provenance WHERE work_id = ?1 AND field = ?2")?
        .query_row(params![work, field], |row| row.get(0))
        .optional()?)
}

/// Stores what a file says about itself, and step 4. A field a person edited is never written over.
pub(crate) fn apply_extraction(
    transaction: &Transaction<'_>,
    revision: RevisionId,
    extracted: &Extracted,
    now: i64,
    report: &mut ContentReport,
) -> Result<()> {
    let identity = |transaction: &Transaction<'_>| -> Result<(DocumentId, WorkId, RevisionOrigin)> {
        Ok(transaction
            .prepare_cached(
                "SELECT r.document_id, d.work_id, r.origin
                 FROM revision r JOIN document d ON d.id = r.document_id WHERE r.id = ?1",
            )?
            .query_row([revision], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?)
    };
    let (mut document, mut work, mut origin) = identity(transaction)?;
    // Before anything is written to the document: these may not be its bytes at all.
    if origin == RevisionOrigin::ReplacedAtPath
        && let Some(evidence) = identifier_conflict(transaction, revision, extracted)?
        && separate_replacement(transaction, revision, &evidence, now, report)?
    {
        (document, work, origin) = identity(transaction)?;
    }

    // The direction is the newest file's word, unlike identifiers, which are the first one's.
    let orientation = extracted.direction.map(|direction| match direction {
        Direction::L2R => "ltr",
        Direction::R2L => "rtl",
    });
    transaction
        .prepare_cached(
            "UPDATE document SET pdf_id0 = coalesce(pdf_id0, ?2),
                 dc_identifier = coalesce(dc_identifier, ?3), isbn = coalesce(isbn, ?4),
                 orientation = coalesce(?5, orientation)
             WHERE id = ?1",
        )?
        .execute(params![
            document,
            extracted.pdf_id0,
            extracted.dc_identifier,
            extracted.isbn,
            orientation,
        ])?;

    if origin == RevisionOrigin::Import {
        if let Some(id) = &extracted.pdf_id0 {
            suggest_same_identifier(
                transaction,
                "pdf_id0",
                id,
                document,
                "the same PDF /ID[0]",
                now,
                report,
            )?;
        }
        if let Some(id) = &extracted.dc_identifier {
            suggest_same_identifier(
                transaction,
                "dc_identifier",
                id,
                document,
                "the same dc:identifier",
                now,
                report,
            )?;
        }
    }

    let writable = |origin: Option<String>| origin.as_deref() != Some("user");

    let mut creators_changed = false;
    if !extracted.creators.is_empty() && writable(field_origin(transaction, work, "creators")?) {
        if creators_of(transaction, work)? != extracted.creators {
            set_creators(transaction, work, &extracted.creators)?;
            creators_changed = true;
        }
        set_provenance(transaction, work, "creators", "embedded", now)?;
    }
    let title = extracted
        .title
        .as_ref()
        .filter(|_| writable(field_origin(transaction, work, "title").ok().flatten()));
    if title.is_some() {
        set_provenance(transaction, work, "title", "embedded", now)?;
    }
    let current_title: String = transaction
        .prepare_cached("SELECT title FROM work WHERE id = ?1")?
        .query_row([work], |row| row.get(0))?;
    let title_changes = title.is_some_and(|title| *title != current_title);
    if extracted.language.is_some() && writable(field_origin(transaction, work, "language")?) {
        transaction
            .prepare_cached("UPDATE work SET language = ?2 WHERE id = ?1")?
            .execute(params![work, extracted.language])?;
        set_provenance(transaction, work, "language", "embedded", now)?;
    }
    if extracted.year.is_some() && writable(field_origin(transaction, work, "year")?) {
        transaction
            .prepare_cached("UPDATE work SET year = ?2 WHERE id = ?1")?
            .execute(params![work, extracted.year])?;
        set_provenance(transaction, work, "year", "embedded", now)?;
    }
    // After the creators, so that if the title changes, the trigger writes the index entry just once
    // and with everything in it.
    transaction
        .prepare_cached(
            "UPDATE work SET title = coalesce(?2, title), updated_at = ?3 WHERE id = ?1",
        )?
        .execute(params![work, title, now])?;
    if creators_changed && !title_changes {
        crate::search::refresh_entry(transaction, work)?;
    }
    Ok(())
}

fn creators_of(connection: &Connection, work: WorkId) -> Result<Vec<String>> {
    let mut statement = connection.prepare_cached(
        "SELECT c.name FROM work_creator wc JOIN creator c ON c.id = wc.creator_id
         WHERE wc.work_id = ?1 ORDER BY wc.position",
    )?;
    let rows = statement.query_map([work], |row| row.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn suggest_same_identifier(
    transaction: &Transaction<'_>,
    column: &str,
    value: &str,
    document: DocumentId,
    evidence: &str,
    now: i64,
    report: &mut ContentReport,
) -> Result<()> {
    // The column is one of two literals at the call sites.
    let others: Vec<DocumentId> = {
        let mut statement = transaction.prepare_cached(&format!(
            "SELECT id FROM document WHERE {column} = ?1 AND id <> ?2 ORDER BY id LIMIT ?3"
        ))?;
        statement
            .query_map(
                params![value, document, MAX_SUGGESTIONS_PER_IDENTIFIER],
                |row| row.get(0),
            )?
            .collect::<rusqlite::Result<_>>()?
    };
    for other in others {
        if suggest(
            transaction,
            SuggestionKind::SameInternalId,
            document,
            other,
            &format!("{evidence}: {value}"),
            now,
        )? {
            report.suggestions += 1;
        }
    }
    Ok(())
}

/// Replaces a work's creators. Bringing the search indexes up to date is up to the caller, who may be
/// about to do that anyway.
pub(crate) fn set_creators(
    transaction: &Transaction<'_>,
    work: WorkId,
    creators: &[String],
) -> Result<()> {
    transaction
        .prepare_cached("DELETE FROM work_creator WHERE work_id = ?1")?
        .execute([work])?;
    for (position, name) in creators.iter().enumerate() {
        transaction
            .prepare_cached("INSERT OR IGNORE INTO creator (name) VALUES (?1)")?
            .execute([name])?;
        let creator: i64 = transaction
            .prepare_cached("SELECT id FROM creator WHERE name = ?1")?
            .query_row([name], |row| row.get(0))?;
        transaction
            .prepare_cached(
                "INSERT INTO work_creator (work_id, creator_id, position) VALUES (?1, ?2, ?3)",
            )?
            .execute(params![work, creator, position as i64])?;
    }
    Ok(())
}

/// Whether any revision is waiting for its full hash or metadata and has its bytes here to read.
pub(crate) fn has_pending(connection: &Connection) -> Result<bool> {
    Ok(connection
        .prepare_cached(
            "SELECT EXISTS (
                 SELECT 1 FROM revision r
                 WHERE (r.blake3 IS NULL OR r.ingested_version IS NULL)
                   AND EXISTS (SELECT 1 FROM file_instance i
                               WHERE i.revision_id = r.id AND i.state = 'present'))",
        )?
        .query_row([], |row| row.get(0))?)
}

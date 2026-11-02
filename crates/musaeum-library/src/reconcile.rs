//! Compares what the disk has with what the library says.
//!
//! The disk is the truth, and this is the only thing that decides what is on it. Events from the
//! system only say where to look first, so a lost event costs some delay and nothing else, and the
//! same code covers a first scan, a change in real time and everything that happened while the
//! program was closed.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use musaeum_fs::{Entry, FileId, OpenError, ReadPattern, enter_background};
use rusqlite::{Connection, OptionalExtension, params};

use crate::db::Database;
use crate::error::{Error, Result};
use crate::fingerprint::{ReadCounters, fast_fingerprint};
use crate::identity::{
    Observation, ObservedState, Pending, ReconcileReport, apply_observations, apply_walk,
};
use crate::model::{FolderId, RootId, RootInfo};
use crate::probe::FsProbe;
use crate::scan::{Exclusions, Walk, WalkParent, WalkStart, walk};
use crate::sql::{StoredName, name_to_sql, now_ms, volume_to_sql};

/// How many observations go into one transaction. Each commit waits for the disk, so one for each
/// file would be slow, and one for all of them would keep what was found from being seen until the
/// very end.
const OBSERVATION_BATCH: usize = 512;

/// How much of a root to look at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReconcileScope {
    /// All of it.
    Everything,
    /// These directories, and whatever under them turns out to be new or moved.
    Directories(HashSet<PathBuf>),
}

/// What reconciling needs from the library.
pub(crate) struct Context<'a> {
    pub database: &'a Database,
    pub probe: &'a Arc<dyn FsProbe>,
    pub counters: &'a ReadCounters,
    pub threads: usize,
    pub low_priority: bool,
    pub stop: &'a AtomicBool,
}

/// Brings the library up to date with one root.
pub(crate) fn reconcile_root(
    context: &Context<'_>,
    root: &RootInfo,
    scope: &ReconcileScope,
) -> Result<ReconcileReport> {
    let started = Instant::now();
    let (opened_before, read_before) = context.counters.snapshot();
    let mut report = ReconcileReport {
        root_id: root.id,
        root_available: true,
        ..ReconcileReport::default()
    };

    let exclusions = Exclusions::new(&root.exclusions);
    let found = match scope {
        ReconcileScope::Everything => walk(
            context.probe.as_ref(),
            &exclusions,
            vec![WalkStart {
                path: root.path.clone(),
                parent: WalkParent::Top,
                name: OsString::new(),
            }],
            &|_, _, _, _| true,
        ),
        ReconcileScope::Directories(directories) => {
            walk_directories(context, root, &exclusions, directories)
        }
    };

    let found = match found {
        Ok(found) => found,
        Err(_) => return mark_unavailable(context, root.id, report),
    };
    // A root that is not there is not a root whose files have all gone away.
    if found.dirs.is_empty() && !found.missing_starts.is_empty() {
        if *scope == ReconcileScope::Everything {
            return mark_unavailable(context, root.id, report);
        }
        return Ok(report);
    }
    report.directories_unreadable = found.unreadable_directories;

    let is_full_scan = *scope == ReconcileScope::Everything;
    // The closure goes to the writer's thread, so it needs a probe of its own to hold on to.
    let probe = Arc::clone(context.probe);
    let root_id = root.id;
    let (pending, after_walk) = context.database.write({
        let mut report = report;
        move |transaction| {
            let now = now_ms();
            let pending = apply_walk(
                transaction,
                root_id,
                &found,
                probe.as_ref(),
                now,
                &mut report,
            )?;
            transaction
                .prepare_cached("UPDATE library_root SET status = 'available' WHERE id = ?1 AND status = 'unavailable'")?
                .execute([root_id])?;
            if is_full_scan {
                transaction
                    .prepare_cached("UPDATE library_root SET last_reconciled_at = ?2 WHERE id = ?1")?
                    .execute(params![root_id, now])?;
            }
            Ok((pending, report))
        }
    })?;
    report = after_walk;

    observe_pending(context, pending, &mut report)?;

    let (opened_after, read_after) = context.counters.snapshot();
    report.files_opened = opened_after - opened_before;
    report.bytes_read = read_after - read_before;
    report.duration_ms = started.elapsed().as_millis() as u64;
    Ok(report)
}

fn mark_unavailable(
    context: &Context<'_>,
    root_id: RootId,
    mut report: ReconcileReport,
) -> Result<ReconcileReport> {
    context.database.write(move |transaction| {
        transaction
            .prepare_cached("UPDATE library_root SET status = 'unavailable' WHERE id = ?1 AND status = 'available'")?
            .execute([root_id])?;
        Ok(())
    })?;
    report.root_available = false;
    Ok(report)
}

fn walk_directories(
    context: &Context<'_>,
    root: &RootInfo,
    exclusions: &Exclusions,
    directories: &HashSet<PathBuf>,
) -> std::io::Result<Walk> {
    let starts = context
        .database
        .read(|connection| {
            let mut starts = Vec::new();
            let mut seen = HashSet::new();
            let mut sorted: Vec<&PathBuf> = directories.iter().collect();
            // Parents first, so that a child already reached from its parent is not listed twice.
            sorted.sort_by_key(|path| path.components().count());
            for directory in sorted {
                if let Some(start) = resolve_start(connection, root, exclusions, directory)?
                    && seen.insert(start.path.clone())
                {
                    starts.push(start);
                }
            }
            Ok(starts)
        })
        .map_err(std::io::Error::other)?;

    let database = context.database;
    let descend = |path: &Path, volume: u64, parent_id: Option<FileId>, entry: &Entry| {
        directories.contains(path) || !is_settled(database, volume, parent_id, entry)
    };
    let mut result = walk(context.probe.as_ref(), exclusions, starts, &descend)?;

    // A directory that is gone says something about its parent, and a walk that started at a
    // deleted directory did not list that. Going up until something exists covers it.
    let mut listed: HashSet<PathBuf> = result.dirs.iter().map(|dir| dir.path.clone()).collect();
    let mut queue = std::mem::take(&mut result.missing_starts);
    while let Some(missing) = queue.pop() {
        let Some(parent) = missing.parent().map(Path::to_path_buf) else {
            continue;
        };
        if !parent.starts_with(&root.path) || listed.contains(&parent) {
            continue;
        }
        let start = context
            .database
            .read(|connection| resolve_start(connection, root, exclusions, &parent))
            .map_err(std::io::Error::other)?;
        let Some(start) = start else {
            continue;
        };
        let more = walk(context.probe.as_ref(), exclusions, vec![start], &descend)?;
        queue.extend(more.missing_starts);
        result.unreadable_directories += more.unreadable_directories;
        let offset = result.dirs.len();
        for mut dir in more.dirs {
            if let WalkParent::Walked(index) = dir.parent {
                dir.parent = WalkParent::Walked(index + offset);
            }
            listed.insert(dir.path.clone());
            result.dirs.push(dir);
        }
        listed.insert(parent);
    }
    Ok(result)
}

/// Whether a subdirectory is exactly where the library says, in which case listing its parent says
/// all there is to say about it. Without File IDs there is no telling, so the answer is no.
fn is_settled(database: &Database, volume: u64, parent_id: Option<FileId>, entry: &Entry) -> bool {
    let (Some(file_id), Some(parent_id)) = (entry.file_id, parent_id) else {
        return false;
    };
    database
        .read(|connection| {
            Ok(connection
                .prepare_cached(
                    "SELECT 1 FROM folder f JOIN folder p ON p.id = f.parent_id
                     WHERE f.volume_serial = ?1 AND f.file_id = ?2 AND f.name = ?3
                       AND f.state = 'present'
                       AND p.volume_serial = ?1 AND p.file_id = ?4",
                )?
                .query_row(
                    params![
                        volume_to_sql(volume),
                        file_id.0.as_slice(),
                        name_to_sql(&entry.name),
                        parent_id.0.as_slice(),
                    ],
                    |_| Ok(()),
                )
                .optional()?
                .is_some())
        })
        .unwrap_or(false)
}

/// Where a walk of `directory` has to start: the directory itself if the library knows the way
/// there, or the deepest directory on the way that it does know.
fn resolve_start(
    connection: &Connection,
    root: &RootInfo,
    exclusions: &Exclusions,
    directory: &Path,
) -> Result<Option<WalkStart>> {
    let Ok(relative) = directory.strip_prefix(&root.path) else {
        return Ok(None);
    };
    if relative
        .iter()
        .any(|component| exclusions.excludes(component))
    {
        return Ok(None);
    }
    let top: Option<FolderId> = connection
        .prepare_cached("SELECT id FROM folder WHERE root_id = ?1 AND parent_id IS NULL")?
        .query_row([root.id], |row| row.get(0))
        .optional()?;
    let Some(top) = top else {
        // It has never been scanned.
        return Ok(Some(WalkStart {
            path: root.path.clone(),
            parent: WalkParent::Top,
            name: OsString::new(),
        }));
    };

    let mut current = WalkStart {
        path: root.path.clone(),
        parent: WalkParent::Top,
        name: OsString::new(),
    };
    let mut current_id = top;
    for component in relative {
        let child: Option<FolderId> = connection
            .prepare_cached(
                "SELECT id FROM folder WHERE parent_id = ?1 AND name = ?2 AND state = 'present'",
            )?
            .query_row(params![current_id, name_to_sql(component)], |row| {
                row.get(0)
            })
            .optional()?;
        let Some(child) = child else {
            break;
        };
        current = WalkStart {
            path: current.path.join(component),
            parent: WalkParent::Known(current_id),
            name: component.to_owned(),
        };
        current_id = child;
    }
    Ok(Some(current))
}

fn observe_pending(
    context: &Context<'_>,
    pending: Vec<Pending>,
    report: &mut ReconcileReport,
) -> Result<()> {
    if pending.is_empty() {
        return Ok(());
    }
    let queue = Mutex::new(pending.into_iter());
    let (sender, receiver) = mpsc::sync_channel::<Observation>(OBSERVATION_BATCH * 2);

    thread::scope(|scope| {
        for _ in 0..context.threads.max(1) {
            let sender = sender.clone();
            let queue = &queue;
            scope.spawn(move || {
                let _background = context.low_priority.then(enter_background);
                loop {
                    if context.stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let next = queue
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .next();
                    let Some(pending) = next else {
                        break;
                    };
                    let observation = observe(context.probe.as_ref(), context.counters, pending);
                    if sender.send(observation).is_err() {
                        break;
                    }
                }
            });
        }
        drop(sender);

        let mut batch = Vec::with_capacity(OBSERVATION_BATCH);
        for observation in receiver {
            batch.push(observation);
            if batch.len() == OBSERVATION_BATCH {
                apply_batch(context, std::mem::take(&mut batch), report)?;
                batch.reserve(OBSERVATION_BATCH);
            }
        }
        apply_batch(context, batch, report)
    })
}

fn apply_batch(
    context: &Context<'_>,
    batch: Vec<Observation>,
    report: &mut ReconcileReport,
) -> Result<()> {
    if batch.is_empty() {
        return Ok(());
    }
    let mut owned = std::mem::take(report);
    let updated = context.database.write(move |transaction| {
        apply_observations(transaction, batch, now_ms(), &mut owned)?;
        Ok(owned)
    })?;
    *report = updated;
    Ok(())
}

/// Reads as little as possible of a file. A file in the cloud is not even opened.
fn observe(probe: &dyn FsProbe, counters: &ReadCounters, pending: Pending) -> Observation {
    if pending.entry.is_cloud_only() {
        return Observation {
            pending,
            outcome: ObservedState::CloudOnly,
        };
    }
    let outcome = match probe.open_content(&pending.path, ReadPattern::Samples) {
        Ok(content) => {
            counters.files_opened.fetch_add(1, Ordering::Relaxed);
            match fast_fingerprint(&content.file, content.stat.size, counters) {
                Ok(fingerprint) => ObservedState::Read {
                    volume: content.stat.volume,
                    file_id: content.stat.file_id,
                    size: content.stat.size,
                    mtime_ns: content.stat.mtime_ns,
                    fingerprint,
                },
                Err(_) => ObservedState::Unreadable,
            }
        }
        Err(OpenError::CloudOnly) => ObservedState::CloudOnly,
        Err(OpenError::Missing) => ObservedState::Gone,
        Err(OpenError::Unreadable(_)) => ObservedState::Unreadable,
    };
    Observation { pending, outcome }
}

/// A root's row.
pub(crate) fn load_root(connection: &Connection, root_id: RootId) -> Result<RootInfo> {
    list_roots(connection)?
        .into_iter()
        .find(|root| root.id == root_id)
        .ok_or_else(|| Error::NotFound(format!("there is no root {root_id}")))
}

/// All the roots still in use.
pub(crate) fn list_roots(connection: &Connection) -> Result<Vec<RootInfo>> {
    let mut statement = connection.prepare_cached(
        "SELECT id, path, policy, exclusions, status, last_reconciled_at
         FROM library_root WHERE status <> 'removed' ORDER BY id",
    )?;
    let rows = statement.query_map([], |row| {
        let exclusions: String = row.get(3)?;
        Ok(RootInfo {
            id: row.get(0)?,
            path: PathBuf::from(row.get::<_, StoredName>(1)?.0),
            policy: row.get(2)?,
            exclusions: serde_json::from_str(&exclusions).unwrap_or_default(),
            status: row.get(4)?,
            last_reconciled_at: row.get(5)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

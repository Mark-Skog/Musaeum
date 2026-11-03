//! Recognizes files: the five steps of the plan, in the plan's order.
//!
//! 1. **Same File ID, size and date.** A move or a rename. The location is updated, and the file is
//!    not read. This runs before any other step, across everything a scan saw, so a file that moved
//!    is out of the way by the time something new at its old path gets looked at.
//! 2. **Same File ID and path, different contents.** A new revision of the same document.
//! 3. **An unknown File ID, or a known one with a different path and contents.** The fast fingerprint
//!    names candidates among what went missing. Only the full hash confirms one, in the background.
//! 4. **New bytes with a known `/ID[0]` or `dc:identifier`.** A suggestion, never a merge.
//! 5. **Otherwise.** A new document.
//!
//! There is one addition, because the plan's own battery requires it. A safe save leaves a different
//! file at the same path: by those five steps alone an edited book would become a new document.
//! Between 2 and 3 there is therefore this: **a different File ID at a path whose file was there
//! until now, and is not anywhere else** becomes a new revision of the same document. A file that had
//! already gone missing does not get this: something at its old path later on is only a suggestion.
//!
//! That link is provisional, because nothing about the bytes is known yet, and `replaced_file` keeps
//! what undoing it needs. It stands only if the new bytes are the same kind of file as the old ones and
//! do not name a different `/ID[0]` or `dc:identifier` than the document has. Otherwise the instance
//! goes back to being the old file, now missing, and the new one becomes a document of its own with a
//! suggestion, which is what it would have been without the shortcut. [`crate::content`] decides,
//! when the hash and what the file says about itself arrive.
//!
//! Steps 1 and 2 are decided here from what the directories say. Steps 3 to 5 need the file's bytes,
//! and finish in [`crate::content`], when the full hash arrives.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use musaeum_fs::{Entry, FileId};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Serialize;

use crate::error::Result;
use crate::fingerprint::Hash;
use crate::model::{
    DocumentId, FolderId, InstanceId, InstanceState, RevisionId, RevisionOrigin, RootId,
    SuggestionKind, WorkId,
};
use crate::probe::FsProbe;
use crate::scan::{Walk, WalkParent, WalkedDir, format_of, layout_of};
use crate::sql::{
    StoredName, file_id_from_sql, file_id_to_sql, name_to_sql, volume_from_sql, volume_to_sql,
};

/// What a scan found and did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ReconcileReport {
    /// The root this is about.
    pub root_id: RootId,
    /// Whether the root could be read at all. If not, nothing else here means anything.
    pub root_available: bool,
    /// Directories listed.
    pub directories_listed: u64,
    /// Directories that exist and could not be listed, whose contents were left as they were.
    pub directories_unreadable: u64,
    /// Files in the directories listed.
    pub files_seen: u64,
    /// Files exactly as the library knew them.
    pub unchanged: u64,
    /// Step 1: files that moved, recognized without reading them.
    pub moved: u64,
    /// Step 1: files that had gone missing and came back the same.
    pub restored: u64,
    /// Step 1: other names for a file already known, which share its File ID.
    pub hard_links: u64,
    /// Step 2: files with new contents.
    pub changed_in_place: u64,
    /// Files replaced at their own path, as by a safe save.
    pub replaced_at_path: u64,
    /// Files the library had no record of, for now each a document of its own.
    pub new_files: u64,
    /// Of those, how many the fast fingerprint found a candidate for.
    pub candidates_named: u64,
    /// Files that are no longer where they were.
    pub became_absent: u64,
    /// Files whose bytes left for the cloud.
    pub became_cloud_only: u64,
    /// Files whose bytes came back from the cloud.
    pub became_local: u64,
    /// Files that could not be opened.
    pub unreadable: u64,
    /// Folders that moved or changed name.
    pub folders_moved: u64,
    /// Folders the library had no record of.
    pub folders_new: u64,
    /// Folders that are no longer where they were.
    pub folders_absent: u64,
    /// Files opened to read their bytes.
    pub files_opened: u64,
    /// Bytes read from those files.
    pub bytes_read: u64,
    /// How long it took.
    pub duration_ms: u64,
}

/// A file a scan could not settle from the directory alone.
#[derive(Debug)]
pub(crate) struct Pending {
    pub folder_id: FolderId,
    pub path: PathBuf,
    pub volume: u64,
    pub entry: Entry,
    pub kind: PendingKind,
}

/// What is known about a file that needs its bytes looked at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PendingKind {
    /// Step 2: same File ID, at the path of this instance.
    InPlace(InstanceId),
    /// A different File ID, at the path of this instance, which was present until now.
    AtPath(InstanceId),
    /// Nothing to go by. If a file with this File ID exists elsewhere, this is its document.
    New {
        same_file_id_document: Option<DocumentId>,
    },
}

/// What looking at a pending file's bytes gave.
#[derive(Debug)]
pub(crate) struct Observation {
    pub pending: Pending,
    pub outcome: ObservedState,
}

#[derive(Debug)]
pub(crate) enum ObservedState {
    /// It was opened, and these are its real values.
    Read {
        volume: u64,
        file_id: Option<FileId>,
        size: u64,
        mtime_ns: i64,
        fingerprint: Hash,
    },
    /// Its bytes are in the cloud, so only the directory's word is available.
    CloudOnly,
    /// It could not be opened, so only the directory's word is available.
    Unreadable,
    /// It is not there any more.
    Gone,
}

#[derive(Clone, Debug)]
struct KnownInstance {
    id: InstanceId,
    name: OsString,
    volume: Option<u64>,
    file_id: Option<FileId>,
    mtime_ns: i64,
    state: InstanceState,
    size: u64,
}

impl KnownInstance {
    fn is_same_file(&self, volume: u64, entry: &Entry) -> bool {
        self.volume == Some(volume)
            && self.file_id == entry.file_id
            && self.size == entry.size
            && self.mtime_ns == entry.mtime_ns
    }
}

/// A file whose directory entry does not match anything at its own path.
#[derive(Debug)]
struct Unsettled<'a> {
    folder_id: FolderId,
    dir: &'a WalkedDir,
    entry: &'a Entry,
    /// What is at this path, and is not this file.
    occupant: Option<KnownInstance>,
    /// An absent instance with this file's ID that used to be at this path.
    absent_same_id: Option<InstanceId>,
}

/// Applies what a walk saw: folders, then step 1 for every file, then what went missing. All of it
/// in one transaction, so nobody ever sees a file gone that is merely on its way somewhere else.
/// Returns the files that need their bytes looked at.
pub(crate) fn apply_walk(
    transaction: &Transaction<'_>,
    root_id: RootId,
    walk: &Walk,
    probe: &dyn FsProbe,
    now: i64,
    report: &mut ReconcileReport,
) -> Result<Vec<Pending>> {
    let folder_ids = apply_folders(transaction, root_id, walk, now, report)?;

    let mut unchanged = HashSet::new();
    let mut unsettled = Vec::new();
    let mut vanished: HashMap<InstanceId, KnownInstance> = HashMap::new();
    let walked_folders: HashSet<FolderId> = folder_ids.iter().copied().collect();

    for (dir, &folder_id) in walk.dirs.iter().zip(&folder_ids) {
        match_directory(
            transaction,
            dir,
            folder_id,
            now,
            report,
            &mut unchanged,
            &mut unsettled,
            &mut vanished,
        )?;
    }

    // Step 1, for everything, before anything concludes that something is gone.
    let mut relocated = HashSet::new();
    let mut displaced: Vec<(InstanceId, InstanceId)> = Vec::new();
    let mut remaining = Vec::with_capacity(unsettled.len());
    for item in unsettled {
        let settled = try_step_one(
            transaction,
            probe,
            &item,
            &walked_folders,
            &unchanged,
            &mut relocated,
            &mut vanished,
            &mut displaced,
            now,
            report,
        )?;
        if !settled {
            remaining.push(item);
        }
    }

    for instance in vanished.values() {
        set_state(transaction, instance.id, InstanceState::Absent, now)?;
        report.became_absent += 1;
    }
    suggest_for_displaced(transaction, &displaced, now)?;

    let mut pending = Vec::with_capacity(remaining.len());
    for item in remaining {
        pending.push(classify(transaction, item, &relocated)?);
    }
    Ok(pending)
}

fn apply_folders(
    transaction: &Transaction<'_>,
    root_id: RootId,
    walk: &Walk,
    now: i64,
    report: &mut ReconcileReport,
) -> Result<Vec<FolderId>> {
    let mut ids: Vec<FolderId> = Vec::with_capacity(walk.dirs.len());
    for dir in &walk.dirs {
        let id = match dir.parent {
            WalkParent::Top => upsert_top_folder(transaction, root_id, dir)?,
            WalkParent::Walked(index) => {
                upsert_folder(transaction, root_id, ids[index], dir, report)?
            }
            WalkParent::Known(parent) => upsert_folder(transaction, root_id, parent, dir, report)?,
        };
        ids.push(id);
    }

    // After all of the above, since a folder that moved has its new parent by now.
    for (dir, &id) in walk.dirs.iter().zip(&ids) {
        let children = {
            let mut statement = transaction.prepare_cached(
                "SELECT id, name FROM folder WHERE parent_id = ?1 AND state = 'present'",
            )?;
            statement
                .query_map([id], |row| {
                    Ok((row.get::<_, FolderId>(0)?, row.get::<_, StoredName>(1)?.0))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (child, name) in children {
            if !dir.subdirectories.contains(&name) {
                mark_subtree_absent(transaction, child, now, report)?;
            }
        }
    }
    Ok(ids)
}

fn upsert_top_folder(
    transaction: &Transaction<'_>,
    root_id: RootId,
    dir: &WalkedDir,
) -> Result<FolderId> {
    let existing: Option<FolderId> = transaction
        .prepare_cached("SELECT id FROM folder WHERE root_id = ?1 AND parent_id IS NULL")?
        .query_row([root_id], |row| row.get(0))
        .optional()?;
    let file_id = file_id_to_sql(dir.directory_id);
    let volume = volume_to_sql(dir.volume);
    if let Some(id) = existing {
        transaction
            .prepare_cached(
                "UPDATE folder SET volume_serial = ?2, file_id = ?3, state = 'present'
                 WHERE id = ?1 AND (volume_serial IS NOT ?2 OR file_id IS NOT ?3 OR state <> 'present')",
            )?
            .execute(params![id, volume, file_id])?;
        return Ok(id);
    }
    transaction
        .prepare_cached(
            "INSERT INTO folder (root_id, parent_id, name, volume_serial, file_id, state)
             VALUES (?1, NULL, '', ?2, ?3, 'present')",
        )?
        .execute(params![root_id, volume, file_id])?;
    Ok(transaction.last_insert_rowid())
}

fn upsert_folder(
    transaction: &Transaction<'_>,
    root_id: RootId,
    parent_id: FolderId,
    dir: &WalkedDir,
    report: &mut ReconcileReport,
) -> Result<FolderId> {
    let name = name_to_sql(&dir.name);
    let volume = volume_to_sql(dir.volume);
    let file_id = file_id_to_sql(dir.directory_id);

    // Known by its ID: however far it moved, this is one row. The root's own folder is not
    // something that moves into another: its row belongs to its root.
    if file_id.is_some() {
        let known = transaction
            .prepare_cached(
                "SELECT id, root_id, parent_id, name, state FROM folder
                 WHERE volume_serial = ?1 AND file_id = ?2 AND parent_id IS NOT NULL
                 ORDER BY state = 'present' DESC, id DESC LIMIT 1",
            )?
            .query_row(params![volume, file_id], |row| {
                Ok((
                    row.get::<_, FolderId>(0)?,
                    row.get::<_, RootId>(1)?,
                    row.get::<_, FolderId>(2)?,
                    row.get::<_, StoredName>(3)?.0,
                    row.get::<_, String>(4)?,
                ))
            })
            .optional()?;
        if let Some((id, old_root, old_parent, old_name, state)) = known {
            let moved = old_parent != parent_id || old_name != dir.name;
            if moved || state != "present" || old_root != root_id {
                vacate_folder_location(transaction, parent_id, &dir.name, id)?;
                transaction
                    .prepare_cached(
                        "UPDATE folder SET root_id = ?2, parent_id = ?3, name = ?4, state = 'present'
                         WHERE id = ?1",
                    )?
                    .execute(params![id, root_id, parent_id, name])?;
                if moved {
                    report.folders_moved += 1;
                }
                if old_root != root_id {
                    move_subtree_to_root(transaction, id, root_id)?;
                }
            }
            return Ok(id);
        }
    }

    // Known by where it is: the same folder recreated, or a volume without lasting IDs.
    let at_location: Option<FolderId> = transaction
        .prepare_cached(
            "SELECT id FROM folder WHERE parent_id = ?1 AND name = ?2
             ORDER BY state = 'present' DESC, id DESC LIMIT 1",
        )?
        .query_row(params![parent_id, name], |row| row.get(0))
        .optional()?;
    if let Some(id) = at_location {
        vacate_folder_location(transaction, parent_id, &dir.name, id)?;
        transaction
            .prepare_cached(
                "UPDATE folder SET volume_serial = ?2, file_id = ?3, state = 'present' WHERE id = ?1",
            )?
            .execute(params![id, volume, file_id])?;
        return Ok(id);
    }

    vacate_folder_location(transaction, parent_id, &dir.name, -1)?;
    transaction
        .prepare_cached(
            "INSERT INTO folder (root_id, parent_id, name, volume_serial, file_id, state)
             VALUES (?1, ?2, ?3, ?4, ?5, 'present')",
        )?
        .execute(params![root_id, parent_id, name, volume, file_id])?;
    report.folders_new += 1;
    Ok(transaction.last_insert_rowid())
}

/// If something else is recorded at this location, it is not there. If it is somewhere else, its
/// own turn in this scan will find it.
fn vacate_folder_location(
    transaction: &Transaction<'_>,
    parent_id: FolderId,
    name: &OsString,
    except: FolderId,
) -> Result<()> {
    transaction
        .prepare_cached(
            "UPDATE folder SET state = 'absent'
             WHERE parent_id = ?1 AND name = ?2 AND state = 'present' AND id <> ?3",
        )?
        .execute(params![parent_id, name_to_sql(name), except])?;
    Ok(())
}

fn move_subtree_to_root(
    transaction: &Transaction<'_>,
    folder_id: FolderId,
    root_id: RootId,
) -> Result<()> {
    transaction
        .prepare_cached(
            "WITH RECURSIVE subtree(id) AS (
                 SELECT id FROM folder WHERE parent_id = ?1
                 UNION ALL
                 SELECT folder.id FROM folder JOIN subtree ON folder.parent_id = subtree.id
             )
             UPDATE folder SET root_id = ?2 WHERE id IN subtree",
        )?
        .execute(params![folder_id, root_id])?;
    Ok(())
}

/// A folder that is gone, and what was in it, which also is not there. Nothing is deleted.
pub(crate) fn mark_subtree_absent(
    transaction: &Transaction<'_>,
    folder_id: FolderId,
    now: i64,
    report: &mut ReconcileReport,
) -> Result<()> {
    let folders = transaction
        .prepare_cached(
            "WITH RECURSIVE subtree(id) AS (
                 VALUES (?1)
                 UNION ALL
                 SELECT folder.id FROM folder JOIN subtree ON folder.parent_id = subtree.id
             )
             UPDATE folder SET state = 'absent' WHERE id IN subtree AND state = 'present'",
        )?
        .execute([folder_id])?;
    report.folders_absent += folders as u64;
    let files = transaction
        .prepare_cached(
            "WITH RECURSIVE subtree(id) AS (
                 VALUES (?1)
                 UNION ALL
                 SELECT folder.id FROM folder JOIN subtree ON folder.parent_id = subtree.id
             )
             UPDATE file_instance SET state = 'absent', state_since = ?2
             WHERE folder_id IN subtree AND state <> 'absent'",
        )?
        .execute(params![folder_id, now])?;
    report.became_absent += files as u64;
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "the state of one pass over a walk"
)]
fn match_directory<'a>(
    transaction: &Transaction<'_>,
    dir: &'a WalkedDir,
    folder_id: FolderId,
    now: i64,
    report: &mut ReconcileReport,
    unchanged: &mut HashSet<InstanceId>,
    unsettled: &mut Vec<Unsettled<'a>>,
    vanished: &mut HashMap<InstanceId, KnownInstance>,
) -> Result<()> {
    report.directories_listed += 1;
    report.files_seen += dir.files.len() as u64;

    let mut known: HashMap<OsString, Vec<KnownInstance>> = HashMap::new();
    for instance in instances_in_folder(transaction, folder_id)? {
        known
            .entry(instance.name.clone())
            .or_default()
            .push(instance);
    }

    for entry in &dir.files {
        let at_this_name = known.remove(&entry.name).unwrap_or_default();
        let live = at_this_name
            .iter()
            .find(|instance| instance.state != InstanceState::Absent);

        match live {
            Some(instance) if instance.is_same_file(dir.volume, entry) => {
                unchanged.insert(instance.id);
                report.unchanged += 1;
                sync_cloud_state(transaction, instance, entry, now, report)?;
            }
            Some(instance) => unsettled.push(Unsettled {
                folder_id,
                dir,
                entry,
                occupant: Some(instance.clone()),
                absent_same_id: None,
            }),
            None => {
                let returned = at_this_name
                    .iter()
                    .find(|instance| instance.is_same_file(dir.volume, entry));
                if let Some(instance) = returned {
                    // It went missing and it is back, in the same place, exactly the same.
                    let state = local_state_of(entry);
                    set_state(transaction, instance.id, state, now)?;
                    unchanged.insert(instance.id);
                    report.restored += 1;
                } else {
                    let absent_same_id = entry.file_id.and_then(|file_id| {
                        at_this_name
                            .iter()
                            .find(|instance| {
                                instance.volume == Some(dir.volume)
                                    && instance.file_id == Some(file_id)
                            })
                            .map(|instance| instance.id)
                    });
                    unsettled.push(Unsettled {
                        folder_id,
                        dir,
                        entry,
                        occupant: None,
                        absent_same_id,
                    });
                }
            }
        }
    }

    // What is left was here and is not any more, unless step 1 finds it somewhere else.
    for instances in known.into_values() {
        for instance in instances {
            if instance.state != InstanceState::Absent {
                vanished.insert(instance.id, instance);
            }
        }
    }
    Ok(())
}

fn instances_in_folder(connection: &Connection, folder_id: FolderId) -> Result<Vec<KnownInstance>> {
    let mut statement = connection.prepare_cached(
        "SELECT i.id, i.name, i.volume_serial, i.file_id, i.mtime_ns, i.state, r.size
         FROM file_instance i JOIN revision r ON r.id = i.revision_id
         WHERE i.folder_id = ?1",
    )?;
    let rows = statement.query_map([folder_id], |row| {
        Ok(KnownInstance {
            id: row.get(0)?,
            name: row.get::<_, StoredName>(1)?.0,
            volume: row.get::<_, Option<i64>>(2)?.map(volume_from_sql),
            file_id: file_id_from_sql(row.get(3)?),
            mtime_ns: row.get(4)?,
            state: row.get(5)?,
            size: row.get::<_, i64>(6)? as u64,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn local_state_of(entry: &Entry) -> InstanceState {
    if entry.is_cloud_only() {
        InstanceState::CloudOnly
    } else {
        InstanceState::Present
    }
}

fn set_state(
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

/// Nothing about a file changes when its bytes leave for the cloud or come back: only this.
fn sync_cloud_state(
    transaction: &Transaction<'_>,
    instance: &KnownInstance,
    entry: &Entry,
    now: i64,
    report: &mut ReconcileReport,
) -> Result<()> {
    let cloud_only = entry.is_cloud_only();
    if cloud_only && instance.state != InstanceState::CloudOnly {
        set_state(transaction, instance.id, InstanceState::CloudOnly, now)?;
        report.became_cloud_only += 1;
    } else if !cloud_only && instance.state == InstanceState::CloudOnly {
        set_state(transaction, instance.id, InstanceState::Present, now)?;
        report.became_local += 1;
    }
    Ok(())
}

/// Step 1. Returns whether this settled the file.
#[expect(
    clippy::too_many_arguments,
    reason = "the state of one pass over a walk"
)]
fn try_step_one(
    transaction: &Transaction<'_>,
    probe: &dyn FsProbe,
    item: &Unsettled<'_>,
    walked_folders: &HashSet<FolderId>,
    unchanged: &HashSet<InstanceId>,
    relocated: &mut HashSet<InstanceId>,
    vanished: &mut HashMap<InstanceId, KnownInstance>,
    displaced: &mut Vec<(InstanceId, InstanceId)>,
    now: i64,
    report: &mut ReconcileReport,
) -> Result<bool> {
    let Some(file_id) = item.entry.file_id else {
        return Ok(false);
    };
    let matches = same_file_candidates(transaction, item.dir.volume, file_id, item.entry)?;

    let mut mover = None;
    let mut linked_revision = None;
    for candidate in &matches {
        if unchanged.contains(&candidate.id) || relocated.contains(&candidate.id) {
            // It is still where it was, or already went elsewhere in this scan: this is another name
            // for the same file.
            linked_revision = Some(candidate.revision_id);
        } else if candidate.state == InstanceState::Absent
            || walked_folders.contains(&candidate.folder_id)
        {
            mover = Some(candidate);
            break;
        } else if still_there(transaction, probe, candidate, file_id)? {
            linked_revision = Some(candidate.revision_id);
        } else {
            mover = Some(candidate);
            break;
        }
    }

    if let Some(candidate) = mover {
        if let Some(occupant) = vacate_location(transaction, item, candidate.id, now)? {
            displaced.push((candidate.id, occupant));
        }
        relocate(transaction, candidate.id, item, now)?;
        relocated.insert(candidate.id);
        vanished.remove(&candidate.id);
        // Absent only because another file took its place a moment ago is not having gone missing.
        let had_gone_missing = candidate.state == InstanceState::Absent
            && !displaced
                .iter()
                .any(|(_, instance)| *instance == candidate.id);
        if had_gone_missing {
            report.restored += 1;
        } else {
            report.moved += 1;
        }
        return Ok(true);
    }
    if let Some(revision) = linked_revision {
        vacate_location(transaction, item, -1, now)?;
        insert_instance(
            transaction,
            revision,
            item.folder_id,
            &item.entry.name,
            Some(item.dir.volume),
            item.entry.file_id,
            item.entry.mtime_ns,
            local_state_of(item.entry),
            now,
        )?;
        report.hard_links += 1;
        return Ok(true);
    }
    Ok(false)
}

#[derive(Debug)]
struct SameFile {
    id: InstanceId,
    revision_id: RevisionId,
    folder_id: FolderId,
    name: OsString,
    state: InstanceState,
}

/// Everything recorded with this File ID, size and date. The ones that went missing come first,
/// since finding one of those means nothing else needs checking.
fn same_file_candidates(
    connection: &Connection,
    volume: u64,
    file_id: FileId,
    entry: &Entry,
) -> Result<Vec<SameFile>> {
    let mut statement = connection.prepare_cached(
        "SELECT i.id, i.revision_id, i.folder_id, i.name, i.state
         FROM file_instance i JOIN revision r ON r.id = i.revision_id
         WHERE i.volume_serial = ?1 AND i.file_id = ?2 AND i.mtime_ns = ?3 AND r.size = ?4
         ORDER BY i.state = 'absent' DESC, i.id",
    )?;
    let rows = statement.query_map(
        params![
            volume_to_sql(volume),
            file_id.0.as_slice(),
            entry.mtime_ns,
            entry.size as i64
        ],
        |row| {
            Ok(SameFile {
                id: row.get(0)?,
                revision_id: row.get(1)?,
                folder_id: row.get(2)?,
                name: row.get::<_, StoredName>(3)?.0,
                state: row.get(4)?,
            })
        },
    )?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// For a folder this scan did not list: whether a recorded file is still at its recorded path. If
/// it is, this new name is a hard link to it. Otherwise it moved.
fn still_there(
    connection: &Connection,
    probe: &dyn FsProbe,
    candidate: &SameFile,
    file_id: FileId,
) -> Result<bool> {
    let path = folder_path(connection, candidate.folder_id)?.join(&candidate.name);
    Ok(probe
        .stat(&path)
        .is_ok_and(|stat| stat.file_id == Some(file_id)))
}

/// Something else recorded at this file's path is not there. If it is somewhere else, its own turn
/// in step 1 will find it. Returns that instance.
fn vacate_location(
    transaction: &Transaction<'_>,
    item: &Unsettled<'_>,
    except: InstanceId,
    now: i64,
) -> Result<Option<InstanceId>> {
    let occupant = transaction
        .prepare_cached(
            "SELECT id FROM file_instance
             WHERE folder_id = ?1 AND name = ?2 AND state <> 'absent' AND id <> ?3",
        )?
        .query_row(
            params![item.folder_id, name_to_sql(&item.entry.name), except],
            |row| row.get::<_, InstanceId>(0),
        )
        .optional()?;
    if let Some(occupant) = occupant {
        set_state(transaction, occupant, InstanceState::Absent, now)?;
    }
    Ok(occupant)
}

fn relocate(
    transaction: &Transaction<'_>,
    instance: InstanceId,
    item: &Unsettled<'_>,
    now: i64,
) -> Result<()> {
    transaction
        .prepare_cached(
            "UPDATE file_instance
             SET folder_id = ?2, name = ?3, volume_serial = ?4, file_id = ?5, mtime_ns = ?6,
                 state = ?7, state_since = ?8
             WHERE id = ?1",
        )?
        .execute(params![
            instance,
            item.folder_id,
            name_to_sql(&item.entry.name),
            volume_to_sql(item.dir.volume),
            file_id_to_sql(item.entry.file_id),
            item.entry.mtime_ns,
            local_state_of(item.entry),
            now,
        ])?;
    Ok(())
}

/// A file that moved onto a path where another file used to be, and that other file is not to be
/// found. Nothing proves they are related, so all this does is say so.
fn suggest_for_displaced(
    transaction: &Transaction<'_>,
    displaced: &[(InstanceId, InstanceId)],
    now: i64,
) -> Result<()> {
    for &(arrived, displaced) in displaced {
        let state: InstanceState = transaction
            .prepare_cached("SELECT state FROM file_instance WHERE id = ?1")?
            .query_row([displaced], |row| row.get(0))?;
        if state != InstanceState::Absent {
            continue;
        }
        let arrived_document = document_of_instance(transaction, arrived)?;
        let displaced_document = document_of_instance(transaction, displaced)?;
        suggest(
            transaction,
            SuggestionKind::SamePath,
            arrived_document,
            displaced_document,
            "moved onto the path of a file that is now missing",
            now,
        )?;
    }
    Ok(())
}

fn document_of_instance(connection: &Connection, instance: InstanceId) -> Result<DocumentId> {
    Ok(connection
        .prepare_cached(
            "SELECT r.document_id FROM file_instance i JOIN revision r ON r.id = i.revision_id
             WHERE i.id = ?1",
        )?
        .query_row([instance], |row| row.get(0))?)
}

fn classify(
    connection: &Connection,
    item: Unsettled<'_>,
    relocated: &HashSet<InstanceId>,
) -> Result<Pending> {
    let occupant = item
        .occupant
        .as_ref()
        .filter(|occupant| !relocated.contains(&occupant.id));
    let kind = match occupant {
        Some(occupant)
            if occupant.file_id == item.entry.file_id
                && occupant.volume == Some(item.dir.volume) =>
        {
            PendingKind::InPlace(occupant.id)
        }
        Some(occupant) => PendingKind::AtPath(occupant.id),
        None => match item.absent_same_id {
            Some(absent) => PendingKind::InPlace(absent),
            None => PendingKind::New {
                same_file_id_document: document_with_file_id(
                    connection,
                    item.dir.volume,
                    item.entry.file_id,
                )?,
            },
        },
    };
    Ok(Pending {
        folder_id: item.folder_id,
        path: item.dir.path.join(&item.entry.name),
        volume: item.dir.volume,
        entry: item.entry.clone(),
        kind,
    })
}

fn document_with_file_id(
    connection: &Connection,
    volume: u64,
    file_id: Option<FileId>,
) -> Result<Option<DocumentId>> {
    let Some(file_id) = file_id else {
        return Ok(None);
    };
    Ok(connection
        .prepare_cached(
            "SELECT r.document_id FROM file_instance i JOIN revision r ON r.id = i.revision_id
             WHERE i.volume_serial = ?1 AND i.file_id = ?2 ORDER BY i.id DESC LIMIT 1",
        )?
        .query_row(
            params![volume_to_sql(volume), file_id.0.as_slice()],
            |row| row.get(0),
        )
        .optional()?)
}

/// Records what looking at pending files gave: steps 2, 3 and 5, as far as they can go before the
/// full hash arrives.
pub(crate) fn apply_observations(
    transaction: &Transaction<'_>,
    observations: Vec<Observation>,
    now: i64,
    report: &mut ReconcileReport,
) -> Result<()> {
    for observation in observations {
        let Observation { pending, outcome } = observation;
        if matches!(outcome, ObservedState::Unreadable) {
            report.unreadable += 1;
        }
        match pending.kind {
            PendingKind::InPlace(instance) => {
                apply_change(
                    transaction,
                    instance,
                    &pending,
                    &outcome,
                    RevisionOrigin::ChangedInPlace,
                    now,
                    report,
                )?;
            }
            PendingKind::AtPath(instance) => {
                apply_change(
                    transaction,
                    instance,
                    &pending,
                    &outcome,
                    RevisionOrigin::ReplacedAtPath,
                    now,
                    report,
                )?;
            }
            PendingKind::New {
                same_file_id_document,
            } => {
                apply_new(
                    transaction,
                    &pending,
                    &outcome,
                    same_file_id_document,
                    now,
                    report,
                )?;
            }
        }
    }
    Ok(())
}

/// What an observation says about a file, with the directory's word for anything it could not get.
struct Facts {
    volume: u64,
    file_id: Option<FileId>,
    size: u64,
    mtime_ns: i64,
    fingerprint: Option<Hash>,
    state: InstanceState,
}

fn facts_of(pending: &Pending, outcome: &ObservedState) -> Option<Facts> {
    let from_directory = |state| Facts {
        volume: pending.volume,
        file_id: pending.entry.file_id,
        size: pending.entry.size,
        mtime_ns: pending.entry.mtime_ns,
        fingerprint: None,
        state,
    };
    match *outcome {
        ObservedState::Read {
            volume,
            file_id,
            size,
            mtime_ns,
            fingerprint,
        } => Some(Facts {
            volume,
            file_id,
            size,
            mtime_ns,
            fingerprint: Some(fingerprint),
            state: InstanceState::Present,
        }),
        ObservedState::CloudOnly => Some(from_directory(InstanceState::CloudOnly)),
        ObservedState::Unreadable => Some(from_directory(InstanceState::Unreadable)),
        ObservedState::Gone => None,
    }
}

/// The file at this instance's path is not what the instance says it is.
fn apply_change(
    transaction: &Transaction<'_>,
    instance: InstanceId,
    pending: &Pending,
    outcome: &ObservedState,
    origin: RevisionOrigin,
    now: i64,
    report: &mut ReconcileReport,
) -> Result<()> {
    let Some(facts) = facts_of(pending, outcome) else {
        set_state(transaction, instance, InstanceState::Absent, now)?;
        report.became_absent += 1;
        return Ok(());
    };
    let (revision, document, confirmed, stored_size, stored_mtime, stored_file_id, siblings): (
        RevisionId,
        DocumentId,
        bool,
        i64,
        i64,
        Option<Vec<u8>>,
        i64,
    ) = transaction
        .prepare_cached(
            "SELECT r.id, r.document_id, r.blake3 IS NOT NULL, r.size, i.mtime_ns, i.file_id,
                    (SELECT count(*) FROM file_instance o WHERE o.revision_id = r.id)
             FROM file_instance i JOIN revision r ON r.id = i.revision_id WHERE i.id = ?1",
        )?
        .query_row([instance], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
            ))
        })?;

    // A directory entry can be behind: if the file itself says nothing changed, nothing did.
    if stored_size as u64 == facts.size
        && stored_mtime == facts.mtime_ns
        && file_id_from_sql(stored_file_id) == facts.file_id
    {
        set_state(transaction, instance, facts.state, now)?;
        return Ok(());
    }

    match origin {
        RevisionOrigin::ChangedInPlace => report.changed_in_place += 1,
        RevisionOrigin::ReplacedAtPath => report.replaced_at_path += 1,
        RevisionOrigin::Import => {}
    }

    let revision_for_file = if !confirmed && siblings == 1 {
        // Nothing has been concluded about those bytes, and they are gone, so there is no history to
        // keep. This is also what happens while a large file is still being copied in.
        transaction
            .prepare_cached(
                "UPDATE revision SET fast_fingerprint = ?2, size = ?3, candidate_revision_id = NULL,
                     ingested_version = NULL
                 WHERE id = ?1",
            )?
            .execute(params![
                revision,
                facts.fingerprint.as_ref().map(<[u8; 32]>::as_slice),
                facts.size as i64,
            ])?;
        revision
    } else {
        let new = insert_revision(
            transaction,
            document,
            facts.fingerprint.as_ref(),
            facts.size,
            Some(revision),
            origin,
            now,
        )?;
        if origin == RevisionOrigin::ReplacedAtPath {
            // Provisional until the bytes are read, so what the instance was goes aside first.
            transaction
                .prepare_cached(
                    "INSERT INTO replaced_file
                         (revision_id, instance_id, folder_id, name, volume_serial, file_id, mtime_ns)
                     SELECT ?1, id, folder_id, name, volume_serial, file_id, mtime_ns
                     FROM file_instance WHERE id = ?2",
                )?
                .execute(params![new, instance])?;
        }
        new
    };
    transaction
        .prepare_cached(
            "UPDATE file_instance
             SET revision_id = ?2, volume_serial = ?3, file_id = ?4, mtime_ns = ?5, state = ?6,
                 state_since = ?7
             WHERE id = ?1",
        )?
        .execute(params![
            instance,
            revision_for_file,
            volume_to_sql(facts.volume),
            file_id_to_sql(facts.file_id),
            facts.mtime_ns,
            facts.state,
            now,
        ])?;
    Ok(())
}

/// Steps 3 and 5: for now a document of its own, with a candidate if the fingerprint names one.
/// Nothing is concluded from the candidate. It only moves this file up in line for its full hash.
fn apply_new(
    transaction: &Transaction<'_>,
    pending: &Pending,
    outcome: &ObservedState,
    same_file_id_document: Option<DocumentId>,
    now: i64,
    report: &mut ReconcileReport,
) -> Result<()> {
    let Some(facts) = facts_of(pending, outcome) else {
        return Ok(());
    };
    let (document, revision) = insert_document(
        transaction,
        &pending.entry.name,
        facts.fingerprint.as_ref(),
        facts.size,
        now,
    )?;
    transaction
        .prepare_cached(
            "UPDATE file_instance SET state = 'absent', state_since = ?3
             WHERE folder_id = ?1 AND name = ?2 AND state <> 'absent'",
        )?
        .execute(params![
            pending.folder_id,
            name_to_sql(&pending.entry.name),
            now
        ])?;
    insert_instance(
        transaction,
        revision,
        pending.folder_id,
        &pending.entry.name,
        Some(facts.volume),
        facts.file_id,
        facts.mtime_ns,
        facts.state,
        now,
    )?;
    report.new_files += 1;

    if let Some(fingerprint) = &facts.fingerprint {
        let candidate = candidate_among_absent(transaction, fingerprint, facts.size, revision)?;
        if let Some(candidate) = candidate {
            transaction
                .prepare_cached("UPDATE revision SET candidate_revision_id = ?2 WHERE id = ?1")?
                .execute(params![revision, candidate])?;
            report.candidates_named += 1;
        }
    }
    if let Some(other) = same_file_id_document.filter(|other| *other != document) {
        suggest(
            transaction,
            SuggestionKind::SameFileId,
            document,
            other,
            "the same File ID as a file in the library, with a different path and contents",
            now,
        )?;
    }
    Ok(())
}

/// A revision with this fingerprint that has gone missing from everywhere it was. One with a known
/// hash goes first, since that one can actually be confirmed.
fn candidate_among_absent(
    connection: &Connection,
    fingerprint: &Hash,
    size: u64,
    except: RevisionId,
) -> Result<Option<RevisionId>> {
    Ok(connection
        .prepare_cached(
            "SELECT r.id FROM revision r
             WHERE r.fast_fingerprint = ?1 AND r.size = ?2 AND r.id <> ?3
               AND EXISTS (SELECT 1 FROM file_instance i
                           WHERE i.revision_id = r.id AND i.state = 'absent')
             ORDER BY r.blake3 IS NULL, r.id LIMIT 1",
        )?
        .query_row(
            params![fingerprint.as_slice(), size as i64, except],
            |row| row.get(0),
        )
        .optional()?)
}

/// A work, a document and a first revision for a file that nothing is known about.
pub(crate) fn insert_document(
    transaction: &Transaction<'_>,
    name: &OsString,
    fingerprint: Option<&Hash>,
    size: u64,
    now: i64,
) -> Result<(DocumentId, RevisionId)> {
    let document = insert_work_and_document(transaction, name, now)?;
    let revision = insert_revision(
        transaction,
        document,
        fingerprint,
        size,
        None,
        RevisionOrigin::Import,
        now,
    )?;
    Ok((document, revision))
}

/// A work and a document for a file called `name`, with nothing in them yet but what the name says.
pub(crate) fn insert_work_and_document(
    transaction: &Transaction<'_>,
    name: &OsString,
    now: i64,
) -> Result<DocumentId> {
    let title = Path::new(name)
        .file_stem()
        .unwrap_or(name.as_os_str())
        .to_string_lossy()
        .into_owned();
    let format = format_of(name).unwrap_or_else(|| "unknown".to_owned());
    let layout = layout_of(&format).unwrap_or("flow");

    transaction
        .prepare_cached("INSERT INTO work (title, created_at, updated_at) VALUES (?1, ?2, ?2)")?
        .execute(params![title, now])?;
    let work: WorkId = transaction.last_insert_rowid();
    transaction
        .prepare_cached(
            "INSERT INTO field_provenance (work_id, field, origin, updated_at)
             VALUES (?1, 'title', 'filename', ?2)",
        )?
        .execute(params![work, now])?;
    transaction
        .prepare_cached(
            "INSERT INTO document (work_id, layout, format, created_at) VALUES (?1, ?2, ?3, ?4)",
        )?
        .execute(params![work, layout, format, now])?;
    Ok(transaction.last_insert_rowid())
}

fn insert_revision(
    transaction: &Transaction<'_>,
    document: DocumentId,
    fingerprint: Option<&Hash>,
    size: u64,
    previous: Option<RevisionId>,
    origin: RevisionOrigin,
    now: i64,
) -> Result<RevisionId> {
    transaction
        .prepare_cached(
            "INSERT INTO revision (document_id, fast_fingerprint, size, previous_revision_id, origin,
                                   created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?
        .execute(params![
            document,
            fingerprint.map(<[u8; 32]>::as_slice),
            size as i64,
            previous,
            origin.as_str(),
            now,
        ])?;
    Ok(transaction.last_insert_rowid())
}

#[expect(clippy::too_many_arguments, reason = "the columns of the row")]
fn insert_instance(
    transaction: &Transaction<'_>,
    revision: RevisionId,
    folder_id: FolderId,
    name: &OsString,
    volume: Option<u64>,
    file_id: Option<FileId>,
    mtime_ns: i64,
    state: InstanceState,
    now: i64,
) -> Result<InstanceId> {
    transaction
        .prepare_cached(
            "INSERT INTO file_instance (revision_id, folder_id, name, volume_serial, file_id, mtime_ns,
                                        state, state_since)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?
        .execute(params![
            revision,
            folder_id,
            name_to_sql(name),
            volume.map(volume_to_sql),
            file_id_to_sql(file_id),
            mtime_ns,
            state,
            now,
        ])?;
    Ok(transaction.last_insert_rowid())
}

/// Records something for a person to decide. Saying the same thing twice changes nothing.
pub(crate) fn suggest(
    transaction: &Transaction<'_>,
    kind: SuggestionKind,
    document: DocumentId,
    candidate: DocumentId,
    evidence: &str,
    now: i64,
) -> Result<bool> {
    let inserted = transaction
        .prepare_cached(
            "INSERT OR IGNORE INTO reconcile_suggestion
                 (kind, document_id, candidate_document_id, evidence, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )?
        .execute(params![kind.as_str(), document, candidate, evidence, now])?;
    Ok(inserted > 0)
}

/// Where a folder is on disk, by walking up to its root.
pub(crate) fn folder_path(connection: &Connection, folder_id: FolderId) -> Result<PathBuf> {
    let mut statement = connection.prepare_cached(
        "WITH RECURSIVE chain(id, parent_id, name, root_id, depth) AS (
             SELECT id, parent_id, name, root_id, 0 FROM folder WHERE id = ?1
             UNION ALL
             SELECT f.id, f.parent_id, f.name, f.root_id, chain.depth + 1
             FROM folder f JOIN chain ON f.id = chain.parent_id
         )
         SELECT name, root_id FROM chain ORDER BY depth DESC",
    )?;
    let names = statement
        .query_map([folder_id], |row| {
            Ok((row.get::<_, StoredName>(0)?.0, row.get::<_, RootId>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let root_id = names.first().map_or(0, |(_, root)| *root);
    let mut path = root_path(connection, root_id)?;
    for (name, _) in names.into_iter().skip(1) {
        path.push(name);
    }
    Ok(path)
}

pub(crate) fn root_path(connection: &Connection, root_id: RootId) -> Result<PathBuf> {
    let name: StoredName = connection
        .prepare_cached("SELECT path FROM library_root WHERE id = ?1")?
        .query_row([root_id], |row| row.get(0))?;
    Ok(PathBuf::from(name.0))
}

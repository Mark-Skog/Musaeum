//! Everything the library itself does to the disk, in a journal, with a way back.
//!
//! The disk only changes because someone asked for it. Each operation commits its intent first,
//! touches the disk, and then commits the outcome together with what it means for the library. A
//! crash can land between any two of those, so [`recover`] looks at what is really on the disk at
//! the next start, and finishes the entry one way or the other.
//!
//! Nothing here overwrites a file or deletes one. A move fails if the target exists, and deleting
//! goes to the system's Recycle Bin, and is refused where there is none.

use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::thread;

use musaeum_fs::{FileId, MoveError, has_recycle_bin, move_no_replace};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use trash::TrashItem;

use crate::db::Database;
use crate::error::{Error, Result};
use crate::identity::folder_path;
use crate::model::{FolderId, InstanceId, InstanceState, OperationId, OperationInfo, RootId};
use crate::probe::FsProbe;
use crate::scan::format_of;
use crate::sql::{StoredName, file_id_from_sql, name_to_sql, now_ms, volume_from_sql};

const KIND_MOVE_FILE: &str = "move_file";
const KIND_MOVE_FOLDER: &str = "move_folder";
const KIND_RECYCLE: &str = "recycle";
const KIND_RESTORE: &str = "restore";

/// The limit of what the shell can bring back out of the Recycle Bin. It takes longer paths in, and
/// then refuses to restore them, so what is past this does not go in.
const LONGEST_PATH_THE_RECYCLE_BIN_RESTORES: usize = 259;

/// What is needed to recognize a file later on, and to undo what happened to it.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
struct UndoData {
    /// The volume's serial number.
    volume: Option<u64>,
    /// The File ID, in hexadecimal.
    file_id: Option<String>,
    /// When the Recycle Bin says it got the file, in seconds since the Unix epoch.
    time_deleted: Option<i64>,
    /// The folder it was in before, for a move.
    source_folder: Option<FolderId>,
    /// The name it had before, for a move.
    source_name: Option<String>,
}

fn hex_id(file_id: FileId) -> String {
    use std::fmt::Write;
    file_id
        .0
        .iter()
        .fold(String::with_capacity(32), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
}

/// What is known about a file the journal is about to touch.
struct KnownFile {
    id: InstanceId,
    folder_id: FolderId,
    path: PathBuf,
    volume: Option<u64>,
    file_id: Option<FileId>,
    mtime_ns: i64,
    size: u64,
    state: InstanceState,
}

fn load_file(connection: &Connection, instance: InstanceId) -> Result<KnownFile> {
    let row = connection
        .prepare_cached(
            "SELECT i.folder_id, i.name, i.volume_serial, i.file_id, i.mtime_ns, i.state, r.size
             FROM file_instance i JOIN revision r ON r.id = i.revision_id WHERE i.id = ?1",
        )?
        .query_row([instance], |row| {
            Ok((
                row.get::<_, FolderId>(0)?,
                row.get::<_, StoredName>(1)?.0,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<Vec<u8>>>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, InstanceState>(5)?,
                row.get::<_, i64>(6)?,
            ))
        })
        .optional()?
        .ok_or_else(|| Error::NotFound(format!("there is no file {instance}")))?;
    let (folder_id, name, volume, file_id, mtime_ns, state, size) = row;
    Ok(KnownFile {
        id: instance,
        folder_id,
        path: folder_path(connection, folder_id)?.join(name),
        volume: volume.map(volume_from_sql),
        file_id: file_id_from_sql(file_id),
        mtime_ns,
        size: size as u64,
        state,
    })
}

/// Refuses to touch a file that is not what the library thinks it is. Otherwise someone's new file
/// could be moved or deleted on the strength of an old record.
fn check_file_is_as_known(probe: &dyn FsProbe, file: &KnownFile) -> Result<()> {
    if file.state == InstanceState::Absent {
        return Err(Error::Refused(format!(
            "{} is not there, as far as the library knows",
            file.path.display()
        )));
    }
    let stat = probe.stat(&file.path).map_err(|error| {
        Error::Refused(format!(
            "{} cannot be looked at: {error}",
            file.path.display()
        ))
    })?;
    let same = Some(stat.volume) == file.volume
        && stat.file_id == file.file_id
        && stat.size == file.size
        && stat.mtime_ns == file.mtime_ns;
    if same {
        Ok(())
    } else {
        Err(Error::Refused(format!(
            "{} is not the file the library knows: it needs a reconcile first",
            file.path.display()
        )))
    }
}

/// The folder at `path`, if the library has one there.
pub(crate) fn folder_at(
    connection: &Connection,
    path: &Path,
) -> Result<Option<(FolderId, RootId)>> {
    let roots = crate::reconcile::list_roots(connection)?;
    for root in roots {
        let Ok(relative) = path.strip_prefix(&root.path) else {
            continue;
        };
        let top: Option<FolderId> = connection
            .prepare_cached("SELECT id FROM folder WHERE root_id = ?1 AND parent_id IS NULL")?
            .query_row([root.id], |row| row.get(0))
            .optional()?;
        let Some(mut current) = top else {
            continue;
        };
        let mut found = true;
        for component in relative {
            let child: Option<FolderId> = connection
                .prepare_cached(
                    "SELECT id FROM folder WHERE parent_id = ?1 AND name = ?2 AND state = 'present'",
                )?
                .query_row(params![current, name_to_sql(component)], |row| row.get(0))
                .optional()?;
            match child {
                Some(child) => current = child,
                None => {
                    found = false;
                    break;
                }
            }
        }
        if found {
            return Ok(Some((current, root.id)));
        }
    }
    Ok(None)
}

#[expect(clippy::too_many_arguments, reason = "the columns of the row")]
fn write_intent(
    database: &Database,
    kind: &'static str,
    instance: Option<InstanceId>,
    folder: Option<FolderId>,
    source: &Path,
    target: Option<&Path>,
    undo: &UndoData,
    undo_of: Option<OperationId>,
) -> Result<OperationId> {
    let source = name_to_sql(source.as_os_str());
    let target = target.map(|path| name_to_sql(path.as_os_str()));
    let undo = serde_json::to_string(undo).unwrap_or_else(|_| "{}".to_owned());
    database.write(move |transaction| {
        transaction
            .prepare_cached(
                "INSERT INTO fs_operation (kind, state, instance_id, folder_id, source_path,
                                           target_path, undo_data, undo_of, created_at)
                 VALUES (?1, 'intent', ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?
            .execute(params![
                kind,
                instance,
                folder,
                source,
                target,
                undo,
                undo_of,
                now_ms()
            ])?;
        Ok(transaction.last_insert_rowid())
    })
}

fn finish(
    transaction: &Transaction<'_>,
    operation: OperationId,
    state: &str,
    error: Option<&str>,
) -> Result<()> {
    transaction
        .prepare_cached(
            "UPDATE fs_operation SET state = ?2, error = ?3, finished_at = ?4 WHERE id = ?1",
        )?
        .execute(params![operation, state, error, now_ms()])?;
    Ok(())
}

fn fail(database: &Database, operation: OperationId, message: String) -> Error {
    let recorded = message.clone();
    let _ = database
        .write(move |transaction| finish(transaction, operation, "failed", Some(&recorded)));
    Error::Refused(message)
}

fn describe_move_error(error: &MoveError, target: &Path) -> String {
    match error {
        MoveError::TargetExists => format!("{} already exists", target.display()),
        MoveError::OtherVolume => {
            format!(
                "{} is on another volume, which takes a copy",
                target.display()
            )
        }
        MoveError::Io(error) => format!("moving to {}: {error}", target.display()),
    }
}

/// Moves or renames a file the library knows.
pub(crate) fn move_file(
    database: &Database,
    probe: &dyn FsProbe,
    instance: InstanceId,
    target: &Path,
) -> Result<OperationId> {
    let target = target.to_owned();
    let (file, target_folder) = database.read(|connection| {
        let file = load_file(connection, instance)?;
        let parent = target.parent().unwrap_or(Path::new(""));
        let folder = folder_at(connection, parent)?;
        Ok((file, folder))
    })?;
    let Some((target_folder, _)) = target_folder else {
        return Err(Error::Refused(format!(
            "{} is not a folder inside a monitored root",
            target.parent().unwrap_or(Path::new("")).display()
        )));
    };
    let Some(name) = target.file_name().map(std::ffi::OsStr::to_owned) else {
        return Err(Error::Refused("the target has no file name".to_owned()));
    };
    if format_of(&name).is_none() {
        return Err(Error::Refused(format!(
            "with the name {}, the library would no longer see this file",
            name.to_string_lossy()
        )));
    }
    check_file_is_as_known(probe, &file)?;

    let undo = UndoData {
        volume: file.volume,
        file_id: file.file_id.map(hex_id),
        source_folder: Some(file.folder_id),
        source_name: file
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned),
        ..UndoData::default()
    };
    let operation = write_intent(
        database,
        KIND_MOVE_FILE,
        Some(file.id),
        None,
        &file.path,
        Some(&target),
        &undo,
        None,
    )?;
    perform_file_move(
        database,
        operation,
        instance,
        &file.path,
        &target,
        target_folder,
    )?;
    Ok(operation)
}

fn perform_file_move(
    database: &Database,
    operation: OperationId,
    instance: InstanceId,
    source: &Path,
    target: &Path,
    target_folder: FolderId,
) -> Result<()> {
    if let Err(error) = move_no_replace(source, target) {
        return Err(fail(
            database,
            operation,
            describe_move_error(&error, target),
        ));
    }
    let name = target.file_name().map(name_to_sql);
    database.write(move |transaction| {
        apply_file_location(transaction, instance, target_folder, name)?;
        finish(transaction, operation, "done", None)
    })
}

fn apply_file_location(
    transaction: &Transaction<'_>,
    instance: InstanceId,
    folder: FolderId,
    name: Option<rusqlite::types::Value>,
) -> Result<()> {
    transaction
        .prepare_cached("UPDATE file_instance SET folder_id = ?2, name = ?3 WHERE id = ?1")?
        .execute(params![instance, folder, name])?;
    Ok(())
}

/// Moves or renames a folder, which changes one row however much is in it.
pub(crate) fn move_folder(
    database: &Database,
    probe: &dyn FsProbe,
    folder: FolderId,
    target: &Path,
) -> Result<OperationId> {
    let target = target.to_owned();
    let (source, is_top, target_parent) = database.read(|connection| {
        let source = folder_path(connection, folder)?;
        let is_top: bool = connection
            .prepare_cached("SELECT parent_id IS NULL FROM folder WHERE id = ?1")?
            .query_row([folder], |row| row.get(0))
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("there is no folder {folder}")))?;
        let parent = target.parent().unwrap_or(Path::new(""));
        Ok((source, is_top, folder_at(connection, parent)?))
    })?;
    if is_top {
        return Err(Error::Refused(
            "a root's own folder is moved by relocating the root".to_owned(),
        ));
    }
    let Some((target_parent, _)) = target_parent else {
        return Err(Error::Refused(format!(
            "{} is not a folder inside a monitored root",
            target.parent().unwrap_or(Path::new("")).display()
        )));
    };
    if target.starts_with(&source) {
        return Err(Error::Refused(
            "a folder cannot move into itself".to_owned(),
        ));
    }
    let stat = probe.stat(&source).map_err(|error| {
        Error::Refused(format!("{} cannot be looked at: {error}", source.display()))
    })?;
    let undo = UndoData {
        volume: Some(stat.volume),
        file_id: stat.file_id.map(hex_id),
        ..UndoData::default()
    };
    let operation = write_intent(
        database,
        KIND_MOVE_FOLDER,
        None,
        Some(folder),
        &source,
        Some(&target),
        &undo,
        None,
    )?;
    perform_folder_move(database, operation, folder, &source, &target, target_parent)?;
    Ok(operation)
}

fn perform_folder_move(
    database: &Database,
    operation: OperationId,
    folder: FolderId,
    source: &Path,
    target: &Path,
    target_parent: FolderId,
) -> Result<()> {
    if let Err(error) = move_no_replace(source, target) {
        return Err(fail(
            database,
            operation,
            describe_move_error(&error, target),
        ));
    }
    let name = target.file_name().map(name_to_sql);
    database.write(move |transaction| {
        apply_folder_location(transaction, folder, target_parent, name)?;
        finish(transaction, operation, "done", None)
    })
}

fn apply_folder_location(
    transaction: &Transaction<'_>,
    folder: FolderId,
    parent: FolderId,
    name: Option<rusqlite::types::Value>,
) -> Result<()> {
    transaction
        .prepare_cached(
            "UPDATE folder SET parent_id = ?2, name = ?3,
                 root_id = (SELECT root_id FROM folder WHERE id = ?2) WHERE id = ?1",
        )?
        .execute(params![folder, parent, name])?;
    transaction
        .prepare_cached(
            "WITH RECURSIVE subtree(id) AS (
                 SELECT id FROM folder WHERE parent_id = ?1
                 UNION ALL
                 SELECT folder.id FROM folder JOIN subtree ON folder.parent_id = subtree.id
             )
             UPDATE folder SET root_id = (SELECT root_id FROM folder WHERE id = ?1)
             WHERE id IN subtree",
        )?
        .execute([folder])?;
    Ok(())
}

/// Sends a file to the system's Recycle Bin. Its instance goes absent, and everything else stays.
pub(crate) fn recycle(
    database: &Database,
    probe: &dyn FsProbe,
    instance: InstanceId,
) -> Result<OperationId> {
    let file = database.read(|connection| load_file(connection, instance))?;
    check_file_is_as_known(probe, &file)?;
    if !has_recycle_bin(&file.path) {
        return Err(Error::Refused(format!(
            "{} is on a volume with no Recycle Bin, where deleting cannot be undone",
            file.path.display()
        )));
    }
    if file.path.as_os_str().encode_wide().count() > LONGEST_PATH_THE_RECYCLE_BIN_RESTORES {
        return Err(Error::Refused(format!(
            "{} has a path too long for the Recycle Bin to give back, so it stays where it is",
            file.path.display()
        )));
    }
    // One item with a name like this makes the crate that reads the Recycle Bin fail on all of it, so
    // it would take away the way back from every other file in there too.
    if file.path.to_str().is_none() {
        return Err(Error::Refused(format!(
            "{} has a name that is not valid Unicode, which the Recycle Bin cannot be asked about",
            file.path.display()
        )));
    }
    // The way back goes through reading the Recycle Bin. If that does not work now, nothing goes in.
    on_com_thread(|| trash::os_limited::list().map_err(|error| error.to_string())).map_err(
        |error| {
            Error::Refused(format!(
                "the Recycle Bin cannot be read, so this could not be undone: {error}"
            ))
        },
    )?;
    let undo = UndoData {
        volume: file.volume,
        file_id: file.file_id.map(hex_id),
        ..UndoData::default()
    };
    let operation = write_intent(
        database,
        KIND_RECYCLE,
        Some(file.id),
        None,
        &file.path,
        None,
        &undo,
        None,
    )?;

    let path = file.path.clone();
    if let Err(message) =
        on_com_thread(move || trash::delete(&path).map_err(|error| error.to_string()))
    {
        return Err(fail(
            database,
            operation,
            format!("the Recycle Bin refused it: {message}"),
        ));
    }
    if file.path.exists() {
        return Err(fail(
            database,
            operation,
            "the file is still there after the Recycle Bin took it".to_owned(),
        ));
    }

    let found = find_in_recycle_bin(&file.path, None).ok().flatten();
    // Windows can be set to delete things right away, and then this is as far as anyone can know.
    let warning = found
        .is_none()
        .then_some("the file is gone, and the Recycle Bin does not list it");
    let undo = UndoData {
        time_deleted: found.map(|item| item.time_deleted),
        ..undo
    };
    let undo_json = serde_json::to_string(&undo).unwrap_or_else(|_| "{}".to_owned());
    database.write(move |transaction| {
        transaction
            .prepare_cached("UPDATE fs_operation SET undo_data = ?2 WHERE id = ?1")?
            .execute(params![operation, undo_json])?;
        transaction
            .prepare_cached(
                "UPDATE file_instance SET state = 'absent', state_since = ?2 WHERE id = ?1",
            )?
            .execute(params![instance, now_ms()])?;
        finish(transaction, operation, "done", warning)
    })?;
    Ok(operation)
}

/// Runs what needs COM on a thread of its own. It settles the thread's COM apartment for good, which
/// is not this library's call to make on a caller's thread, and it can panic, which stops here.
fn on_com_thread<T: Send + 'static>(
    work: impl FnOnce() -> std::result::Result<T, String> + Send + 'static,
) -> std::result::Result<T, String> {
    thread::Builder::new()
        .name("musaeum-recycle".to_owned())
        .spawn(work)
        .map_err(|error| error.to_string())?
        .join()
        .unwrap_or_else(|_| Err("the call panicked".to_owned()))
}

/// Restoring moves the file back from `$R...`, and leaves the `$I...` next to it that said where it
/// came from. That record is of no use to anyone any more. If this fails, it is only a few bytes that
/// go away with the next emptying of the Recycle Bin.
fn remove_record_left_behind(held_at: &Path) {
    let Some(name) = held_at.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    if let Some(rest) = name.strip_prefix("$R") {
        let _ = std::fs::remove_file(held_at.with_file_name(format!("$I{rest}")));
    }
}

/// For comparing the paths the shell reports with the ones used here.
fn normalized(path: &Path) -> String {
    let text = path.to_string_lossy();
    text.strip_prefix(r"\\?\")
        .unwrap_or(&text)
        .replace('/', r"\")
        .to_lowercase()
}

/// The item the Recycle Bin holds for this path: the one deleted at that moment, or else the newest.
fn find_in_recycle_bin(
    path: &Path,
    time_deleted: Option<i64>,
) -> std::result::Result<Option<TrashItem>, String> {
    let wanted = normalized(path);
    let items = on_com_thread(|| trash::os_limited::list().map_err(|error| error.to_string()))?;
    let mut matching: Vec<TrashItem> = items
        .into_iter()
        .filter(|item| normalized(&item.original_path()) == wanted)
        .collect();
    matching.sort_by_key(|item| item.time_deleted);
    if let Some(time) = time_deleted
        && let Some(position) = matching.iter().position(|item| item.time_deleted == time)
    {
        return Ok(Some(matching.swap_remove(position)));
    }
    Ok(matching.pop())
}

/// Takes back what an operation did, as a new entry in the journal.
pub(crate) fn undo(
    database: &Database,
    probe: &dyn FsProbe,
    operation: OperationId,
) -> Result<OperationId> {
    let record = database.read(|connection| load_record(connection, operation))?;
    if record.state != "done" {
        return Err(Error::Refused(format!(
            "operation {operation} is {}, so there is nothing to undo",
            record.state
        )));
    }
    if record.undo_of.is_some() {
        return Err(Error::Refused(format!(
            "operation {operation} is itself an undo"
        )));
    }
    match record.kind.as_str() {
        KIND_MOVE_FILE => undo_move_file(database, probe, operation, &record),
        KIND_MOVE_FOLDER => undo_move_folder(database, probe, operation, &record),
        KIND_RECYCLE => undo_recycle(database, probe, operation, &record),
        other => Err(Error::Refused(format!("a {other} cannot be undone"))),
    }
}

struct Record {
    kind: String,
    state: String,
    instance: Option<InstanceId>,
    folder: Option<FolderId>,
    source: PathBuf,
    target: Option<PathBuf>,
    undo: UndoData,
    undo_of: Option<OperationId>,
}

fn load_record(connection: &Connection, operation: OperationId) -> Result<Record> {
    connection
        .prepare_cached(
            "SELECT kind, state, instance_id, folder_id, source_path, target_path, undo_data, undo_of
             FROM fs_operation WHERE id = ?1",
        )?
        .query_row([operation], read_record)
        .optional()?
        .ok_or_else(|| Error::NotFound(format!("there is no operation {operation}")))
}

fn read_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<Record> {
    let undo: String = row.get(6)?;
    Ok(Record {
        kind: row.get(0)?,
        state: row.get(1)?,
        instance: row.get(2)?,
        folder: row.get(3)?,
        source: PathBuf::from(row.get::<_, StoredName>(4)?.0),
        target: row
            .get::<_, Option<StoredName>>(5)?
            .map(|name| PathBuf::from(name.0)),
        undo: serde_json::from_str(&undo).unwrap_or_default(),
        undo_of: row.get(7)?,
    })
}

fn undo_move_file(
    database: &Database,
    probe: &dyn FsProbe,
    original: OperationId,
    record: &Record,
) -> Result<OperationId> {
    let instance = record
        .instance
        .ok_or_else(|| Error::Refused("the journal entry names no file".to_owned()))?;
    let source_folder = record
        .undo
        .source_folder
        .ok_or_else(|| Error::Refused("the journal entry names no folder".to_owned()))?;
    let file = database.read(|connection| load_file(connection, instance))?;
    check_file_is_as_known(probe, &file)?;

    let back_to = database.read(|connection| {
        let folder = folder_path(connection, source_folder)?;
        Ok(folder.join(record.source.file_name().unwrap_or_default()))
    })?;
    let operation = write_intent(
        database,
        KIND_MOVE_FILE,
        Some(instance),
        None,
        &file.path,
        Some(&back_to),
        &record.undo,
        Some(original),
    )?;
    perform_file_move(
        database,
        operation,
        instance,
        &file.path,
        &back_to,
        source_folder,
    )?;
    mark_undone(database, original)?;
    Ok(operation)
}

fn undo_move_folder(
    database: &Database,
    probe: &dyn FsProbe,
    original: OperationId,
    record: &Record,
) -> Result<OperationId> {
    let folder = record
        .folder
        .ok_or_else(|| Error::Refused("the journal entry names no folder".to_owned()))?;
    let (current, parent) = database.read(|connection| {
        let current = folder_path(connection, folder)?;
        let parent = folder_at(connection, record.source.parent().unwrap_or(Path::new("")))?;
        Ok((current, parent))
    })?;
    let Some((parent, _)) = parent else {
        return Err(Error::Refused(format!(
            "the folder {} used to be in is gone",
            current.display()
        )));
    };
    probe.stat(&current).map_err(|error| {
        Error::Refused(format!(
            "{} cannot be looked at: {error}",
            current.display()
        ))
    })?;
    let operation = write_intent(
        database,
        KIND_MOVE_FOLDER,
        None,
        Some(folder),
        &current,
        Some(&record.source),
        &record.undo,
        Some(original),
    )?;
    perform_folder_move(
        database,
        operation,
        folder,
        &current,
        &record.source,
        parent,
    )?;
    mark_undone(database, original)?;
    Ok(operation)
}

fn undo_recycle(
    database: &Database,
    probe: &dyn FsProbe,
    original: OperationId,
    record: &Record,
) -> Result<OperationId> {
    let instance = record
        .instance
        .ok_or_else(|| Error::Refused("the journal entry names no file".to_owned()))?;
    if record.source.exists() {
        return Err(Error::Refused(format!(
            "{} already exists, and restoring would overwrite it",
            record.source.display()
        )));
    }
    let item = find_in_recycle_bin(&record.source, record.undo.time_deleted)
        .map_err(Error::Refused)?
        .ok_or_else(|| {
            Error::Refused(format!(
                "{} is no longer in the Recycle Bin",
                record.source.display()
            ))
        })?;
    let operation = write_intent(
        database,
        KIND_RESTORE,
        Some(instance),
        None,
        &record.source,
        None,
        &record.undo,
        Some(original),
    )?;
    let held_at = PathBuf::from(&item.id);
    if let Err(message) = on_com_thread(move || {
        trash::os_limited::restore_all([item]).map_err(|error| error.to_string())
    }) {
        return Err(fail(
            database,
            operation,
            format!("the Recycle Bin refused it: {message}"),
        ));
    }
    remove_record_left_behind(&held_at);
    let stat = probe.stat(&record.source).map_err(|error| {
        fail(
            database,
            operation,
            format!("the restored file is not there: {error}"),
        )
    })?;
    database.write(move |transaction| {
        let state = if stat.is_cloud_only() {
            InstanceState::CloudOnly
        } else {
            InstanceState::Present
        };
        transaction
            .prepare_cached("UPDATE file_instance SET state = ?2, state_since = ?3 WHERE id = ?1")?
            .execute(params![instance, state, now_ms()])?;
        finish(transaction, operation, "done", None)
    })?;
    mark_undone(database, original)?;
    Ok(operation)
}

fn mark_undone(database: &Database, operation: OperationId) -> Result<()> {
    database.write(move |transaction| {
        transaction
            .prepare_cached("UPDATE fs_operation SET state = 'undone' WHERE id = ?1")?
            .execute([operation])?;
        Ok(())
    })
}

/// What the journal found on starting up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct RecoveryReport {
    /// Operations that did reach the disk, and are now recorded as done.
    pub completed: u64,
    /// Operations that did not, and are now recorded as never having happened.
    pub aborted: u64,
    /// Operations whose outcome cannot be told from the disk. The next scan settles the library.
    pub failed: u64,
}

/// Finishes the entries a crash left behind, by looking at what is on the disk.
pub(crate) fn recover(database: &Database, probe: &dyn FsProbe) -> Result<RecoveryReport> {
    let records = database.read(|connection| {
        let mut statement = connection.prepare(
            "SELECT id, kind, state, instance_id, folder_id, source_path, target_path, undo_data,
                    undo_of
             FROM fs_operation WHERE state = 'intent' ORDER BY id",
        )?;
        let rows = statement.query_map([], |row| {
            let undo: String = row.get(7)?;
            Ok((
                row.get::<_, OperationId>(0)?,
                Record {
                    kind: row.get(1)?,
                    state: row.get(2)?,
                    instance: row.get(3)?,
                    folder: row.get(4)?,
                    source: PathBuf::from(row.get::<_, StoredName>(5)?.0),
                    target: row
                        .get::<_, Option<StoredName>>(6)?
                        .map(|name| PathBuf::from(name.0)),
                    undo: serde_json::from_str(&undo).unwrap_or_default(),
                    undo_of: row.get(8)?,
                },
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    })?;

    let mut report = RecoveryReport::default();
    for (operation, record) in records {
        recover_one(database, probe, operation, &record, &mut report)?;
    }
    Ok(report)
}

fn same_identity(probe: &dyn FsProbe, path: &Path, undo: &UndoData) -> bool {
    probe.stat(path).is_ok_and(|stat| {
        undo.file_id
            .as_ref()
            .is_none_or(|id| stat.file_id.map(hex_id).as_ref() == Some(id))
            && undo.volume.is_none_or(|volume| volume == stat.volume)
    })
}

fn recover_one(
    database: &Database,
    probe: &dyn FsProbe,
    operation: OperationId,
    record: &Record,
    report: &mut RecoveryReport,
) -> Result<()> {
    let source_here = same_identity(probe, &record.source, &record.undo);
    let target_here = record
        .target
        .as_ref()
        .is_some_and(|target| same_identity(probe, target, &record.undo));

    let outcome = match record.kind.as_str() {
        KIND_MOVE_FILE | KIND_MOVE_FOLDER if source_here => Outcome::NeverHappened,
        KIND_MOVE_FILE | KIND_MOVE_FOLDER if target_here => Outcome::Happened,
        KIND_RECYCLE if source_here => Outcome::NeverHappened,
        KIND_RECYCLE if !record.source.exists() => Outcome::Happened,
        KIND_RESTORE if source_here => Outcome::Happened,
        KIND_RESTORE if !record.source.exists() => Outcome::NeverHappened,
        _ => Outcome::Unknown,
    };

    let target = record.target.clone();
    let kind = record.kind.clone();
    let instance = record.instance;
    let folder = record.folder;
    let undo_of = record.undo_of;
    match outcome {
        Outcome::NeverHappened => {
            database.write(move |transaction| finish(transaction, operation, "aborted", None))?;
            report.aborted += 1;
        }
        Outcome::Unknown => {
            database.write(move |transaction| {
                finish(
                    transaction,
                    operation,
                    "failed",
                    Some("after a crash, the disk matches neither before nor after"),
                )
            })?;
            report.failed += 1;
        }
        Outcome::Happened => {
            database.write(move |transaction| {
                apply_recovered(transaction, &kind, instance, folder, target.as_deref())?;
                finish(transaction, operation, "done", None)?;
                if let Some(original) = undo_of {
                    transaction
                        .prepare_cached("UPDATE fs_operation SET state = 'undone' WHERE id = ?1")?
                        .execute([original])?;
                }
                Ok(())
            })?;
            report.completed += 1;
        }
    }
    Ok(())
}

enum Outcome {
    NeverHappened,
    Happened,
    Unknown,
}

fn apply_recovered(
    transaction: &Transaction<'_>,
    kind: &str,
    instance: Option<InstanceId>,
    folder: Option<FolderId>,
    target: Option<&Path>,
) -> Result<()> {
    match (kind, instance, folder, target) {
        (KIND_MOVE_FILE, Some(instance), _, Some(target)) => {
            let parent = target.parent().unwrap_or(Path::new(""));
            if let Some((target_folder, _)) = folder_at(transaction, parent)? {
                apply_file_location(
                    transaction,
                    instance,
                    target_folder,
                    target.file_name().map(name_to_sql),
                )?;
            }
        }
        (KIND_MOVE_FOLDER, _, Some(folder), Some(target)) => {
            let parent = target.parent().unwrap_or(Path::new(""));
            if let Some((target_parent, _)) = folder_at(transaction, parent)? {
                apply_folder_location(
                    transaction,
                    folder,
                    target_parent,
                    target.file_name().map(name_to_sql),
                )?;
            }
        }
        (KIND_RECYCLE, Some(instance), _, _) => {
            transaction
                .prepare_cached(
                    "UPDATE file_instance SET state = 'absent', state_since = ?2 WHERE id = ?1",
                )?
                .execute(params![instance, now_ms()])?;
        }
        (KIND_RESTORE, Some(instance), _, _) => {
            transaction
                .prepare_cached(
                    "UPDATE file_instance SET state = 'present', state_since = ?2 WHERE id = ?1",
                )?
                .execute(params![instance, now_ms()])?;
        }
        _ => {}
    }
    Ok(())
}

/// The journal, newest first.
pub(crate) fn list(connection: &Connection, limit: usize) -> Result<Vec<OperationInfo>> {
    let mut statement = connection.prepare_cached(
        "SELECT id, kind, state, instance_id, folder_id, source_path, target_path, undo_of, error
         FROM fs_operation ORDER BY id DESC LIMIT ?1",
    )?;
    let rows = statement.query_map([limit as i64], |row| {
        Ok(OperationInfo {
            id: row.get(0)?,
            kind: row.get(1)?,
            state: row.get(2)?,
            instance_id: row.get(3)?,
            folder_id: row.get(4)?,
            source_path: PathBuf::from(row.get::<_, StoredName>(5)?.0),
            target_path: row
                .get::<_, Option<StoredName>>(6)?
                .map(|name| name.0.to_string_lossy().into_owned()),
            undo_of: row.get(7)?,
            error: row.get(8)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

//! A listing of everything in a folder tree, to prove afterwards that nothing in it changed.
//!
//! It deliberately does not use `musaeum-fs`, which is part of what is being checked: this is the
//! standard library's directory listing, which reads what the directory says about each entry and
//! opens no file.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde::Serialize;

/// What is recorded about one entry. Its last access time is kept apart, because Windows may update
/// it on its own when a file is read, which is not a change to the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub is_dir: bool,
    pub size: u64,
    pub modified: u64,
    pub created: u64,
    pub attributes: u32,
}

#[derive(Debug, Default)]
pub struct Snapshot {
    pub entries: BTreeMap<PathBuf, Entry>,
    pub accessed: BTreeMap<PathBuf, u64>,
    pub unreadable_dirs: Vec<PathBuf>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Comparison {
    pub files: usize,
    pub directories: usize,
    pub bytes: u64,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<String>,
    /// Entries whose last access time Windows updated while they were read.
    pub access_time_updated: usize,
}

impl Comparison {
    pub fn unchanged(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

pub fn take(root: &Path) -> io::Result<Snapshot> {
    let mut snapshot = Snapshot::default();
    let mut pending = vec![root.to_owned()];
    while let Some(directory) = pending.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(_) if directory != root => {
                snapshot.unreadable_dirs.push(directory);
                continue;
            }
            Err(error) => return Err(error),
        };
        for entry in entries {
            let entry = entry?;
            // From the directory listing itself: no file is opened.
            let metadata = entry.metadata()?;
            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap_or(&path).to_owned();
            if metadata.is_dir() {
                pending.push(path);
            }
            snapshot.entries.insert(
                relative.clone(),
                Entry {
                    is_dir: metadata.is_dir(),
                    size: metadata.file_size(),
                    modified: metadata.last_write_time(),
                    created: metadata.creation_time(),
                    attributes: metadata.file_attributes(),
                },
            );
            snapshot
                .accessed
                .insert(relative, metadata.last_access_time());
        }
    }
    Ok(snapshot)
}

pub fn compare(before: &Snapshot, after: &Snapshot) -> Comparison {
    let mut comparison = Comparison::default();
    for (path, entry) in &before.entries {
        if entry.is_dir {
            comparison.directories += 1;
        } else {
            comparison.files += 1;
            comparison.bytes += entry.size;
        }
        match after.entries.get(path) {
            None => comparison.removed.push(path.display().to_string()),
            Some(now) if now != entry => comparison.changed.push(path.display().to_string()),
            Some(_) => {}
        }
        if before.accessed.get(path) != after.accessed.get(path) {
            comparison.access_time_updated += 1;
        }
    }
    for path in after.entries.keys() {
        if !before.entries.contains_key(path) {
            comparison.added.push(path.display().to_string());
        }
    }
    comparison
}

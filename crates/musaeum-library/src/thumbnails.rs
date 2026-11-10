//! Page thumbnails on disk, by the exact bytes of the file they were drawn from.
//!
//! ```text
//! cache/thumbnails/v1/ab/<the file's BLAKE3, in hex>/<page>.jpg
//! ```
//!
//! Keyed by the hash, a thumbnail survives renames and moves, is shared by every copy of the same
//! file, and is never shown for a file that changed. Pages count from zero, as everywhere else in
//! the cache. Like `cache.db`, all of this can be deleted and drawn again; the version in the path
//! lets a future size or format live beside this one instead of being mistaken for it.

use std::collections::HashSet;
use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use crate::fingerprint::Hash;
use crate::model::hex;

/// The marker of a file with no cover to draw.
const NO_COVER: &str = "no-cover";

/// Where a library keeps its thumbnails.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThumbnailStore {
    root: PathBuf,
}

impl ThumbnailStore {
    /// The store inside a cache folder.
    pub fn new(cache_dir: &Path) -> Self {
        Self {
            root: cache_dir.join("thumbnails").join("v1"),
        }
    }

    /// The folder holding one file's thumbnails.
    pub fn document_dir(&self, blake3: &Hash) -> PathBuf {
        let hash = hex(blake3);
        self.root.join(&hash[..2]).join(hash)
    }

    /// Where a page's thumbnail is, or would be.
    pub fn path(&self, blake3: &Hash, page: u32) -> PathBuf {
        self.document_dir(blake3).join(format!("{page}.jpg"))
    }

    /// The pages below `page_count` that have no thumbnail yet, in order. One listing of a folder,
    /// however many pages there are.
    pub fn missing(&self, blake3: &Hash, page_count: u32) -> io::Result<Vec<u32>> {
        let present: HashSet<u32> = match fs::read_dir(self.document_dir(blake3)) {
            Ok(entries) => entries
                .filter_map(|entry| entry.ok())
                .filter_map(|entry| {
                    let name = entry.file_name();
                    name.to_str()?.strip_suffix(".jpg")?.parse().ok()
                })
                .collect(),
            Err(error) if error.kind() == ErrorKind::NotFound => HashSet::new(),
            Err(error) => return Err(error),
        };
        Ok((0..page_count)
            .filter(|page| !present.contains(page))
            .collect())
    }

    /// Keeps a page's thumbnail, replacing any older one. The file is written beside its final
    /// name and then renamed, so a reader never sees half of one.
    pub fn put(&self, blake3: &Hash, page: u32, jpeg: &[u8]) -> io::Result<()> {
        let path = self.path(blake3, page);
        let dir = path
            .parent()
            .expect("a thumbnail is inside its document's folder");
        fs::create_dir_all(dir)?;
        let partial = dir.join(format!("{page}.jpg.{}.part", std::process::id()));
        fs::write(&partial, jpeg)?;
        fs::rename(&partial, &path).inspect_err(|_| {
            let _ = fs::remove_file(&partial);
        })
    }

    /// Records that the file with these bytes has no cover that can be drawn, so that nothing
    /// tries again for the same bytes. Forgotten with the rest of its thumbnails.
    pub fn mark_without_cover(&self, blake3: &Hash) -> io::Result<()> {
        let dir = self.document_dir(blake3);
        fs::create_dir_all(&dir)?;
        fs::write(dir.join(NO_COVER), b"")
    }

    /// Whether the file with these bytes was found to have no cover.
    pub fn is_without_cover(&self, blake3: &Hash) -> bool {
        self.document_dir(blake3).join(NO_COVER).exists()
    }

    /// A page's thumbnail, if there is one.
    pub fn get(&self, blake3: &Hash, page: u32) -> io::Result<Option<Vec<u8>>> {
        match fs::read(self.path(blake3, page)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Forgets every thumbnail of one file.
    pub fn remove(&self, blake3: &Hash) -> io::Result<()> {
        match fs::remove_dir_all(self.document_dir(blake3)) {
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            result => result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(test: &str) -> (ThumbnailStore, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("musaeum-thumbnails-{test}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        (ThumbnailStore::new(&dir), dir)
    }

    #[test]
    fn thumbnails_are_kept_by_hash_and_page_and_found_missing_in_one_listing() {
        let (store, dir) = store("keep");
        let hash = [0xab; 32];
        assert_eq!(store.missing(&hash, 3).unwrap(), vec![0, 1, 2]);
        store.put(&hash, 1, b"jpeg one").unwrap();
        store.put(&hash, 1, b"jpeg one, again").unwrap();
        assert_eq!(
            store.get(&hash, 1).unwrap(),
            Some(b"jpeg one, again".to_vec())
        );
        assert_eq!(store.get(&hash, 0).unwrap(), None);
        assert_eq!(store.missing(&hash, 3).unwrap(), vec![0, 2]);
        assert!(
            store
                .path(&hash, 1)
                .ends_with(Path::new("thumbnails/v1/ab").join(hex(&hash)).join("1.jpg"))
        );
        // Another file's thumbnails are its own.
        assert_eq!(store.missing(&[0xcd; 32], 2).unwrap(), vec![0, 1]);

        // Nothing half written is left behind.
        let leftovers = fs::read_dir(store.document_dir(&hash))
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".part")
            })
            .count();
        assert_eq!(leftovers, 0);

        store.remove(&hash).unwrap();
        store.remove(&hash).unwrap();
        assert_eq!(store.missing(&hash, 2).unwrap(), vec![0, 1]);
        fs::remove_dir_all(dir).unwrap();
    }
}

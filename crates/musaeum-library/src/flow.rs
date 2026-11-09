//! Normalized books on disk, by the exact bytes of the file and the normalizer's version.
//!
//! ```text
//! cache/flow/v1/ab/<the file's BLAKE3, in hex>/package.opf, musaeum.json, text/0000.xhtml, ...
//! ```
//!
//! Like the thumbnails, a normalized book is shared by every copy of the same file, survives
//! renames and moves, and is never shown for a file that changed. The normalizer's version is in the
//! path, so a new version normalizes again beside the old instead of reading what it did not write.
//! A book is written whole into a folder of its own and only then renamed into place, so a reader
//! finds a complete book or none.

use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use musaeum_flow::NORMALIZER_VERSION;
use musaeum_flow::package::{MANIFEST_PATH, Manifest};
use musaeum_ingest::protocol::is_safe_path;

use crate::fingerprint::Hash;
use crate::model::hex;

/// Where a library keeps its normalized books.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlowStore {
    root: PathBuf,
}

impl FlowStore {
    /// The store inside a cache folder.
    pub fn new(cache_dir: &Path) -> Self {
        Self {
            root: cache_dir
                .join("flow")
                .join(format!("v{NORMALIZER_VERSION}")),
        }
    }

    /// The folder of one book.
    pub fn book_dir(&self, blake3: &Hash) -> PathBuf {
        let hash = hex(blake3);
        self.root.join(&hash[..2]).join(hash)
    }

    /// Keeps a normalized book, replacing an older one of the same file. Every path is checked
    /// again here, whoever produced it.
    pub fn put(&self, blake3: &Hash, files: &[(String, Vec<u8>)]) -> io::Result<PathBuf> {
        let target = self.book_dir(blake3);
        let parent = target.parent().expect("a book's folder has a parent");
        fs::create_dir_all(parent)?;
        let partial = parent.join(format!("{}.{}.part", hex(blake3), std::process::id()));
        let _ = fs::remove_dir_all(&partial);
        let written = (|| {
            for (path, data) in files {
                if !is_safe_path(path) {
                    return Err(io::Error::new(
                        ErrorKind::InvalidData,
                        format!("the path {path:?}"),
                    ));
                }
                let file = partial.join(path);
                fs::create_dir_all(file.parent().expect("a file is in a folder"))?;
                fs::write(&file, data)?;
            }
            Ok(())
        })();
        if let Err(error) = written {
            let _ = fs::remove_dir_all(&partial);
            return Err(error);
        }
        match fs::remove_dir_all(&target) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                let _ = fs::remove_dir_all(&partial);
                return Err(error);
            }
        }
        fs::rename(&partial, &target).inspect_err(|_| {
            let _ = fs::remove_dir_all(&partial);
        })?;
        Ok(target)
    }

    /// One file of a stored book.
    pub fn read(&self, blake3: &Hash, path: &str) -> io::Result<Option<Vec<u8>>> {
        if !is_safe_path(path) {
            return Ok(None);
        }
        match fs::read(self.book_dir(blake3).join(path)) {
            Ok(data) => Ok(Some(data)),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// A stored book's manifest, if the book is there.
    pub fn manifest(&self, blake3: &Hash) -> io::Result<Option<Manifest>> {
        let Some(bytes) = self.read(blake3, MANIFEST_PATH)? else {
            return Ok(None);
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| io::Error::new(ErrorKind::InvalidData, error))
    }

    /// Removes a stored book.
    pub fn remove(&self, blake3: &Hash) -> io::Result<()> {
        match fs::remove_dir_all(self.book_dir(blake3)) {
            Err(error) if error.kind() != ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_book_is_kept_whole_and_read_back() {
        let dir = std::env::temp_dir().join(format!("musaeum-flow-store-{}", std::process::id()));
        let store = FlowStore::new(&dir);
        let hash = [7u8; 32];
        let files = vec![
            ("text/0000.xhtml".to_owned(), b"<html/>".to_vec()),
            ("package.opf".to_owned(), b"<package/>".to_vec()),
        ];
        let at = store.put(&hash, &files).unwrap();
        assert!(at.ends_with(hex(&hash)));
        assert_eq!(
            store.read(&hash, "text/0000.xhtml").unwrap().unwrap(),
            b"<html/>"
        );
        assert_eq!(store.read(&hash, "../x").unwrap(), None);
        assert!(
            store
                .put(&hash, &[("../evil".to_owned(), b"x".to_vec())])
                .is_err()
        );
        assert!(
            store.read(&hash, "package.opf").unwrap().is_some(),
            "a refused put leaves the old book"
        );
        store.remove(&hash).unwrap();
        assert_eq!(store.read(&hash, "package.opf").unwrap(), None);
        let _ = fs::remove_dir_all(&dir);
    }
}

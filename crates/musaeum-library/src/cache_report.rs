//! What a library's cache holds, and forgetting the part of it that is the slowest to make but
//! the easiest to make again: the lines traced on scanned pages, and what the OCR read on them.
//!
//! Everything in the cache can be made again from the books; nothing in it is the person's. The
//! annotations, their quotes (an OCR quote is kept with its annotation, in `library.db`) and every
//! other choice live in the library, which this does not touch.

use std::path::Path;

use serde::Serialize;

use crate::error::Result;
use crate::library::Library;

/// What the cache holds, by part, and what it weighs on the disk.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheReport {
    /// The cache's folder.
    pub folder: String,
    /// Pages whose lines were traced, and the bytes of their models.
    pub line_pages: u64,
    pub line_bytes: u64,
    /// Lines the OCR read, and the bytes of what it read.
    pub ocr_lines: u64,
    pub ocr_bytes: u64,
    /// The full-text index (Tantivy), on the disk.
    pub fulltext_bytes: u64,
    /// The flowing books, normalized, on the disk.
    pub flow_bytes: u64,
    /// The covers and page thumbnails, on the disk.
    pub thumbnail_bytes: u64,
    /// `cache.db` and its log, on the disk: the lines and the OCR among the rest (pages' sizes and
    /// characters).
    pub database_bytes: u64,
    /// Everything in the cache's folder.
    pub total_bytes: u64,
}

/// The bytes of every file under `path`, or of `path` itself; nothing if it is not there.
fn bytes_under(path: &Path) -> u64 {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if metadata.is_file() {
        return metadata.len();
    }
    if !metadata.is_dir() {
        return 0;
    }
    std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .map(|entry| bytes_under(&entry.path()))
        .sum()
}

impl Library {
    /// What the cache holds now.
    pub fn cache_report(&self) -> Result<CacheReport> {
        let folder = self.paths().cache_dir.clone();
        let mut report = CacheReport {
            folder: folder.to_string_lossy().into_owned(),
            fulltext_bytes: bytes_under(&folder.join("fulltext")),
            flow_bytes: bytes_under(&folder.join("flow")),
            thumbnail_bytes: bytes_under(&folder.join("thumbnails")),
            database_bytes: ["cache.db", "cache.db-wal", "cache.db-shm"]
                .iter()
                .map(|name| bytes_under(&folder.join(name)))
                .sum(),
            total_bytes: bytes_under(&folder),
            ..CacheReport::default()
        };
        if let Some(cache) = self.cache() {
            let ((line_pages, line_bytes), (ocr_lines, ocr_bytes)) = cache.read(|connection| {
                let count = |sql: &str| -> Result<(u64, u64)> {
                    Ok(connection.query_row(sql, [], |row| {
                        Ok((
                            row.get::<_, i64>(0)?.max(0) as u64,
                            row.get::<_, i64>(1)?.max(0) as u64,
                        ))
                    })?)
                };
                Ok((
                    count("SELECT count(*), coalesce(sum(length(model)), 0) FROM line_model")?,
                    count("SELECT count(*), coalesce(sum(length(words)), 0) FROM ocr_line")?,
                ))
            })?;
            report.line_pages = line_pages;
            report.line_bytes = line_bytes;
            report.ocr_lines = ocr_lines;
            report.ocr_bytes = ocr_bytes;
        }
        Ok(report)
    }

    /// Forgets every page's lines and everything the OCR read, and gives the disk back the room
    /// they took. Lines are traced again when a page is shown with a tool that needs them, and a
    /// line is read again when a selection or a mark needs its text. Returns how many pages' lines
    /// and how many lines read were forgotten.
    pub fn clear_lines_and_ocr(&self) -> Result<(u64, u64)> {
        let Some(cache) = self.cache() else {
            return Ok((0, 0));
        };
        let removed = cache.write(|transaction| {
            let pages = transaction.execute("DELETE FROM line_model", [])? as u64;
            let lines = transaction.execute("DELETE FROM ocr_line", [])? as u64;
            Ok((pages, lines))
        })?;
        // Giving the room back is a courtesy: what matters, that nothing stale is used, is done.
        if let Err(error) = cache.vacuum() {
            eprintln!("the cache could not be compacted after clearing: {error}");
        }
        Ok(removed)
    }
}

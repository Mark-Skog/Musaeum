//! The lines of text the detector found on scanned pages, in `cache.db`, by the file's exact bytes.
//!
//! The library does not read the model: it keeps the detector's bytes, the version that made them
//! and the page's perceptual hash, and says which pages still need tracing. What the bytes mean is
//! `musaeum-vision`'s.

use rusqlite::{OptionalExtension, params};

use crate::error::{Error, Result};
use crate::fingerprint::Hash;
use crate::library::Library;
use crate::sql::now_ms;

/// The most an encoded model may weigh. A page of a dense dictionary in two columns comes to a few
/// tens of kilobytes; a model this large is not one.
pub const MAX_LINE_MODEL_BYTES: usize = 4 << 20;

/// One page's model, as it is kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineModelRecord {
    /// The page, from zero.
    pub page: u32,
    /// The detector's version that made it.
    pub version: u32,
    /// The 64-bit difference hash of the page's picture.
    pub page_hash: u64,
    /// The encoded model.
    pub model: Vec<u8>,
}

/// What the OCR read on one line of a scanned page, as it is kept: the words, in reading order, as
/// JSON the reader writes and reads; the library only keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OcrLineRecord {
    /// The line's index in its page's model.
    pub line: u32,
    pub words: String,
}

/// What read a page's lines: the line detector's version, the OCR's, and its languages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OcrReading<'a> {
    pub lines_version: u32,
    pub ocr_version: u32,
    pub languages: &'a str,
}

/// The most one line's words may weigh as JSON.
pub const MAX_OCR_LINE_BYTES: usize = 256 << 10;

impl Library {
    /// The lines of a page already read by `reading`, among `lines`.
    pub fn ocr_lines(
        &self,
        blake3: &Hash,
        page: u32,
        reading: OcrReading<'_>,
        lines: &[u32],
    ) -> Result<Vec<OcrLineRecord>> {
        let Some(cache) = self.cache() else {
            return Ok(Vec::new());
        };
        cache.read(|connection| {
            let mut statement = connection.prepare_cached(
                "SELECT words FROM ocr_line
                 WHERE blake3 = ?1 AND page = ?2 AND line = ?3 AND lines_version = ?4
                   AND ocr_version = ?5 AND languages = ?6",
            )?;
            let mut found = Vec::new();
            for &line in lines {
                let words: Option<String> = statement
                    .query_row(
                        params![
                            blake3.as_slice(),
                            i64::from(page),
                            i64::from(line),
                            i64::from(reading.lines_version),
                            i64::from(reading.ocr_version),
                            reading.languages,
                        ],
                        |row| row.get(0),
                    )
                    .optional()?;
                if let Some(words) = words {
                    found.push(OcrLineRecord { line, words });
                }
            }
            Ok(found)
        })
    }

    /// Keeps what `reading` read on lines of a page, replacing what another reading had kept.
    pub fn store_ocr_lines(
        &self,
        blake3: &Hash,
        page: u32,
        reading: OcrReading<'_>,
        lines: &[OcrLineRecord],
    ) -> Result<()> {
        let Some(cache) = self.cache() else {
            return Ok(());
        };
        for record in lines {
            if record.words.len() > MAX_OCR_LINE_BYTES
                || serde_json::from_str::<serde_json::Value>(&record.words).is_err()
            {
                return Err(Error::Refused(format!("the words of line {}", record.line)));
            }
        }
        if !(2..=64).contains(&reading.languages.len())
            || reading.lines_version == 0
            || reading.ocr_version == 0
        {
            return Err(Error::Refused(
                "a reading of no version or language".to_owned(),
            ));
        }
        let blake3 = *blake3;
        let lines = lines.to_vec();
        let (lines_version, ocr_version, languages) = (
            reading.lines_version,
            reading.ocr_version,
            reading.languages.to_owned(),
        );
        cache.write(move |transaction| {
            let created_at = now_ms();
            for record in &lines {
                transaction
                    .prepare_cached(
                        "INSERT OR REPLACE INTO ocr_line (blake3, page, line, lines_version,
                             ocr_version, languages, words, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    )?
                    .execute(params![
                        blake3.as_slice(),
                        i64::from(page),
                        i64::from(record.line),
                        i64::from(lines_version),
                        i64::from(ocr_version),
                        languages,
                        record.words,
                        created_at,
                    ])?;
            }
            Ok(())
        })
    }

    /// The model of a page of the file with these bytes, if one of `version` was kept.
    pub fn line_model(
        &self,
        blake3: &Hash,
        page: u32,
        version: u32,
    ) -> Result<Option<LineModelRecord>> {
        let Some(cache) = self.cache() else {
            return Ok(None);
        };
        cache.read(|connection| {
            let row = connection
                .prepare_cached(
                    "SELECT page_hash, model FROM line_model
                     WHERE blake3 = ?1 AND page = ?2 AND version = ?3",
                )?
                .query_row(
                    params![blake3.as_slice(), i64::from(page), i64::from(version)],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)),
                )
                .optional()?;
            Ok(row.map(|(page_hash, model)| LineModelRecord {
                page,
                version,
                page_hash: page_hash as u64,
                model,
            }))
        })
    }

    /// Which of `pages` have no model of `version` yet, in the order given. Pages the file's
    /// geometry does not have are left out, since a model could not be kept for them; without a
    /// cache nothing can be kept, so nothing is missing.
    pub fn pages_without_line_model(
        &self,
        blake3: &Hash,
        pages: &[u32],
        version: u32,
    ) -> Result<Vec<u32>> {
        let Some(cache) = self.cache() else {
            return Ok(Vec::new());
        };
        cache.read(|connection| {
            let mut known = connection.prepare_cached(
                "SELECT EXISTS (SELECT 1 FROM page_geometry WHERE blake3 = ?1 AND page = ?2),
                        EXISTS (SELECT 1 FROM line_model
                                WHERE blake3 = ?1 AND page = ?2 AND version = ?3)",
            )?;
            let mut missing = Vec::new();
            for &page in pages {
                let (exists, traced): (bool, bool) = known.query_row(
                    params![blake3.as_slice(), i64::from(page), i64::from(version)],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?;
                if exists && !traced {
                    missing.push(page);
                }
            }
            Ok(missing)
        })
    }

    /// Keeps the models of some pages of the file with these bytes, replacing what each page had.
    /// Pages the geometry does not have are skipped, and without a cache this does nothing.
    pub fn store_line_models(&self, blake3: &Hash, models: &[LineModelRecord]) -> Result<()> {
        let Some(cache) = self.cache() else {
            return Ok(());
        };
        for record in models {
            if record.model.is_empty() || record.model.len() > MAX_LINE_MODEL_BYTES {
                return Err(Error::Refused(format!(
                    "a line model of {} bytes",
                    record.model.len()
                )));
            }
            if record.version == 0 {
                return Err(Error::Refused("a line model of version 0".to_owned()));
            }
        }
        let blake3 = *blake3;
        let models = models.to_vec();
        cache.write(move |transaction| {
            let created_at = now_ms();
            for record in &models {
                transaction
                    .prepare_cached(
                        "INSERT OR REPLACE INTO line_model
                             (blake3, page, version, page_hash, model, created_at)
                         SELECT ?1, ?2, ?3, ?4, ?5, ?6
                         WHERE EXISTS (SELECT 1 FROM page_geometry WHERE blake3 = ?1 AND page = ?2)",
                    )?
                    .execute(params![
                        blake3.as_slice(),
                        i64::from(record.page),
                        i64::from(record.version),
                        record.page_hash as i64,
                        record.model,
                        created_at,
                    ])?;
            }
            Ok(())
        })
    }
}

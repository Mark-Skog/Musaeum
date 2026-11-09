//! ZIP archives in memory, read with a limit on everything: entries, sizes, and what they unpack to.
//! The sizes an archive declares are only claims, so what counts is what actually comes out.

use std::collections::HashMap;
use std::io::Read;

use flate2::read::DeflateDecoder;

use crate::source::{Limits, NormalizeError, Result};

const END_RECORD_LEN: usize = 22;
const CENTRAL_RECORD_LEN: usize = 46;
const LOCAL_HEADER_LEN: usize = 30;
const ENCRYPTED: u16 = 1;

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

#[derive(Clone, Debug)]
struct Entry {
    name: String,
    method: u16,
    flags: u16,
    compressed: usize,
    uncompressed: usize,
    local_header: usize,
}

/// An archive's directory over its bytes.
pub struct Archive<'a> {
    bytes: &'a [u8],
    entries: Vec<Entry>,
    by_name: HashMap<String, usize>,
    by_lowercase: HashMap<String, usize>,
    limits: Limits,
    unpacked: std::cell::Cell<usize>,
}

impl<'a> Archive<'a> {
    pub fn open(bytes: &'a [u8], limits: Limits) -> Result<Self> {
        let tail_start = bytes
            .len()
            .saturating_sub(END_RECORD_LEN + u16::MAX as usize);
        let end = bytes[tail_start..]
            .windows(4)
            .rposition(|signature| signature == b"PK\x05\x06")
            .map(|at| tail_start + at)
            .ok_or(NormalizeError::NotThisFormat("ZIP"))?;
        let record = &bytes[end..];
        let malformed =
            || NormalizeError::Malformed("the end of the ZIP directory is cut short".to_owned());
        let directory_len = u32_at(record, 12).ok_or_else(malformed)? as usize;
        let directory_at = u32_at(record, 16).ok_or_else(malformed)? as usize;
        if directory_len == u32::MAX as usize || directory_at == u32::MAX as usize {
            return Err(NormalizeError::Unsupported("ZIP64 archives".to_owned()));
        }
        let directory = bytes
            .get(directory_at..directory_at.saturating_add(directory_len))
            .ok_or_else(|| {
                NormalizeError::Malformed("the ZIP directory is outside the file".to_owned())
            })?;
        let mut entries = Vec::new();
        let mut at = 0;
        while let Some(head) = directory.get(at..at + CENTRAL_RECORD_LEN) {
            if &head[..4] != b"PK\x01\x02" {
                break;
            }
            if entries.len() >= limits.max_files {
                return Err(NormalizeError::TooLarge(format!(
                    "more than {} files",
                    limits.max_files
                )));
            }
            let name_len = u16_at(head, 28).ok_or_else(malformed)? as usize;
            let extra_len = u16_at(head, 30).ok_or_else(malformed)? as usize;
            let comment_len = u16_at(head, 32).ok_or_else(malformed)? as usize;
            let name_at = at + CENTRAL_RECORD_LEN;
            let raw = directory
                .get(name_at..name_at + name_len)
                .ok_or_else(malformed)?;
            entries.push(Entry {
                name: String::from_utf8_lossy(raw).replace('\\', "/"),
                method: u16_at(head, 10).ok_or_else(malformed)?,
                flags: u16_at(head, 8).ok_or_else(malformed)?,
                compressed: u32_at(head, 20).ok_or_else(malformed)? as usize,
                uncompressed: u32_at(head, 24).ok_or_else(malformed)? as usize,
                local_header: u32_at(head, 42).ok_or_else(malformed)? as usize,
            });
            at = name_at + name_len + extra_len + comment_len;
        }
        let mut by_name = HashMap::new();
        let mut by_lowercase = HashMap::new();
        for (index, entry) in entries.iter().enumerate() {
            // A repeated name: the first one is the one every reader agrees on.
            by_name.entry(entry.name.clone()).or_insert(index);
            by_lowercase
                .entry(entry.name.to_lowercase())
                .or_insert(index);
        }
        Ok(Self {
            bytes,
            entries,
            by_name,
            by_lowercase,
            limits,
            unpacked: std::cell::Cell::new(0),
        })
    }

    /// Every file's name, in the order the directory lists them, directories left out.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries
            .iter()
            .map(|entry| entry.name.as_str())
            .filter(|name| !name.ends_with('/'))
    }

    /// The name of the file at `path`, exactly or, failing that, ignoring case, as a book made on
    /// Windows needs.
    pub fn find(&self, path: &str) -> Option<&str> {
        self.by_name
            .get(path)
            .or_else(|| self.by_lowercase.get(&path.to_lowercase()))
            .map(|&index| self.entries[index].name.as_str())
    }

    pub fn contains(&self, path: &str) -> bool {
        self.find(path).is_some()
    }

    /// Whether the archive says the file is encrypted with ZIP's own encryption.
    pub fn is_encrypted(&self, path: &str) -> bool {
        self.index(path)
            .is_some_and(|index| self.entries[index].flags & ENCRYPTED != 0)
    }

    fn index(&self, path: &str) -> Option<usize> {
        self.by_name
            .get(path)
            .or_else(|| self.by_lowercase.get(&path.to_lowercase()))
            .copied()
    }

    /// The declared size of a file, unpacked, which is only a claim.
    pub fn declared_size(&self, path: &str) -> Option<usize> {
        self.index(path)
            .map(|index| self.entries[index].uncompressed)
    }

    /// Reads one file.
    pub fn read(&self, path: &str) -> Result<Vec<u8>> {
        let index = self
            .index(path)
            .ok_or_else(|| NormalizeError::Malformed(format!("{path} is not in the archive")))?;
        let entry = &self.entries[index];
        if entry.flags & ENCRYPTED != 0 {
            return Err(NormalizeError::Drm);
        }
        let limit = self.limits.max_file_bytes.min(
            self.limits
                .max_total_bytes
                .saturating_sub(self.unpacked.get()),
        );
        let malformed = |what: &str| NormalizeError::Malformed(format!("{}: {what}", entry.name));
        let header = self
            .bytes
            .get(entry.local_header..entry.local_header + LOCAL_HEADER_LEN)
            .filter(|header| &header[..4] == b"PK\x03\x04")
            .ok_or_else(|| malformed("its header is not there"))?;
        let name_len = u16_at(header, 26).ok_or_else(|| malformed("a short header"))? as usize;
        let extra_len = u16_at(header, 28).ok_or_else(|| malformed("a short header"))? as usize;
        let body_at = entry.local_header + LOCAL_HEADER_LEN + name_len + extra_len;
        let body = self
            .bytes
            .get(body_at..body_at.saturating_add(entry.compressed))
            .ok_or_else(|| malformed("its data is outside the file"))?;
        let data = match entry.method {
            0 => {
                if body.len() > limit {
                    return Err(NormalizeError::TooLarge(entry.name.clone()));
                }
                body.to_vec()
            }
            8 => {
                let mut output = Vec::with_capacity(entry.uncompressed.min(limit).min(64 << 20));
                let mut decoder = DeflateDecoder::new(body).take(limit as u64 + 1);
                decoder
                    .read_to_end(&mut output)
                    .map_err(|_| malformed("it does not unpack"))?;
                if output.len() > limit {
                    return Err(NormalizeError::TooLarge(entry.name.clone()));
                }
                output
            }
            other => {
                return Err(NormalizeError::Unsupported(format!(
                    "{} is compressed with method {other}",
                    entry.name
                )));
            }
        };
        self.unpacked.set(self.unpacked.get() + data.len());
        Ok(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zipwrite::ZipWriter;

    #[test]
    fn files_read_back_and_names_match_without_case() {
        let mut zip = ZipWriter::new();
        zip.add("mimetype", b"application/epub+zip", false);
        zip.add("OEBPS/Text/Ch1.xhtml", b"<p>hello</p>", true);
        let bytes = zip.finish();
        let archive = Archive::open(&bytes, Limits::default()).unwrap();
        assert_eq!(
            archive.read("OEBPS/Text/Ch1.xhtml").unwrap(),
            b"<p>hello</p>"
        );
        assert_eq!(
            archive.find("oebps/text/ch1.xhtml"),
            Some("OEBPS/Text/Ch1.xhtml")
        );
        assert!(archive.read("missing").is_err());
    }

    #[test]
    fn a_bomb_stops_at_the_limit() {
        let mut zip = ZipWriter::new();
        zip.add("bomb", &vec![0u8; 20 << 20], true);
        let bytes = zip.finish();
        assert!(bytes.len() < 100_000);
        let limits = Limits {
            max_file_bytes: 1 << 20,
            ..Limits::default()
        };
        let archive = Archive::open(&bytes, limits).unwrap();
        assert!(matches!(
            archive.read("bomb"),
            Err(NormalizeError::TooLarge(_))
        ));
    }

    #[test]
    fn the_total_is_limited_too() {
        let mut zip = ZipWriter::new();
        for n in 0..5 {
            zip.add(&format!("f{n}"), &vec![b'x'; 1 << 20], true);
        }
        let bytes = zip.finish();
        let limits = Limits {
            max_total_bytes: 3 << 20,
            ..Limits::default()
        };
        let archive = Archive::open(&bytes, limits).unwrap();
        let read: Vec<bool> = (0..5)
            .map(|n| archive.read(&format!("f{n}")).is_ok())
            .collect();
        assert_eq!(read, [true, true, true, false, false]);
    }

    #[test]
    fn garbage_is_not_a_zip() {
        assert!(matches!(
            Archive::open(b"not a zip", Limits::default()),
            Err(NormalizeError::NotThisFormat(_))
        ));
    }
}

//! Reads bytes and interprets nothing.

use std::fs::File;
use std::io;
use std::os::windows::fs::FileExt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// A BLAKE3 hash.
pub type Hash = [u8; 32];

const HEAD: u64 = 64 * 1024;
const TAIL: u64 = 64 * 1024;
const SAMPLE: u64 = 16 * 1024;
const SAMPLES: u64 = 8;
/// Below this, the samples would overlap, so the fingerprint reads all of it.
const WHOLE_BELOW: u64 = HEAD + TAIL + SAMPLE * SAMPLES;

const HASH_BUFFER: usize = 1024 * 1024;

/// How much a piece of work has read, which is what proves that some work read nothing.
#[derive(Debug, Default)]
pub(crate) struct ReadCounters {
    pub files_opened: AtomicU64,
    pub bytes_read: AtomicU64,
}

impl ReadCounters {
    /// What has been counted so far.
    pub fn snapshot(&self) -> (u64, u64) {
        (
            self.files_opened.load(Ordering::Relaxed),
            self.bytes_read.load(Ordering::Relaxed),
        )
    }

    fn add_bytes(&self, bytes: usize) {
        self.bytes_read.fetch_add(bytes as u64, Ordering::Relaxed);
    }
}

fn read_fully(file: &File, offset: u64, buffer: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        let read = file.seek_read(&mut buffer[filled..], offset + filled as u64)?;
        if read == 0 {
            break;
        }
        filled += read;
    }
    Ok(filled)
}

/// A hash of the size, the start, the end and evenly spaced samples of a file, which reads at most
/// 256 KiB however large it is. Two files with different fingerprints have different bytes. Two
/// with the same one may still differ anywhere it did not look, so it can only name candidates.
pub(crate) fn fast_fingerprint(
    file: &File,
    size: u64,
    counters: &ReadCounters,
) -> io::Result<Hash> {
    let mut hasher = blake3::Hasher::new_derive_key("musaeum 2026-11 fast fingerprint");
    hasher.update(&size.to_le_bytes());
    let mut buffer = vec![0; HEAD.max(TAIL).max(SAMPLE) as usize];

    let mut add = |offset: u64, length: u64| -> io::Result<()> {
        let piece = &mut buffer[..length as usize];
        let read = read_fully(file, offset, piece)?;
        counters.add_bytes(read);
        hasher.update(&piece[..read]);
        Ok(())
    };

    if size < WHOLE_BELOW {
        let mut offset = 0;
        while offset < size {
            let length = (size - offset).min(HEAD);
            add(offset, length)?;
            offset += length;
        }
    } else {
        add(0, HEAD)?;
        let last_sample = size - TAIL - SAMPLE;
        for sample in 0..SAMPLES {
            let offset = HEAD + (last_sample - HEAD) * sample / (SAMPLES - 1);
            add(offset, SAMPLE)?;
        }
        add(size - TAIL, TAIL)?;
    }
    Ok(*hasher.finalize().as_bytes())
}

/// The BLAKE3 of all of a file, or `None` if `stop` was set while this was reading. On success,
/// also returns how many bytes that was.
pub(crate) fn full_hash(
    file: &File,
    stop: &AtomicBool,
    counters: &ReadCounters,
) -> io::Result<Option<(Hash, u64)>> {
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0; HASH_BUFFER];
    let mut offset = 0;
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let read = file.seek_read(&mut buffer, offset)?;
        if read == 0 {
            return Ok(Some((*hasher.finalize().as_bytes(), offset)));
        }
        counters.add_bytes(read);
        hasher.update(&buffer[..read]);
        offset += read as u64;
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Write;

    use super::*;

    struct TempFile(std::path::PathBuf);

    impl TempFile {
        fn with(name: &str, contents: &[u8]) -> Self {
            let path = std::env::temp_dir()
                .join(format!("musaeum-fingerprint-{}-{name}", std::process::id()));
            fs::File::create(&path)
                .unwrap()
                .write_all(contents)
                .unwrap();
            Self(path)
        }

        fn open(&self) -> File {
            File::open(&self.0).unwrap()
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    fn fingerprint(file: &TempFile) -> (Hash, u64) {
        let opened = file.open();
        let counters = ReadCounters::default();
        let size = opened.metadata().unwrap().len();
        let hash = fast_fingerprint(&opened, size, &counters).unwrap();
        (hash, counters.bytes_read.load(Ordering::Relaxed))
    }

    #[test]
    fn a_small_file_is_read_entirely() {
        let contents = vec![7; 100_000];
        let file = TempFile::with("small", &contents);
        assert_eq!(fingerprint(&file).1, contents.len() as u64);
    }

    #[test]
    fn a_large_file_costs_the_same_as_any_other() {
        let file = TempFile::with("large", &vec![1; 5_000_000]);
        assert_eq!(fingerprint(&file).1, HEAD + TAIL + SAMPLE * SAMPLES);
    }

    #[test]
    fn the_size_counts_even_if_no_byte_looked_at_differs() {
        let one = TempFile::with("size-a", &vec![0; 2_000_000]);
        let other = TempFile::with("size-b", &vec![0; 2_000_001]);
        assert_ne!(fingerprint(&one).0, fingerprint(&other).0);
    }

    #[test]
    fn a_change_in_the_places_it_looks_is_seen() {
        let contents = vec![9; 2_000_000];
        let original = fingerprint(&TempFile::with("base", &contents)).0;
        for (name, position) in [("start", 10), ("end", 1_999_990), ("sample", HEAD as usize)] {
            let mut changed = contents.clone();
            changed[position] = 0;
            let file = TempFile::with(name, &changed);
            assert_ne!(fingerprint(&file).0, original, "{name}");
        }
    }

    #[test]
    fn a_change_where_it_does_not_look_is_what_the_full_hash_is_for() {
        let contents = vec![5; 4_000_000];
        // Between two samples.
        let mut changed = contents.clone();
        changed[HEAD as usize + 40_000] = 0;
        let one = TempFile::with("gap-a", &contents);
        let other = TempFile::with("gap-b", &changed);
        assert_eq!(fingerprint(&one).0, fingerprint(&other).0);

        let stop = AtomicBool::new(false);
        let counters = ReadCounters::default();
        let full_one = full_hash(&one.open(), &stop, &counters).unwrap().unwrap();
        let full_other = full_hash(&other.open(), &stop, &counters).unwrap().unwrap();
        assert_ne!(full_one.0, full_other.0);
        assert_eq!(full_one.1, 4_000_000);
    }

    #[test]
    fn the_full_hash_is_plain_blake3() {
        let contents = b"the quick brown fox";
        let file = TempFile::with("plain", contents);
        let stop = AtomicBool::new(false);
        let counters = ReadCounters::default();
        let (hash, length) = full_hash(&file.open(), &stop, &counters).unwrap().unwrap();
        assert_eq!(hash, *blake3::hash(contents).as_bytes());
        assert_eq!(length, contents.len() as u64);
    }

    #[test]
    fn hashing_stops_when_told_to() {
        let file = TempFile::with("stop", &[1; 1024]);
        let stop = AtomicBool::new(true);
        let counters = ReadCounters::default();
        assert_eq!(full_hash(&file.open(), &stop, &counters).unwrap(), None);
    }

    #[test]
    fn an_empty_file_has_a_fingerprint() {
        let file = TempFile::with("empty", b"");
        assert_eq!(fingerprint(&file).1, 0);
    }
}

//! The little of ZIP an EPUB's metadata needs: its directory and two small files inside it.

use std::io::Read;

use flate2::read::DeflateDecoder;

use super::{ParseError, ReadAt, read_up_to};

const END_RECORD_LEN: usize = 22;
const MAX_COMMENT_LEN: usize = u16::MAX as usize;
const CENTRAL_RECORD_LEN: usize = 46;
const LOCAL_HEADER_LEN: usize = 30;
/// Far more than the 64 KiB files a real EPUB has times the length of a long path.
const MAX_DIRECTORY_BYTES: u64 = 16 * 1024 * 1024;

const STORED: u16 = 0;
const DEFLATED: u16 = 8;
const ENCRYPTED: u16 = 1;

/// One file in the archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Entry {
    pub name: Vec<u8>,
    method: u16,
    flags: u16,
    compressed: u32,
    local_header: u32,
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// Reads the archive's directory. ZIP64 is left out: it is for archives over 4 GiB, and the one
/// consequence of leaving it out is that such a book has no metadata from here.
pub(super) fn read_directory(source: &(impl ReadAt + ?Sized)) -> Result<Vec<Entry>, ParseError> {
    let size = source.size()?;
    let tail_len = (END_RECORD_LEN + MAX_COMMENT_LEN).min(size as usize);
    let tail = read_up_to(source, size - tail_len as u64, tail_len)?;
    let end = tail
        .windows(4)
        .rposition(|signature| signature == b"PK\x05\x06")
        .ok_or(ParseError::NotThisFormat("ZIP"))?;
    let record = &tail[end..];
    let malformed = || ParseError::Malformed("the end of the ZIP directory is cut short");
    let directory_len = u32_at(record, 12).ok_or_else(malformed)?;
    let directory_at = u32_at(record, 16).ok_or_else(malformed)?;
    if directory_len == u32::MAX || directory_at == u32::MAX {
        return Err(ParseError::Malformed("ZIP64 is not read here"));
    }
    if u64::from(directory_len) > MAX_DIRECTORY_BYTES {
        return Err(ParseError::TooLarge("the ZIP directory"));
    }
    let directory = read_up_to(source, u64::from(directory_at), directory_len as usize)?;
    parse_directory(&directory)
}

fn parse_directory(directory: &[u8]) -> Result<Vec<Entry>, ParseError> {
    let mut entries = Vec::new();
    let mut at = 0;
    while let Some(record) = directory.get(at..at + CENTRAL_RECORD_LEN) {
        if &record[..4] != b"PK\x01\x02" {
            break;
        }
        let malformed = || ParseError::Malformed("a ZIP directory record is cut short");
        let name_len = u16_at(record, 28).ok_or_else(malformed)? as usize;
        let extra_len = u16_at(record, 30).ok_or_else(malformed)? as usize;
        let comment_len = u16_at(record, 32).ok_or_else(malformed)? as usize;
        let name_at = at + CENTRAL_RECORD_LEN;
        let name = directory
            .get(name_at..name_at + name_len)
            .ok_or_else(malformed)?;
        entries.push(Entry {
            name: name.to_vec(),
            method: u16_at(record, 10).ok_or_else(malformed)?,
            flags: u16_at(record, 8).ok_or_else(malformed)?,
            compressed: u32_at(record, 20).ok_or_else(malformed)?,
            local_header: u32_at(record, 42).ok_or_else(malformed)?,
        });
        at = name_at + name_len + extra_len + comment_len;
    }
    Ok(entries)
}

/// Reads one file out of the archive, refusing anything that unpacks to more than `limit`. The
/// sizes in the archive are only claims, so this counts what actually comes out.
pub(super) fn read_entry(
    source: &(impl ReadAt + ?Sized),
    entry: &Entry,
    limit: usize,
) -> Result<Vec<u8>, ParseError> {
    if entry.flags & ENCRYPTED != 0 {
        return Err(ParseError::Malformed("an encrypted ZIP entry"));
    }
    if entry.compressed as usize > limit + 1024 {
        return Err(ParseError::TooLarge("a ZIP entry"));
    }
    let header_at = u64::from(entry.local_header);
    let header = read_up_to(source, header_at, LOCAL_HEADER_LEN)?;
    if header.len() < LOCAL_HEADER_LEN || &header[..4] != b"PK\x03\x04" {
        return Err(ParseError::Malformed("a ZIP entry's header is not there"));
    }
    let malformed = || ParseError::Malformed("a ZIP entry's header is cut short");
    let name_len = u64::from(u16_at(&header, 26).ok_or_else(malformed)?);
    let extra_len = u64::from(u16_at(&header, 28).ok_or_else(malformed)?);
    let body_at = header_at + LOCAL_HEADER_LEN as u64 + name_len + extra_len;
    let body = read_up_to(source, body_at, entry.compressed as usize)?;

    match entry.method {
        STORED if body.len() <= limit => Ok(body),
        STORED => Err(ParseError::TooLarge("a ZIP entry")),
        DEFLATED => {
            let mut output = Vec::new();
            let mut decoder = DeflateDecoder::new(body.as_slice()).take(limit as u64 + 1);
            decoder
                .read_to_end(&mut output)
                .map_err(|_| ParseError::Malformed("a ZIP entry does not unpack"))?;
            if output.len() > limit {
                return Err(ParseError::TooLarge("what a ZIP entry unpacks to"));
            }
            Ok(output)
        }
        _ => Err(ParseError::Malformed("a ZIP entry with an unknown method")),
    }
}

#[cfg(test)]
pub(super) mod test_support {
    use std::io::Write;

    use flate2::Compression;
    use flate2::write::DeflateEncoder;

    /// Builds an archive. Each entry is a name, its bytes, and whether to compress them.
    pub fn archive(entries: &[(&str, &[u8], bool)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data, compress) in entries {
            let at = out.len() as u32;
            let (method, body) = if *compress {
                let mut encoder = DeflateEncoder::new(Vec::new(), Compression::new(6));
                encoder.write_all(data).unwrap();
                (8u16, encoder.finish().unwrap())
            } else {
                (0u16, data.to_vec())
            };
            let mut fields = Vec::new();
            fields.extend_from_slice(&20u16.to_le_bytes());
            fields.extend_from_slice(&0u16.to_le_bytes());
            fields.extend_from_slice(&method.to_le_bytes());
            fields.extend_from_slice(&[0; 8]);
            fields.extend_from_slice(&(body.len() as u32).to_le_bytes());
            fields.extend_from_slice(&(data.len() as u32).to_le_bytes());
            fields.extend_from_slice(&(name.len() as u16).to_le_bytes());
            fields.extend_from_slice(&0u16.to_le_bytes());

            central.extend_from_slice(b"PK\x01\x02");
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&fields);
            central.extend_from_slice(&[0; 10]);
            central.extend_from_slice(&at.to_le_bytes());
            central.extend_from_slice(name.as_bytes());

            out.extend_from_slice(b"PK\x03\x04");
            out.extend_from_slice(&fields);
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&body);
        }
        let at = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(b"PK\x05\x06");
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&at.to_le_bytes());
        out.extend_from_slice(&[0; 2]);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::archive;
    use super::*;

    #[test]
    fn stored_and_deflated_entries_read_back() {
        let bytes = archive(&[("a.txt", b"hello", false), ("b.txt", b"world world", true)]);
        let entries = read_directory(bytes.as_slice()).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, b"a.txt");
        assert_eq!(
            read_entry(bytes.as_slice(), &entries[0], 100).unwrap(),
            b"hello"
        );
        assert_eq!(
            read_entry(bytes.as_slice(), &entries[1], 100).unwrap(),
            b"world world"
        );
    }

    #[test]
    fn a_bomb_stops_at_the_limit() {
        let bomb = vec![0u8; 50_000_000];
        let bytes = archive(&[("bomb.txt", &bomb, true)]);
        assert!(bytes.len() < 100_000, "it has to be a small file");
        let entries = read_directory(bytes.as_slice()).unwrap();
        assert!(matches!(
            read_entry(bytes.as_slice(), &entries[0], 1_000_000),
            Err(ParseError::TooLarge(_))
        ));
    }

    #[test]
    fn something_else_is_refused() {
        assert!(matches!(
            read_directory(b"not a zip at all".as_slice()),
            Err(ParseError::NotThisFormat("ZIP"))
        ));
        assert!(matches!(
            read_directory(b"".as_slice()),
            Err(ParseError::NotThisFormat("ZIP"))
        ));
    }

    #[test]
    fn lies_in_the_directory_do_no_harm() {
        let mut bytes = archive(&[("a.txt", b"hello", false)]);
        let end = bytes.len() - END_RECORD_LEN;
        // A directory claimed to be enormous.
        bytes[end + 12..end + 16].copy_from_slice(&0x7FFF_FFFFu32.to_le_bytes());
        assert!(matches!(
            read_directory(bytes.as_slice()),
            Err(ParseError::TooLarge(_))
        ));
        // A directory claimed to be elsewhere.
        bytes[end + 12..end + 16].copy_from_slice(&100u32.to_le_bytes());
        bytes[end + 16..end + 20].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(read_directory(bytes.as_slice()).is_err());
    }

    #[test]
    fn a_name_longer_than_the_directory_is_refused() {
        let mut bytes = archive(&[("a.txt", b"hello", false)]);
        let start = bytes.windows(4).position(|s| s == b"PK\x01\x02").unwrap();
        bytes[start + 28..start + 30].copy_from_slice(&u16::MAX.to_le_bytes());
        assert!(read_directory(bytes.as_slice()).is_err());
    }

    #[test]
    fn an_entry_pointing_outside_the_file_is_refused() {
        let bytes = archive(&[("a.txt", b"hello", false)]);
        let mut entries = read_directory(bytes.as_slice()).unwrap();
        entries[0].local_header = u32::MAX - 10;
        assert!(read_entry(bytes.as_slice(), &entries[0], 100).is_err());
    }

    #[test]
    fn a_claimed_size_beyond_the_limit_reads_nothing() {
        let bytes = archive(&[("a.txt", b"hello", false)]);
        let mut entries = read_directory(bytes.as_slice()).unwrap();
        entries[0].compressed = u32::MAX;
        assert!(matches!(
            read_entry(bytes.as_slice(), &entries[0], 100),
            Err(ParseError::TooLarge(_))
        ));
    }

    #[test]
    fn a_corrupt_stream_is_an_error_and_not_a_panic() {
        let mut bytes = archive(&[("a.txt", b"hello hello hello", true)]);
        let body = 30 + 5;
        for byte in &mut bytes[body..body + 8] {
            *byte = 0xFF;
        }
        let entries = read_directory(bytes.as_slice()).unwrap();
        assert!(read_entry(bytes.as_slice(), &entries[0], 100).is_err());
    }
}

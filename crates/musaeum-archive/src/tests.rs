use gen_fixtures::comics::{ArchiveEntry, comic_entries, rar4_bytes, rar5_bytes, zip_bytes};

use super::*;

fn files(entries: &[ArchiveEntry]) -> Vec<&ArchiveEntry> {
    entries
        .iter()
        .filter(|entry| !entry.name.ends_with('/'))
        .collect()
}

/// Reads every entry and its data, as a list of names and contents.
fn read_everything(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, ArchiveError> {
    let mut archive = Archive::open(bytes)?;
    let mut out = Vec::new();
    while let Some(entry) = archive.next_entry()? {
        if entry.is_file {
            out.push((entry.name, archive.read_all(1 << 24)?));
        }
    }
    Ok(out)
}

#[test]
fn every_format_yields_the_same_files_in_stored_order() {
    let entries = comic_entries(true).unwrap();
    let expected: Vec<(String, Vec<u8>)> = files(&entries)
        .into_iter()
        .map(|entry| (entry.name.clone(), entry.data.clone()))
        .collect();

    for (format, bytes) in [
        ("deflated ZIP", zip_bytes(&entries, true).unwrap()),
        ("stored ZIP", zip_bytes(&entries, false).unwrap()),
        ("RAR 4", rar4_bytes(&entries)),
        ("RAR 5", rar5_bytes(&entries)),
    ] {
        assert_eq!(
            read_everything(&bytes).unwrap(),
            expected,
            "{format} did not read back"
        );
    }
}

#[test]
fn entries_report_kind_size_and_format() {
    let entries = comic_entries(false).unwrap();
    let bytes = zip_bytes(&entries, true).unwrap();
    let mut archive = Archive::open(&bytes).unwrap();
    let first = archive.next_entry().unwrap().unwrap();
    assert_eq!(first.name, "extras/");
    assert!(!first.is_file, "a directory is not a file");
    let second = archive.next_entry().unwrap().unwrap();
    assert_eq!((second.index, second.name.as_str()), (1, "notes.txt"));
    assert_eq!(second.size, Some(entries[1].data.len() as u64));
    assert!(second.is_file && !second.encrypted);
    assert!(
        archive.format_name().starts_with("ZIP"),
        "{}",
        archive.format_name()
    );

    let rar = rar5_bytes(&entries);
    let mut archive = Archive::open(&rar).unwrap();
    archive.next_entry().unwrap();
    assert_eq!(archive.format_name(), "RAR5");
}

#[test]
fn a_prefix_or_a_limit_reads_no_more_than_asked() {
    let entries = comic_entries(false).unwrap();
    let bytes = zip_bytes(&entries, true).unwrap();
    let mut archive = Archive::open(&bytes).unwrap();
    archive.next_entry().unwrap();
    archive.next_entry().unwrap();
    assert_eq!(archive.read_prefix(7).unwrap(), b"Scanned".to_vec());

    let mut archive = Archive::open(&bytes).unwrap();
    archive.next_entry().unwrap();
    archive.next_entry().unwrap();
    assert_eq!(archive.read_all(4), Err(ArchiveError::TooLarge(4)));
    // The reader goes on to the next entry regardless of what was left unread.
    assert_eq!(archive.next_entry().unwrap().unwrap().name, "p10.png");
}

#[test]
fn damage_and_strangers_are_errors_not_crashes() {
    let entries = comic_entries(false).unwrap();
    for bytes in [
        zip_bytes(&entries, true).unwrap(),
        rar4_bytes(&entries),
        rar5_bytes(&entries),
    ] {
        let whole = read_everything(&bytes).unwrap();
        for cut in [bytes.len() / 3, bytes.len() / 2, bytes.len() - 20] {
            // A RAR cut inside a header reads as if it ended before that header; anything else
            // is an error. Never does a cut archive read whole.
            match read_everything(&bytes[..cut]) {
                Err(_) => {}
                Ok(read) => assert!(read.len() < whole.len(), "cut at {cut} read whole"),
            }
        }
        let mut flipped = bytes.clone();
        let middle = flipped.len() / 2;
        for byte in &mut flipped[middle..middle + 64] {
            *byte ^= 0x5A;
        }
        // A flipped byte in stored data only fails the checksum; either way, nothing crashes.
        let _ = read_everything(&flipped);
    }
    assert!(Archive::open(b"%PDF-1.7 not an archive").is_err());
    assert!(Archive::open(&[]).is_err());
}

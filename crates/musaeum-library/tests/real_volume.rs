//! Changing volume for real, on a real second volume.
//!
//! No administrator rights are needed to mount a disc image, and with no drive letter it does not
//! show up anywhere. That gives a second volume with a serial number of its own, a different file
//! system, and a copy of the book as the only way to get there, which is what moving across volumes
//! is. It is read-only, which makes no difference to what the library does with it.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::{Fixture, pdf};
use musaeum_library::InstanceState;

const SECTOR: usize = 2048;
const ROOT_SECTOR: usize = 20;
const FILE_SECTOR: usize = 21;

fn both_32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    bytes[at + 4..at + 8].copy_from_slice(&value.to_be_bytes());
}

fn both_16(bytes: &mut [u8], at: usize, value: u16) {
    bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
    bytes[at + 2..at + 4].copy_from_slice(&value.to_be_bytes());
}

/// One ISO 9660 directory record, returning how long it is.
fn directory_record(
    bytes: &mut [u8],
    at: usize,
    name: &[u8],
    sector: usize,
    length: usize,
    is_directory: bool,
) -> usize {
    let padded = name.len() + (name.len() + 1) % 2;
    let record = 33 + padded;
    bytes[at] = record as u8;
    both_32(bytes, at + 2, sector as u32);
    both_32(bytes, at + 10, length as u32);
    // 2020-01-01 12:00:00 UTC, so that the date does not depend on when this runs.
    bytes[at + 18..at + 25].copy_from_slice(&[120, 1, 1, 12, 0, 0, 0]);
    bytes[at + 25] = if is_directory { 2 } else { 0 };
    both_16(bytes, at + 28, 1);
    bytes[at + 32] = name.len() as u8;
    bytes[at + 33..at + 33 + name.len()].copy_from_slice(name);
    record
}

/// A disc image with one file on it, called `BOOK.PDF`.
fn iso_with(contents: &[u8]) -> Vec<u8> {
    let sectors = FILE_SECTOR + contents.len().div_ceil(SECTOR);
    let mut bytes = vec![0; sectors * SECTOR];

    let descriptor = 16 * SECTOR;
    bytes[descriptor] = 1;
    bytes[descriptor + 1..descriptor + 6].copy_from_slice(b"CD001");
    bytes[descriptor + 6] = 1;
    bytes[descriptor + 8..descriptor + 72].fill(b' ');
    both_32(&mut bytes, descriptor + 80, sectors as u32);
    both_16(&mut bytes, descriptor + 120, 1);
    both_16(&mut bytes, descriptor + 124, 1);
    both_16(&mut bytes, descriptor + 128, SECTOR as u16);
    both_32(&mut bytes, descriptor + 132, 10);
    bytes[descriptor + 140..descriptor + 144].copy_from_slice(&18u32.to_le_bytes());
    bytes[descriptor + 148..descriptor + 152].copy_from_slice(&19u32.to_be_bytes());
    directory_record(
        &mut bytes,
        descriptor + 156,
        &[0],
        ROOT_SECTOR,
        SECTOR,
        true,
    );
    bytes[descriptor + 881] = 1;

    let terminator = 17 * SECTOR;
    bytes[terminator] = 255;
    bytes[terminator + 1..terminator + 6].copy_from_slice(b"CD001");
    bytes[terminator + 6] = 1;

    let little_endian_table = 18 * SECTOR;
    bytes[little_endian_table] = 1;
    bytes[little_endian_table + 2..little_endian_table + 6]
        .copy_from_slice(&(ROOT_SECTOR as u32).to_le_bytes());
    bytes[little_endian_table + 6] = 1;
    let big_endian_table = 19 * SECTOR;
    bytes[big_endian_table] = 1;
    bytes[big_endian_table + 2..big_endian_table + 6]
        .copy_from_slice(&(ROOT_SECTOR as u32).to_be_bytes());
    bytes[big_endian_table + 7] = 1;

    let root = ROOT_SECTOR * SECTOR;
    let mut at = root;
    at += directory_record(&mut bytes, at, &[0], ROOT_SECTOR, SECTOR, true);
    at += directory_record(&mut bytes, at, &[1], ROOT_SECTOR, SECTOR, true);
    directory_record(
        &mut bytes,
        at,
        b"BOOK.PDF;1",
        FILE_SECTOR,
        contents.len(),
        false,
    );

    bytes[FILE_SECTOR * SECTOR..FILE_SECTOR * SECTOR + contents.len()].copy_from_slice(contents);
    bytes
}

/// A mounted image, without a drive letter, which is taken down at the end whatever happens.
struct Mounted {
    image: PathBuf,
    volume: Option<PathBuf>,
}

impl Mounted {
    fn new(image: &Path) -> Self {
        let mut mounted = Self {
            image: image.to_owned(),
            volume: None,
        };
        mounted.mount();
        mounted
    }

    fn mount(&mut self) {
        let script = format!(
            "(Mount-DiskImage -ImagePath '{}' -StorageType ISO -NoDriveLetter -PassThru | Get-Volume).Path",
            self.image.display()
        );
        let output = powershell(&script);
        let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        assert!(
            path.starts_with(r"\\?\Volume{"),
            "mounting a disc image did not give a volume: {path} {}",
            String::from_utf8_lossy(&output.stderr)
        );
        self.volume = Some(PathBuf::from(path));
    }

    fn dismount(&mut self) {
        if self.volume.take().is_some() {
            powershell(&format!(
                "Dismount-DiskImage -ImagePath '{}' | Out-Null",
                self.image.display()
            ));
        }
    }

    fn volume(&self) -> &Path {
        self.volume.as_deref().expect("it is mounted")
    }
}

impl Drop for Mounted {
    fn drop(&mut self) {
        self.dismount();
    }
}

fn powershell(script: &str) -> std::process::Output {
    Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        .unwrap()
}

#[test]
fn a_book_that_moves_to_a_real_second_volume_keeps_its_identity() {
    let fixture = Fixture::new();
    let contents = pdf(1);
    let path = fixture.write("book.pdf", &contents);
    fixture.settle();
    let identity = fixture.mark_as_cared_for(&path);
    let first_volume = musaeum_fs::stat(&path).unwrap().volume;

    let image = fixture.dir.0.join("second-volume.iso");
    fs::write(&image, iso_with(&contents)).unwrap();
    let mut mounted = Mounted::new(&image);
    let on_second_volume = mounted.volume().join("BOOK.PDF");

    let stat = musaeum_fs::stat(&on_second_volume).unwrap();
    assert_ne!(stat.volume, first_volume, "it really is another volume");
    assert_eq!(stat.size, contents.len() as u64);
    let listing = musaeum_fs::list_directory(mounted.volume()).unwrap();
    assert_eq!(listing.entries.len(), 1);
    assert_eq!(listing.volume, stat.volume);
    println!(
        "second volume: serial {:#x}, File IDs {}",
        stat.volume,
        if stat.file_id.is_some() {
            "reported"
        } else {
            "not reported"
        },
    );

    // The rest of moving a file across volumes.
    fs::remove_file(&path).unwrap();

    let second_root = fixture.library().add_root(mounted.volume(), &[]).unwrap();
    fixture.library().reconcile_all().unwrap();
    let content = fixture.process();

    assert_eq!(content.reattached, 1, "recognized by its full hash");
    assert_eq!(fixture.identity(&on_second_volume), identity);
    assert_eq!(fixture.state_of(&on_second_volume), InstanceState::Present);
    assert_eq!(fixture.document_count(), 1);
    fixture.assert_still_cared_for(&identity);
    assert_eq!(fixture.library().status().unwrap().absent, 0);

    // A disc has no Recycle Bin, and on a volume without one, deleting could not be undone.
    let refused = fixture.library().recycle_file(identity.instance);
    assert!(
        matches!(refused, Err(musaeum_library::Error::Refused(_))),
        "{refused:?}"
    );
    assert!(on_second_volume.exists());

    // A second look finds nothing to do, even if this file system has no lasting IDs.
    let again = fixture
        .library()
        .reconcile(second_root.id, &musaeum_library::ReconcileScope::Everything)
        .unwrap();
    assert_eq!(again.unchanged, 1);
    assert_eq!(again.bytes_read, 0);

    // Unplugged: not there for now, which is not the same as gone.
    mounted.dismount();
    let unplugged = fixture
        .library()
        .reconcile(second_root.id, &musaeum_library::ReconcileScope::Everything)
        .unwrap();
    assert!(!unplugged.root_available);
    assert_eq!(fixture.identity(&on_second_volume), identity);
    assert_eq!(
        fixture.state_of(&on_second_volume),
        InstanceState::Present,
        "unplugging a disk says nothing about the files on it"
    );
    fixture.assert_still_cared_for(&identity);
}

#[test]
fn the_image_this_test_builds_is_what_it_says() {
    let contents = b"%PDF-1.7 contents";
    let image = iso_with(contents);
    assert_eq!(&image[16 * SECTOR + 1..16 * SECTOR + 6], b"CD001");
    assert_eq!(image.len() % SECTOR, 0);
    assert_eq!(
        &image[FILE_SECTOR * SECTOR..FILE_SECTOR * SECTOR + contents.len()],
        contents
    );
}

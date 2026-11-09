//! The isolated worker, for real: in its sandbox, and when it goes wrong.

mod common;

use std::fs::File;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::{Fixture, TempDir, epub, ingest_config, options, pdf, pdf_with_id};
use musaeum_ingest::protocol::{Format, PROTOCOL_VERSION, Reply, Request, read_frame, write_frame};
use musaeum_library::{IngestConfig, LibraryOptions};
use musaeum_sandbox::{Policy, SpawnConfig, spawn};
use musaeum_shm::region::duplicate_file_read_only;
use std::os::windows::io::AsHandle;

#[test]
fn the_worker_runs_with_everything_locked_and_still_reads_a_book() {
    let dir = TempDir::new();
    let path = dir.0.join("book.epub");
    std::fs::write(&path, epub("Um Título", &["Alguém"], Some("urn:x:1"), 1)).unwrap();

    let exe = PathBuf::from(env!("CARGO_BIN_EXE_musaeum-ingest"));
    let mut child = spawn(SpawnConfig {
        exe: &exe,
        args: &[],
        policy: Policy::LockdownParser,
        memory_limit: Some(128 * 1024 * 1024),
    })
    .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();

    let ready: Reply = read_frame(&mut stdout).unwrap().unwrap();
    assert_eq!(
        ready,
        Reply::Ready {
            protocol: PROTOCOL_VERSION
        }
    );

    assert!(
        !child.is_impersonating().unwrap(),
        "it dropped its startup token"
    );
    let report = child.report().unwrap();
    assert!(
        report.mitigations.win32k_disabled,
        "no win32k: no kernel window code to attack"
    );
    assert!(report.mitigations.dynamic_code_prohibited);
    assert!(report.mitigations.microsoft_signed_only);
    assert!(report.mitigations.strict_handle_checks);
    assert!(report.mitigations.control_flow_guard);
    assert!(report.mitigations.image_load_restricted);

    let file = File::open(&path).unwrap();
    let handle = duplicate_file_read_only(&file, child.as_handle()).unwrap();
    write_frame(
        &mut stdin,
        &Request {
            handle,
            format: Format::Epub,
            task: musaeum_ingest::protocol::Task::Extract,
        },
    )
    .unwrap();
    let reply: Reply = read_frame(&mut stdout).unwrap().unwrap();
    let Reply::Extracted(extracted) = reply else {
        panic!("{reply:?}");
    };
    assert_eq!(extracted.title.as_deref(), Some("Um Título"));
    assert_eq!(extracted.dc_identifier.as_deref(), Some("urn:x:1"));
}

/// A library whose worker is the one that misbehaves in this way.
fn with_misbehaving_worker(behaviour: &str) -> Fixture {
    // The library starts its worker with no arguments, so this one goes by its own file name.
    let directory = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("misbehave");
    std::fs::create_dir_all(&directory).unwrap();
    let copy = directory.join(format!("{behaviour}.exe"));
    std::fs::copy(env!("CARGO_BIN_EXE_ingest-misbehave"), &copy).unwrap();
    Fixture::with_options(LibraryOptions {
        ingest: IngestConfig {
            exe: Some(copy),
            policy: Policy::Unrestricted,
            timeout: Duration::from_millis(1500),
            ..IngestConfig::default()
        },
        content_threads: 1,
        ..options()
    })
}

#[test]
fn a_worker_that_crashes_on_a_book_costs_that_book_its_metadata_and_nothing_else() {
    let fixture = with_misbehaving_worker("crash");
    let first = fixture.write("first.pdf", &pdf_with_id("aa11", 1));
    let second = fixture.write("second.pdf", &pdf_with_id("bb22", 2));

    let (_, content) = fixture.settle();

    assert_eq!(content.hashed, 2, "both books still got their hash");
    assert_eq!(content.extraction_failed, 2);
    assert_eq!(content.worker_restarts, 2, "a new worker each time");
    assert!(fixture.document_at(&first).revisions[0].blake3.is_some());
    assert!(fixture.document_at(&second).revisions[0].blake3.is_some());

    let (_, content) = fixture.settle();
    assert_eq!(
        content.extraction_failed, 0,
        "the same bytes are not tried again"
    );
}

#[test]
fn a_worker_that_exits_is_the_same_as_one_that_crashes() {
    let fixture = with_misbehaving_worker("exit");
    let path = fixture.write("book.pdf", &pdf(1));
    let (_, content) = fixture.settle();
    assert_eq!(content.hashed, 1);
    assert_eq!(content.worker_restarts, 1);
    assert!(fixture.document_at(&path).revisions[0].blake3.is_some());
}

#[test]
fn a_worker_that_hangs_is_killed_after_its_time() {
    let fixture = with_misbehaving_worker("hang");
    let path = fixture.write("book.pdf", &pdf(1));

    let started = Instant::now();
    let (_, content) = fixture.settle();
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "took {:?}",
        started.elapsed()
    );

    assert_eq!(content.hashed, 1);
    assert_eq!(content.extraction_failed, 1);
    assert_eq!(content.worker_restarts, 1);
    assert!(fixture.document_at(&path).revisions[0].blake3.is_some());
}

#[test]
fn a_worker_that_talks_nonsense_is_replaced() {
    for behaviour in ["garbage", "huge-frame", "out-of-turn"] {
        let fixture = with_misbehaving_worker(behaviour);
        let path = fixture.write("book.pdf", &pdf(1));
        let (_, content) = fixture.settle();
        assert_eq!(content.hashed, 1, "{behaviour}");
        assert_eq!(content.extraction_failed, 1, "{behaviour}");
        assert_eq!(content.worker_restarts, 1, "{behaviour}");
        assert!(
            fixture.document_at(&path).revisions[0].blake3.is_some(),
            "{behaviour}"
        );
    }
}

#[test]
fn what_a_lying_worker_says_is_cut_down_to_what_a_field_can_be() {
    let fixture = with_misbehaving_worker("lie");
    let path = fixture.write("book.epub", &epub("Título Verdadeiro", &[], None, 1));
    let (_, content) = fixture.settle();
    assert_eq!(
        content.extracted, 1,
        "it answered, so it counts as an answer"
    );

    let document = fixture.document_at(&path);
    assert_eq!(document.pdf_id0, None, "<script> is not hexadecimal");
    assert_eq!(document.isbn, None);
    assert!(document.dc_identifier.unwrap().chars().count() <= 512);
    let work = fixture.library().work(document.work_id).unwrap();
    assert!(work.title.chars().count() <= 2048);
    assert!(!work.title.contains('\u{0}'), "no control characters");
    assert!(work.creators.len() <= 64);
    assert!(work.language.unwrap().chars().count() <= 64);
    assert_eq!(work.year, None, "99999 is not a year");
}

#[test]
fn a_worker_that_never_starts_leaves_the_book_to_be_read_later() {
    for behaviour in ["never-ready", "wrong-protocol"] {
        let fixture = with_misbehaving_worker(behaviour);
        fixture.write("book.pdf", &pdf(1));
        let (_, content) = fixture.settle();
        assert_eq!(
            content.hashed, 1,
            "{behaviour}: hashing does not depend on it"
        );
        assert_eq!(
            content.extraction_failed, 0,
            "{behaviour}: no conclusion about the file"
        );
    }
}

#[test]
fn with_no_worker_at_all_the_library_works_and_catches_up_later() {
    let mut fixture = Fixture::with_options(LibraryOptions {
        ingest: IngestConfig {
            exe: Some(PathBuf::from(r"C:\does\not\exist.exe")),
            ..IngestConfig::default()
        },
        ..options()
    });
    let path = fixture.write("book.epub", &epub("Título de Dentro", &[], None, 1));
    let (_, content) = fixture.settle();
    assert_eq!(content.hashed, 1);
    assert_eq!(fixture.document_at(&path).title, "book");

    // The next time the program runs, the worker is where it should be.
    fixture.close();
    fixture.options.ingest = ingest_config();
    fixture.reopen();
    let (_, content) = fixture.settle();
    assert_eq!(content.hashed, 0, "already done");
    assert_eq!(content.extracted, 1);
    assert_eq!(fixture.document_at(&path).title, "Título de Dentro");
}

#[test]
fn a_hostile_book_does_not_get_past_the_limits_of_the_parser() {
    let fixture = Fixture::new();
    // What an attacker controls: everything. None of this may stop the worker from reading the next
    // book, or take it past the limits of its job.
    let bomb = {
        let mut bytes = Vec::new();
        {
            use std::io::Write;
            let mut encoder =
                flate2::write::DeflateEncoder::new(&mut bytes, flate2::Compression::new(9));
            encoder.write_all(&vec![b'x'; 200_000_000]).unwrap();
            encoder.finish().unwrap();
        }
        zip_with_deflated_package(&bytes, 200_000_000)
    };
    fixture.write("bomb.epub", &bomb);
    fixture.write("truncated.epub", &epub("X", &[], None, 1)[..50]);
    fixture.write("not-a-pdf.pdf", b"not a pdf");
    let good = fixture.write("good.epub", &epub("Bom", &["Alguém"], None, 2));

    let started = Instant::now();
    let (_, content) = fixture.settle();
    assert!(started.elapsed() < Duration::from_secs(60));

    assert_eq!(content.hashed, 4);
    assert_eq!(content.worker_restarts, 0, "refused, not crashed");
    assert_eq!(content.extraction_failed, 3);
    assert_eq!(fixture.document_at(&good).title, "Bom");
}

/// An EPUB whose package document is a block of Deflate that unpacks to far more than it says.
fn zip_with_deflated_package(deflated: &[u8], unpacked: u32) -> Vec<u8> {
    let container =
        br#"<container><rootfiles><rootfile full-path="p.opf"/></rootfiles></container>"#;
    let mut out = Vec::new();
    let mut central = Vec::new();
    let mut entry = |name: &str, method: u16, body: &[u8], size: u32| {
        let at = out.len() as u32;
        let mut fields = Vec::new();
        fields.extend_from_slice(&20u16.to_le_bytes());
        fields.extend_from_slice(&0u16.to_le_bytes());
        fields.extend_from_slice(&method.to_le_bytes());
        fields.extend_from_slice(&[0; 8]);
        fields.extend_from_slice(&(body.len() as u32).to_le_bytes());
        fields.extend_from_slice(&size.to_le_bytes());
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
        out.extend_from_slice(body);
    };
    entry(
        "META-INF/container.xml",
        0,
        container,
        container.len() as u32,
    );
    entry("p.opf", 8, deflated, unpacked);
    let at = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(b"PK\x05\x06");
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&at.to_le_bytes());
    out.extend_from_slice(&[0; 2]);
    out
}

//! Normalizing in the real worker, in its strictest sandbox, and keeping the result by hash.

mod common;

use std::fs::File;

use common::{TempDir, ingest_config};
use gen_fixtures::flow;
use musaeum_flow::package::MANIFEST_PATH;
use musaeum_ingest::protocol::Format;
use musaeum_library::{
    CoverOutcome, FlowStore, ImageKind, IngestClient, IngestConfig, NormalizeOutcome,
    check_normalized,
};

fn client() -> IngestClient {
    IngestClient::new(IngestConfig {
        ..IngestConfig {
            timeout: IngestConfig::for_normalizing().timeout,
            memory_limit: IngestConfig::for_normalizing().memory_limit,
            ..ingest_config()
        }
    })
}

#[test]
fn every_format_is_normalized_in_the_sandbox_and_kept_by_hash() {
    let dir = TempDir::new();
    let short = flow::book(false);
    let long = flow::book(true);
    let cases: Vec<(&str, Format, Vec<u8>)> = vec![
        ("book.epub", Format::Epub, flow::epub3(&long)),
        ("old.epub", Format::Epub, flow::epub2(&short)),
        ("book.mobi", Format::Mobi, flow::mobi6(&short)),
        ("book.azw3", Format::Azw3, flow::kf8(&short)),
        ("book.fb2", Format::Fb2, flow::fb2(&short, "utf-8")),
        ("book.txt", Format::Txt, flow::txt(&long, "windows-1252")),
        ("book.md", Format::Markdown, flow::markdown(&short)),
    ];
    let store = FlowStore::new(&dir.0.join("cache"));
    let mut client = client();
    let mut worker = None;
    for (index, (name, format, bytes)) in cases.into_iter().enumerate() {
        let path = dir.0.join(name);
        std::fs::write(&path, &bytes).unwrap();
        let file = File::open(&path).unwrap();
        let normalized = match client.normalize(&file, format) {
            NormalizeOutcome::Normalized(normalized) => normalized,
            other => panic!("{name}: {other:?}"),
        };
        // One worker for all of them: nothing made it restart.
        let current = client.current_worker();
        assert!(current.is_some());
        if let Some(previous) = worker {
            assert_eq!(current, Some(previous), "{name} replaced the worker");
        }
        worker = current;
        assert!(normalized.manifest.stats.documents > 0, "{name}");
        // The same handle again: the worker reads by position, whatever the first request left.
        match client.normalize(&file, format) {
            NormalizeOutcome::Normalized(again) => assert_eq!(
                again.files, normalized.files,
                "{name} is not the same twice"
            ),
            other => panic!("{name}, again: {other:?}"),
        }
        let hash = [index as u8 + 1; 32];
        let at = store.put(&hash, &normalized.files).unwrap();
        assert!(at.join(MANIFEST_PATH).exists());
        assert_eq!(
            store.manifest(&hash).unwrap().unwrap(),
            normalized.manifest,
            "{name}"
        );
        for (path, data) in &normalized.files {
            assert_eq!(
                &store.read(&hash, path).unwrap().unwrap(),
                data,
                "{name}: {path}"
            );
        }
    }
    assert_eq!(client.restarts(), 0);
}

#[test]
fn a_book_that_cannot_be_read_is_an_answer_and_not_a_crash() {
    let dir = TempDir::new();
    let mut client = client();
    for (name, format, bytes) in [
        (
            "broken.epub",
            Format::Epub,
            b"PK\x03\x04 this is not a zip".to_vec(),
        ),
        ("broken.mobi", Format::Mobi, vec![0u8; 200]),
        ("broken.fb2", Format::Fb2, b"<FictionBook><body>".to_vec()),
    ] {
        let path = dir.0.join(name);
        std::fs::write(&path, &bytes).unwrap();
        let file = File::open(&path).unwrap();
        match client.normalize(&file, format) {
            NormalizeOutcome::Nothing(reason) => assert!(!reason.is_empty(), "{name}"),
            other => panic!("{name}: {other:?}"),
        }
    }
    assert_eq!(
        client.restarts(),
        0,
        "a refusal is an answer: the worker stays"
    );

    // A chapter that unpacks to 2 GiB stops at the limit and is replaced, and the book still opens.
    let path = dir.0.join("bomb.epub");
    std::fs::write(&path, zip_bomb()).unwrap();
    let file = File::open(&path).unwrap();
    match client.normalize(&file, Format::Epub) {
        NormalizeOutcome::Normalized(normalized) => {
            let manifest = normalized.manifest;
            assert_eq!(manifest.documents[0].parsed, "generated");
            assert!(
                manifest.warnings.iter().any(|w| w.contains("too large")),
                "{:?}",
                manifest.warnings
            );
        }
        other => panic!("bomb: {other:?}"),
    }
    assert_eq!(client.restarts(), 0);
}

/// An EPUB whose one chapter unpacks to 2 GiB of zeros.
fn zip_bomb() -> Vec<u8> {
    use std::io::Write;
    let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
    let chunk = vec![0u8; 1 << 20];
    for _ in 0..2048 {
        encoder.write_all(&chunk).unwrap();
    }
    let body = encoder.finish().unwrap();
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data, stored_size, method) in [
        ("META-INF/container.xml", b"<container xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\"><rootfiles><rootfile full-path=\"a.opf\"/></rootfiles></container>".to_vec(), None, 0u16),
        ("a.opf", b"<package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\"><metadata/><manifest><item id=\"c\" href=\"c.xhtml\" media-type=\"application/xhtml+xml\"/></manifest><spine><itemref idref=\"c\"/></spine></package>".to_vec(), None, 0),
        ("c.xhtml", body.clone(), Some(2048u64 << 20), 8),
    ] {
        let at = out.len() as u32;
        let mut fields = Vec::new();
        fields.extend_from_slice(&20u16.to_le_bytes());
        fields.extend_from_slice(&0u16.to_le_bytes());
        fields.extend_from_slice(&method.to_le_bytes());
        fields.extend_from_slice(&[0; 8]);
        fields.extend_from_slice(&(data.len() as u32).to_le_bytes());
        fields.extend_from_slice(&(stored_size.unwrap_or(data.len() as u64).min(u64::from(u32::MAX - 1)) as u32).to_le_bytes());
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
        out.extend_from_slice(&data);
    }
    let at = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(b"PK\x05\x06");
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&3u16.to_le_bytes());
    out.extend_from_slice(&3u16.to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&at.to_le_bytes());
    out.extend_from_slice(&[0; 2]);
    out
}

#[test]
fn a_package_that_does_not_hold_together_is_refused() {
    let manifest_for = |part: &str| {
        format!(
            r#"{{"normalizer":{},"source":{{"format":"epub","version":null,"fixedLayout":false,"direction":null}},
            "metadata":{{"titles":[],"creators":[],"contributors":[],"languages":[],"identifiers":[],"uniqueIdentifier":null,
            "publisher":null,"published":null,"modified":null,"description":null,"subjects":[],"rights":null,"series":null,"cover":null}},
            "documents":[{{"href":"a.xhtml","id":null,"mediaType":"application/xhtml+xml","linear":true,"title":null,"parsed":"xml",
            "bodyStep":4,"textLength":0,"parts":[{{"href":"{part}","start":[],"bytes":0,"textStart":0,"textLength":0}}]}}],
            "resources":[],"toc":[],"pageList":[],"landmarks":[],"stats":{{"documents":1,"parts":1,"splitDocuments":0,"oversizeParts":0,
            "largestPart":0,"largestDocument":0,"htmlFallbacks":0,"resources":0,"resourceBytes":0,"droppedFiles":0,
            "sanitized":{{"scripts":0,"event_handlers":0,"embedded":0,"head_directives":0,"forms":0,"dropped_urls":0,"external_links":0,
            "dropped_attributes":0,"animations":0,"renamed":0}}}},"warnings":[]}}"#,
            musaeum_flow::NORMALIZER_VERSION
        )
    };
    let good = vec![
        (
            "text/0000.xhtml".to_owned(),
            b"<html xmlns='http://www.w3.org/1999/xhtml'><body><p>ok</p></body></html>".to_vec(),
        ),
        (
            MANIFEST_PATH.to_owned(),
            manifest_for("text/0000.xhtml").into_bytes(),
        ),
    ];
    assert!(check_normalized(good).is_ok());
    let missing = vec![(
        MANIFEST_PATH.to_owned(),
        manifest_for("text/0000.xhtml").into_bytes(),
    )];
    assert!(
        check_normalized(missing).is_err(),
        "a part that is not there"
    );
    let twice = vec![
        (
            "text/0000.xhtml".to_owned(),
            b"<html xmlns='http://www.w3.org/1999/xhtml'/>".to_vec(),
        ),
        (
            "TEXT/0000.xhtml".to_owned(),
            b"<html xmlns='http://www.w3.org/1999/xhtml'/>".to_vec(),
        ),
        (
            MANIFEST_PATH.to_owned(),
            manifest_for("text/0000.xhtml").into_bytes(),
        ),
    ];
    assert!(
        check_normalized(twice).is_err(),
        "the same path twice, by case"
    );
    let nonsense = vec![(MANIFEST_PATH.to_owned(), b"{\"normalizer\": 1}".to_vec())];
    assert!(check_normalized(nonsense).is_err());
    // A worker that sends a script back is refused, whatever the manifest says.
    let scripted = vec![
        ("text/0000.xhtml".to_owned(), b"<html xmlns='http://www.w3.org/1999/xhtml'><body><script>alert(1)</script></body></html>".to_vec()),
        (MANIFEST_PATH.to_owned(), manifest_for("text/0000.xhtml").into_bytes()),
    ];
    assert!(check_normalized(scripted).unwrap_err().contains("script"));
    let handler = vec![
        (
            "text/0000.xhtml".to_owned(),
            b"<html xmlns='http://www.w3.org/1999/xhtml'><body onload='x()'/></html>".to_vec(),
        ),
        (
            MANIFEST_PATH.to_owned(),
            manifest_for("text/0000.xhtml").into_bytes(),
        ),
    ];
    assert!(check_normalized(handler).is_err());
    let broken = vec![
        ("text/0000.xhtml".to_owned(), b"<html><body>".to_vec()),
        (
            MANIFEST_PATH.to_owned(),
            manifest_for("text/0000.xhtml").into_bytes(),
        ),
    ];
    assert!(check_normalized(broken).is_err());
}

#[test]
fn a_cover_comes_back_alone_and_is_judged_by_its_bytes() {
    let dir = TempDir::new();
    let short = flow::book(false);
    let mut client = client();
    let cases: Vec<(&str, Format, Vec<u8>, bool)> = vec![
        ("book.epub", Format::Epub, flow::epub3(&short), true),
        ("book.azw3", Format::Azw3, flow::kf8(&short), true),
        ("book.fb2", Format::Fb2, flow::fb2(&short, "utf-8"), true),
        ("book.txt", Format::Txt, flow::txt(&short, "utf-8"), false),
    ];
    for (name, format, bytes, has_cover) in cases {
        let path = dir.0.join(name);
        std::fs::write(&path, &bytes).unwrap();
        let file = File::open(&path).unwrap();
        match client.cover(&file, format) {
            CoverOutcome::Cover { kind, bytes } => {
                assert!(has_cover, "{name} has no cover, yet one came back");
                assert_eq!(ImageKind::sniff(&bytes), Some(kind), "{name}");
            }
            CoverOutcome::NoCover => assert!(!has_cover, "{name} lost its cover"),
            other => panic!("{name}: {other:?}"),
        }
    }
    assert_eq!(client.restarts(), 0);
    assert_eq!(
        ImageKind::sniff(b"<svg xmlns='http://www.w3.org/2000/svg'/>"),
        None
    );
    assert_eq!(
        ImageKind::sniff(b"RIFF\0\0\0\0WEBPVP8 "),
        Some(ImageKind::Webp)
    );
}

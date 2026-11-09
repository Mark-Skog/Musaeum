//! Every format the normalizer reads, from fixtures written by `gen-fixtures`: the same book in each,
//! so the same text, chapters and links have to come out of all of them.

use std::collections::HashMap;

use gen_fixtures::flow;
use musaeum_flow::cfi::{self, Point};
use musaeum_flow::dom::{Dom, NodeId};
use musaeum_flow::locator::{self, Found, LogicalDocument};
use musaeum_flow::normalize::{Options, normalize};
use musaeum_flow::package::{Manifest, NavPoint, NormalizedBook, SourceFormat};
use musaeum_flow::sanitize::find_danger;
use musaeum_flow::split::PART_BUDGET;
use musaeum_flow::url::{self, Reference};
use musaeum_flow::xml;

fn run(format: SourceFormat, bytes: &[u8]) -> NormalizedBook {
    normalize(format, bytes, &Options::default())
        .unwrap_or_else(|error| panic!("{format:?}: {error}"))
}

fn parse(book: &NormalizedBook, path: &str) -> Dom {
    let bytes = book
        .file(path)
        .unwrap_or_else(|| panic!("{path} is missing"));
    xml::parse(std::str::from_utf8(bytes).unwrap(), &xml::Limits::default())
        .unwrap_or_else(|error| panic!("{path} is not well-formed: {error}"))
}

/// Every document put back together from its parts.
fn documents(book: &NormalizedBook) -> Vec<LogicalDocument> {
    book.manifest
        .documents
        .iter()
        .enumerate()
        .map(|(index, document)| {
            let parts = document
                .parts
                .iter()
                .map(|part| (part.start.clone(), parse(book, &part.href)))
                .collect();
            LogicalDocument::assemble(index, parts).unwrap()
        })
        .collect()
}

fn all_text(book: &NormalizedBook) -> String {
    documents(book)
        .iter()
        .map(|document| document.text.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

fn labels(points: &[NavPoint], into: &mut Vec<String>) {
    for point in points {
        into.push(point.label.clone());
        labels(&point.children, into);
    }
}

/// Checks what every normalized book must be, whatever it came from.
fn check_common(name: &str, book: &NormalizedBook) {
    let manifest: &Manifest = &book.manifest;
    // The manifest on disk is the one in memory.
    let stored: Manifest = serde_json::from_slice(book.file("musaeum.json").unwrap()).unwrap();
    assert_eq!(&stored, manifest, "{name}");
    let present: HashMap<&str, &[u8]> = book
        .files
        .iter()
        .map(|(p, d)| (p.as_str(), d.as_slice()))
        .collect();
    for document in &manifest.documents {
        for part in &document.parts {
            let dom = parse(book, &part.href);
            assert_eq!(find_danger(&dom), None, "{name}: {}", part.href);
            if document.parts.len() > 1 {
                assert!(
                    part.bytes as usize <= PART_BUDGET,
                    "{name}: {} is {} bytes",
                    part.href,
                    part.bytes
                );
            }
            // Every reference into the book leads to a file that is there, and a fragment to an id.
            for node in dom.descendants(Dom::DOCUMENT) {
                for attr in dom.attrs(node) {
                    if !matches!(attr.name.local.as_str(), "href" | "src") {
                        continue;
                    }
                    match url::resolve(&part.href, &attr.value) {
                        Reference::Internal { path, fragment } => {
                            assert!(
                                present.contains_key(path.as_str()),
                                "{name}: {} -> {path}",
                                part.href
                            );
                            if let Some(fragment) = fragment
                                && path.ends_with(".xhtml")
                            {
                                let target = parse(book, &path);
                                assert!(
                                    cfi::element_by_id(&target, &fragment).is_some(),
                                    "{name}: {} -> {path}#{fragment}",
                                    part.href
                                );
                            }
                        }
                        // The hostile book links to fragments it never defines, on purpose.
                        Reference::Fragment(fragment) if name != "hostile" => {
                            assert!(
                                cfi::element_by_id(&dom, &fragment).is_some(),
                                "{name}: {} -> #{fragment}",
                                part.href
                            );
                        }
                        Reference::Fragment(_) => {}
                        Reference::External { scheme, .. } => {
                            assert!(
                                matches!(scheme.as_str(), "https" | "http" | "mailto" | "data"),
                                "{name}: {scheme}"
                            );
                        }
                        Reference::Empty => {}
                    }
                }
            }
        }
    }
    for resource in &manifest.resources {
        assert!(
            present.contains_key(resource.href.as_str()),
            "{name}: {}",
            resource.href
        );
    }
    // The same input gives the same bytes.
    assert!(!book.files.is_empty());
}

/// Every text position of every document goes to a Locator and back, and through the reader's CFI.
fn check_positions(name: &str, book: &NormalizedBook) -> usize {
    let mut checked = 0;
    for document in documents(book) {
        let nodes: Vec<NodeId> = document.dom.descendants(document.body.unwrap()).collect();
        let step = (nodes.len() / 400).max(1);
        for node in nodes.into_iter().step_by(step) {
            let point = match document.dom.text(node) {
                Some(text) => {
                    let length = cfi::utf16_len(text);
                    let offset = (0..=length / 2)
                        .rev()
                        .find(|&o| cfi::utf16_to_byte(text, o).is_some())
                        .unwrap_or(0);
                    Point::Text { node, offset }
                }
                None => Point::Element(node),
            };
            let locator = locator::locate(&book.manifest, &document, point).unwrap();
            let json = serde_json::to_string(&locator).unwrap();
            let read: locator::Locator = serde_json::from_str(&json).unwrap();
            let (found, how) = locator::resolve(&read, &document).unwrap();
            assert_eq!(how, Found::Cfi, "{name}");
            assert_eq!(found, point, "{name}: {}", locator.locations.cfi);
            assert_eq!(
                locator::locate(&book.manifest, &document, found).unwrap(),
                locator,
                "{name}"
            );
            let reader =
                locator::canonical_to_reader(&book.manifest, &locator.locations.cfi).unwrap();
            assert_eq!(
                locator::reader_to_canonical(&book.manifest, &reader).unwrap(),
                locator.locations.cfi,
                "{name}"
            );
            checked += 1;
        }
    }
    checked
}

#[test]
fn every_format_gives_the_same_book() {
    let short = flow::book(false);
    let cases: Vec<(&str, SourceFormat, Vec<u8>, usize)> = vec![
        ("epub3", SourceFormat::Epub, flow::epub3(&short), 6),
        ("epub2", SourceFormat::Epub, flow::epub2(&short), 3),
        ("mobi", SourceFormat::Mobi, flow::mobi6(&short), 6),
        ("azw3", SourceFormat::Azw3, flow::kf8(&short), 6),
        ("fb2", SourceFormat::Fb2, flow::fb2(&short, "utf-8"), 6),
        (
            "txt",
            SourceFormat::Txt,
            flow::txt(&short, "windows-1252"),
            6,
        ),
        (
            "markdown",
            SourceFormat::Markdown,
            flow::markdown(&short),
            6,
        ),
    ];
    for (name, format, bytes, chapters) in cases {
        let book = run(format, &bytes);
        check_common(name, &book);
        let text = all_text(&book);
        for (index, chapter) in short.chapters.iter().enumerate().take(chapters) {
            let number = index + 1;
            let first = &chapter.paragraphs[0];
            let last = chapter.paragraphs.last().unwrap();
            // The first paragraph and the last, word for word, whatever the format did with lines.
            for paragraph in [first, last] {
                assert!(
                    text.contains(paragraph.as_str()),
                    "{name}: chapter {number} lost {}",
                    &paragraph[..12]
                );
            }
        }
        // Accents survive every encoding.
        assert!(text.contains("coração"), "{name}");
        let mut toc = Vec::new();
        labels(&book.manifest.toc, &mut toc);
        for chapter in short.chapters.iter().take(chapters) {
            assert!(
                toc.iter().any(|label| label.starts_with(&chapter.title)),
                "{name}: {} is not in {toc:?}",
                chapter.title
            );
        }
        let metadata = &book.manifest.metadata;
        assert!(
            metadata
                .title()
                .is_some_and(|t| t.starts_with("The Sample Reader")),
            "{name}: {:?}",
            metadata.title()
        );
        if name != "txt" {
            assert_eq!(
                metadata.creators.first().map(|c| c.name.as_str()),
                Some("Ana Lima"),
                "{name}"
            );
        }
        let positions = check_positions(name, &book);
        assert!(positions > 200, "{name}: {positions}");
    }
}

#[test]
fn metadata_and_pictures_come_through() {
    let short = flow::book(false);
    let epub = run(SourceFormat::Epub, &flow::epub3(&flow::book(true)));
    let m = &epub.manifest.metadata;
    assert_eq!(m.titles[0].kind.as_deref(), Some("main"));
    assert_eq!(m.creators[0].roles, ["aut"]);
    assert_eq!(m.creators[0].file_as.as_deref(), Some("Lima, Ana"));
    assert_eq!(
        m.series
            .as_ref()
            .map(|s| (s.name.as_str(), s.position.as_deref())),
        Some(("Fixtures", Some("2")))
    );
    assert_eq!(m.description.as_deref(), Some("A book for tests."));
    assert_eq!(
        m.identifiers
            .iter()
            .find(|i| i.scheme.as_deref() == Some("ISBN"))
            .map(|i| i.value.as_str()),
        Some("urn:isbn:9780306406157")
    );
    let cover = m.cover.as_deref().unwrap();
    assert!(epub.file(cover).unwrap().starts_with(b"\x89PNG"));
    // The obfuscated font comes out as the font it was.
    let font = epub
        .manifest
        .resources
        .iter()
        .find(|r| r.href.starts_with("fonts/"))
        .unwrap();
    assert!(epub.file(&font.href).unwrap().starts_with(&[0, 1, 0, 0]));
    // The stylesheet lost what leads outside and kept what is inside.
    let css = epub
        .manifest
        .resources
        .iter()
        .filter(|r| r.media_type == "text/css")
        .map(|r| String::from_utf8_lossy(epub.file(&r.href).unwrap()).into_owned())
        .collect::<String>();
    assert!(
        !css.contains("example.com") && !css.contains("evil"),
        "{css}"
    );
    assert!(
        css.contains("../fonts/") && css.contains("../images/"),
        "{css}"
    );

    let kf8 = run(SourceFormat::Azw3, &flow::kf8(&short));
    let font = kf8
        .manifest
        .resources
        .iter()
        .find(|r| r.href.starts_with("fonts/"))
        .expect("the KF8 font");
    assert!(
        kf8.file(&font.href).unwrap().starts_with(&[0, 1, 0, 0]),
        "decompressed and unmasked"
    );
    assert!(kf8.manifest.metadata.cover.is_some());
    let mobi = run(SourceFormat::Mobi, &flow::mobi6(&short));
    assert!(mobi.manifest.metadata.cover.is_some());
    assert_eq!(mobi.manifest.metadata.languages, ["en"]);
    let fb2 = run(SourceFormat::Fb2, &flow::fb2(&short, "utf-8"));
    assert_eq!(
        fb2.manifest
            .metadata
            .series
            .as_ref()
            .map(|s| s.name.as_str()),
        Some("Fixtures")
    );
    assert!(fb2.manifest.metadata.cover.is_some());
    let russian = run(
        SourceFormat::Fb2,
        &flow::fb2(&flow::russian_book(), "windows-1251"),
    );
    assert_eq!(russian.manifest.metadata.title(), Some("Тихая река"));
    assert!(all_text(&russian).contains("тихой реке"));
}

#[test]
fn a_chapter_over_a_mebibyte_is_cut_between_blocks() {
    let long = flow::book(true);
    for (name, format, bytes) in [
        ("epub3", SourceFormat::Epub, flow::epub3(&long)),
        ("txt", SourceFormat::Txt, flow::txt(&long, "windows-1252")),
    ] {
        let book = run(format, &bytes);
        let stats = &book.manifest.stats;
        assert!(
            stats.largest_document > 1 << 20,
            "{name}: {}",
            stats.largest_document
        );
        assert!(stats.split_documents >= 1, "{name}");
        assert_eq!(stats.oversize_parts, 0, "{name}");
        assert!(
            stats.largest_part as usize <= PART_BUDGET,
            "{name}: {}",
            stats.largest_part
        );
        let cut = book
            .manifest
            .documents
            .iter()
            .find(|d| d.parts.len() > 1)
            .unwrap();
        assert!(cut.parts.len() >= 6, "{name}: {} parts", cut.parts.len());
        if name == "epub3" {
            // Tables, code and figures whole; lists continue their numbering.
            for part in &cut.parts {
                let dom = parse(&book, &part.href);
                for node in dom.descendants(Dom::DOCUMENT) {
                    if dom.is_html(node, "table") {
                        let rows = dom
                            .descendants(node)
                            .filter(|&n| dom.is_html(n, "tr"))
                            .count();
                        assert_eq!(rows, 31, "{}", part.href);
                    }
                    if dom.is_html(node, "pre") {
                        assert!(dom.text_content(node).contains("never cut"));
                    }
                    if dom.is_html(node, "figure") {
                        assert!(dom.text_content(node).contains("stays whole"));
                    }
                }
            }
            check_positions(name, &book);
        }
    }
}

#[test]
fn the_same_book_gives_the_same_bytes() {
    let long = flow::book(true);
    for (format, bytes) in [
        (SourceFormat::Epub, flow::epub3(&long)),
        (SourceFormat::Azw3, flow::kf8(&flow::book(false))),
        (SourceFormat::Fb2, flow::fb2(&flow::book(false), "utf-8")),
    ] {
        let first = run(format, &bytes);
        let second = run(format, &bytes);
        assert_eq!(first.files, second.files, "{format:?}");
    }
}

#[test]
fn the_hostile_book_keeps_nothing_that_runs_or_reaches_out() {
    let dir = std::env::temp_dir().join(format!("musaeum-hostile-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("hostile.epub");
    gen_fixtures::epub::write_hostile_epub(&path, 48780).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let book = run(SourceFormat::Epub, &bytes);
    check_common("hostile", &book);
    let report = book.manifest.stats.sanitized;
    assert!(report.scripts >= 9, "{report:?}");
    assert!(report.event_handlers >= 5, "{report:?}");
    for (path, data) in &book.files {
        let text = String::from_utf8_lossy(data);
        assert!(!text.contains("127.0.0.1"), "{path} still names the canary");
        if path.ends_with(".svg") {
            let dom = xml::parse(&text, &xml::Limits::default()).unwrap();
            assert_eq!(find_danger(&dom), None, "{path}");
        }
    }
}

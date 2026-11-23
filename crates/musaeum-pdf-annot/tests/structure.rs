//! The structure of PDFs written in every layout, read and then updated with annotations: what an
//! update adds is read back as it was given, and nothing that was there changes.

use gen_fixtures::annot::{Layout, annot_fixture, fixture_pages};
use musaeum_pdf_annot::PdfFile;
use musaeum_pdf_annot::document::{PdfError, SectionKind};
use musaeum_pdf_annot::model::{Annot, FLAG_PRINT, Frame, Subtype};
use musaeum_pdf_annot::read::read_annotations;
use musaeum_pdf_annot::syntax::Object;
use musaeum_pdf_annot::write::{WriteOptions, append_annotations};

const LAYOUTS: [Layout; 3] = [Layout::Classic, Layout::Streams, Layout::Hybrid];

fn expected_frames() -> Vec<Frame> {
    fixture_pages()
        .iter()
        .map(|page| {
            Frame::of(
                Some(page.media_box.map(f64::from)),
                page.crop_box.map(|crop| crop.map(f64::from)),
                i64::from(page.rotation) * 90,
            )
        })
        .collect()
}

fn box_quad(left: f64, top: f64, right: f64, bottom: f64) -> [f64; 8] {
    [left, top, right, top, right, bottom, left, bottom]
}

/// One annotation of every kind Musaeum writes, on pages placed every way.
fn new_annots() -> Vec<Annot> {
    let base = Annot {
        opacity: 1.0,
        flags: FLAG_PRINT,
        created: Some(1_795_437_296_000),
        modified: Some(1_795_437_300_000),
        author: Some("Leitora".into()),
        ..Annot::default()
    };
    vec![
        Annot {
            page: 0,
            subtype: Some(Subtype::Highlight),
            rect: [0.1, 0.1, 0.5, 0.12],
            quads: vec![
                box_quad(0.1, 0.1, 0.5, 0.12),
                box_quad(0.1, 0.13, 0.3, 0.15),
            ],
            color: Some([1.0, 0.89, 0.42]),
            contents: "Ação do coração, “aspas” e pão.".into(),
            name: Some("019ab7c2-1111-7aaa-8bbb-000000000001".into()),
            subject: Some("Realce".into()),
            ..base.clone()
        },
        Annot {
            page: 1,
            subtype: Some(Subtype::Underline),
            rect: [0.2, 0.3, 0.6, 0.32],
            quads: vec![box_quad(0.2, 0.3, 0.6, 0.32)],
            color: Some([0.1, 0.4, 0.85]),
            contents: "seta → e marca ✓, fora do PDFDocEncoding".into(),
            name: Some("019ab7c2-1111-7aaa-8bbb-000000000002".into()),
            subject: Some("Sublinhado".into()),
            ..base.clone()
        },
        Annot {
            page: 2,
            subtype: Some(Subtype::Square),
            rect: [0.2, 0.2, 0.4, 0.3],
            color: Some([0.66, 0.9, 0.63]),
            interior: Some([0.66, 0.9, 0.63]),
            width: Some(0.0),
            name: Some("019ab7c2-1111-7aaa-8bbb-000000000003".into()),
            ..base.clone()
        },
        Annot {
            page: 3,
            subtype: Some(Subtype::Text),
            rect: [0.7, 0.1, 0.73, 0.12],
            color: Some([0.98, 0.68, 0.84]),
            contents: "Uma nota na página que gira 270°".into(),
            icon: Some("Comment".into()),
            flags: 0,
            name: Some("019ab7c2-1111-7aaa-8bbb-000000000004".into()),
            ..base.clone()
        },
        Annot {
            page: 5,
            subtype: Some(Subtype::Ink),
            rect: [0.3, 0.3, 0.6, 0.5],
            ink: vec![vec![0.3, 0.3, 0.4, 0.45, 0.6, 0.5], vec![0.5, 0.31]],
            color: Some([0.78, 0.71, 0.99]),
            width: Some(9.0),
            opacity: 0.5,
            name: Some("019ab7c2-1111-7aaa-8bbb-000000000005".into()),
            ..base.clone()
        },
        Annot {
            page: 6,
            subtype: Some(Subtype::Highlight),
            rect: [0.1, 0.4, 0.5, 0.42],
            quads: vec![box_quad(0.1, 0.4, 0.5, 0.42)],
            color: Some([0.6, 0.79, 0.99]),
            contents: String::new(),
            name: Some("019ab7c2-1111-7aaa-8bbb-000000000006".into()),
            ..base
        },
    ]
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

#[test]
fn every_layout_reads_the_same_pages_and_annotations() {
    let frames = expected_frames();
    for layout in LAYOUTS {
        let bytes = annot_fixture(layout);
        let file = PdfFile::open(&bytes).unwrap_or_else(|error| panic!("{layout:?}: {error}"));
        assert_eq!(
            file.newest,
            if layout == Layout::Streams {
                SectionKind::Stream
            } else {
                SectionKind::Table
            },
            "{layout:?}"
        );
        let pages = file.pages().unwrap();
        assert_eq!(pages.len(), 7, "{layout:?}");
        for (page, want) in pages.iter().zip(&frames) {
            assert_eq!(
                page.frame, *want,
                "{layout:?} page {}",
                page.reference.number
            );
        }
        if layout == Layout::Classic {
            assert_eq!(pages[2].reference.generation, 1);
        }
        let reading = read_annotations(&file).unwrap();
        assert_eq!(reading.frames, frames);
        let kinds: Vec<(u32, Option<Subtype>, Option<String>)> = reading
            .annots
            .iter()
            .map(|annot| (annot.page, annot.subtype, annot.name.clone()))
            .collect();
        assert_eq!(
            kinds,
            vec![
                (1, Some(Subtype::Text), None),
                (
                    6,
                    Some(Subtype::Highlight),
                    Some("existing-highlight".into())
                ),
            ],
            "{layout:?}"
        );
        assert_eq!(reading.annots[0].icon.as_deref(), Some("Note"));
        assert_eq!(reading.annots[1].contents, "Nota de outro programa");
        assert_eq!(reading.annots[1].author.as_deref(), Some("Outra Pessoa"));
        assert_eq!(reading.skipped.get("Popup"), Some(&1));
        assert_eq!(reading.skipped.get("Link"), Some(&1));
        // The Streams layout's own update gave /Info a /ModDate.
        if layout == Layout::Streams {
            let info = file
                .trailer
                .get(b"Info")
                .and_then(Object::as_reference)
                .unwrap();
            let loaded = file.load(info.number).unwrap();
            assert!(
                loaded
                    .value
                    .as_dictionary()
                    .unwrap()
                    .get(b"ModDate")
                    .is_some()
            );
        }
    }
}

#[test]
fn an_update_adds_what_it_was_given_and_changes_nothing_that_was_there() {
    let wanted = new_annots();
    for layout in LAYOUTS {
        let original = annot_fixture(layout);
        let file = PdfFile::open(&original).unwrap();
        let before = read_annotations(&file).unwrap();
        let update = append_annotations(
            &file,
            &wanted,
            &WriteOptions {
                author: Some("Ninguém".into()),
                now: 1_795_437_296_000,
            },
        )
        .unwrap_or_else(|error| panic!("{layout:?}: {error}"));
        assert_eq!(update.written.len(), wanted.len());
        let mut copy = original.clone();
        copy.extend_from_slice(&update.tail);
        assert_eq!(
            &copy[..original.len()],
            &original[..],
            "the original's bytes come first, all of them"
        );

        let updated = PdfFile::open(&copy).unwrap_or_else(|error| panic!("{layout:?}: {error}"));
        assert_eq!(
            updated.newest, file.newest,
            "{layout:?}: the update is written as the file was"
        );
        let pages = updated.pages().unwrap();
        assert_eq!(pages.len(), 7);
        for (a, b) in pages.iter().zip(file.pages().unwrap().iter()) {
            assert_eq!(a.frame, b.frame);
            assert_eq!(
                a.reference, b.reference,
                "pages keep their numbers and generations"
            );
        }
        let after = read_annotations(&updated).unwrap();
        assert_eq!(
            after.annots.len(),
            before.annots.len() + wanted.len(),
            "{layout:?}"
        );
        assert_eq!(
            after.skipped, before.skipped,
            "the popup and the link are still there"
        );
        for old in &before.annots {
            assert!(
                after.annots.contains(old),
                "{layout:?}: {old:?} was there and still is"
            );
        }
        for want in &wanted {
            let got = after
                .annots
                .iter()
                .find(|annot| annot.name == want.name)
                .unwrap_or_else(|| panic!("{layout:?}: {:?} is not there", want.name));
            assert_eq!(got.page, want.page);
            assert_eq!(got.subtype, want.subtype);
            assert_eq!(got.contents, want.contents);
            assert_eq!(got.subject, want.subject);
            assert_eq!(got.author.as_deref(), Some("Leitora"));
            assert_eq!(got.created, want.created);
            assert_eq!(got.modified, want.modified);
            assert!(close(got.opacity, want.opacity));
            for (a, b) in got.color.unwrap().iter().zip(want.color.unwrap().iter()) {
                assert!(close(*a, *b));
            }
            assert_eq!(got.quads.len(), want.quads.len());
            for (a, b) in got.quads.iter().flatten().zip(want.quads.iter().flatten()) {
                assert!(close(*a, *b), "{layout:?} {:?}: {a} {b}", want.subtype);
            }
            assert_eq!(got.ink.len(), want.ink.len());
            for (a, b) in got.ink.iter().flatten().zip(want.ink.iter().flatten()) {
                assert!(close(*a, *b));
            }
            if matches!(want.subtype, Some(Subtype::Square | Subtype::Text)) {
                for (a, b) in got.rect.iter().zip(want.rect.iter()) {
                    assert!(close(*a, *b), "{:?} rect {a} {b}", want.subtype);
                }
            }
            if want.subtype == Some(Subtype::Ink) {
                assert_eq!(got.width, Some(9.0));
            }
            if want.subtype == Some(Subtype::Text) {
                assert_eq!(got.icon.as_deref(), Some("Comment"));
            }
        }
        // The outline is still there, as it was.
        let catalog = updated.catalog().unwrap();
        let outlines = catalog
            .value
            .as_dictionary()
            .unwrap()
            .get(b"Outlines")
            .and_then(Object::as_reference)
            .unwrap();
        assert_eq!(
            updated.load(outlines.number).unwrap().value,
            file.load(outlines.number).unwrap().value
        );

        // A page that had no /Annots is its own bytes with one entry added.
        let old_page = file.load(pages[0].reference.number).unwrap();
        let new_page = updated.load(pages[0].reference.number).unwrap();
        let old_raw = old_page.raw();
        let new_raw = new_page.raw();
        assert!(
            new_raw.starts_with(&old_raw[..old_raw.len() - 2]),
            "{layout:?}"
        );
        assert!(new_raw.ends_with(b">>"));
        let added = &new_raw[old_raw.len() - 2..new_raw.len() - 2];
        assert!(
            added.starts_with(b" /Annots ["),
            "{}",
            String::from_utf8_lossy(added)
        );
    }
}

#[test]
fn the_bytes_of_a_note_are_pdfdocencoding_or_utf16_with_its_mark() {
    let original = annot_fixture(Layout::Classic);
    let file = PdfFile::open(&original).unwrap();
    let update = append_annotations(&file, &new_annots(), &WriteOptions::default()).unwrap();
    let mut copy = original.clone();
    copy.extend_from_slice(&update.tail);
    let updated = PdfFile::open(&copy).unwrap();
    let contents = |name: &str| -> Vec<u8> {
        let number = update
            .written
            .iter()
            .find(|written| written.name.as_deref() == Some(name))
            .unwrap()
            .number;
        let loaded = updated.load(number).unwrap();
        loaded
            .value
            .as_dictionary()
            .unwrap()
            .get(b"Contents")
            .unwrap()
            .as_string()
            .unwrap()
            .to_vec()
    };
    let portuguese = contents("019ab7c2-1111-7aaa-8bbb-000000000001");
    assert_ne!(
        &portuguese[..2],
        &[0xFE, 0xFF],
        "accents and curly quotes are in PDFDocEncoding"
    );
    assert!(portuguese.contains(&0xE7), "ç is 0xE7");
    assert!(portuguese.contains(&0x8D), "“ is 0x8D");
    let outside = contents("019ab7c2-1111-7aaa-8bbb-000000000002");
    assert_eq!(
        &outside[..2],
        &[0xFE, 0xFF],
        "an arrow and a check mark need UTF-16"
    );
    assert_eq!(&outside[2..4], &[0x00, b's']);
}

#[test]
fn an_update_goes_on_top_of_an_update() {
    let original = annot_fixture(Layout::Streams);
    let file = PdfFile::open(&original).unwrap();
    let all = new_annots();
    let first = append_annotations(&file, &all[..3], &WriteOptions::default()).unwrap();
    let mut once = original.clone();
    once.extend_from_slice(&first.tail);
    let file = PdfFile::open(&once).unwrap();
    let second = append_annotations(&file, &all[3..], &WriteOptions::default()).unwrap();
    let mut twice = once.clone();
    twice.extend_from_slice(&second.tail);
    let reading = read_annotations(&PdfFile::open(&twice).unwrap()).unwrap();
    assert_eq!(reading.annots.len(), 2 + all.len());
    // Page 6's array is its own object: the second update wrote a newer version of it again.
    assert_eq!(
        reading
            .annots
            .iter()
            .filter(|annot| annot.page == 6)
            .count(),
        2
    );
}

#[test]
fn an_encrypted_file_and_a_broken_one_are_refused() {
    let mut encrypted = annot_fixture(Layout::Classic);
    let at = encrypted
        .windows(7)
        .rposition(|window| window == b"trailer")
        .unwrap();
    let insert = b"<< /Encrypt << /Filter /Standard /V 1 /R 2 /O <00> /U <00> /P -4 >>";
    let open = encrypted[at..]
        .windows(2)
        .position(|window| window == b"<<")
        .unwrap()
        + at;
    encrypted.splice(open..open + 2, insert.iter().copied());
    assert_eq!(PdfFile::open(&encrypted).err(), Some(PdfError::Encrypted));
    assert!(
        PdfFile::open_any(&encrypted).is_ok(),
        "its structure can still be read"
    );

    let whole = annot_fixture(Layout::Classic);
    let truncated = &whole[..whole.len() - 40];
    assert!(PdfFile::open(truncated).is_err(), "no startxref");
    let mut wrong = whole.clone();
    let at = wrong
        .windows(9)
        .rposition(|window| window == b"startxref")
        .unwrap();
    let digits = at + 10;
    wrong[digits] = if wrong[digits] == b'1' { b'2' } else { b'1' };
    assert!(
        PdfFile::open(&wrong).is_err(),
        "a startxref that points elsewhere"
    );
    // The table puts the catalog one byte off, where no object starts.
    let mut moved = whole.clone();
    let table = moved
        .windows(6)
        .rposition(|window| window == b"\nxref\n")
        .unwrap()
        + 1;
    let catalog_entry = table + "xref\n0 36\n".len() + 20;
    assert_eq!(&moved[catalog_entry + 11..catalog_entry + 18], b"00000 n");
    moved[catalog_entry + 9] += 1;
    assert!(PdfFile::open(&moved).is_err() || PdfFile::open(&moved).unwrap().pages().is_err());
}

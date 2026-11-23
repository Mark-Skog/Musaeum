//! Annotations written into a copy of a PDF by the real annotations worker, in its sandbox, and read
//! back by PDFium: each one where its text is on pages turned every way and boxed every way, drawn
//! there, and named, colored and noted as it was given, with the file's own annotations, outline and
//! bytes untouched. Then XFDF, there and back, and what the worker refuses.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use gen_fixtures::annot::{Layout, annot_fixture, fixture_pages};
use musaeum_pdf_annot::client::{AnnotClient, AnnotWorkerConfig};
use musaeum_pdf_annot::model::{Annot, FLAG_PRINT, Subtype};
use musaeum_pdf_annot::protocol::{Found, Reader};
use musaeum_pdf_annot::write::WriteOptions;
use musaeum_render_pdf::engine::{Document, Pdfium};
use musaeum_sandbox::Policy;
use musaeum_shm::chars::line_quads;
use musaeum_shm::tile::SCALE_ONE;
use musaeum_worker::Engine;

/// PDFium allows one thread at a time in a process, and these tests load it here too.
static PDFIUM_HERE: Mutex<()> = Mutex::new(());

fn one_at_a_time() -> MutexGuard<'static, ()> {
    PDFIUM_HERE.lock().unwrap_or_else(PoisonError::into_inner)
}

fn worker_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_musaeum-pdf-annots"))
}

fn client() -> AnnotClient {
    AnnotClient::new(AnnotWorkerConfig {
        exe: Some(worker_exe()),
        policy: Policy::Lockdown,
        ..AnnotWorkerConfig::default()
    })
}

fn dir(test: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("annots-{test}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// The characters of `word` on a page, as PDFium reads them. PDFium orders lines as the page is
/// shown, so which line a word is on depends on how the page is turned.
fn word_range(layer: &musaeum_shm::CharLayer, _line: usize, word: &str) -> (usize, usize) {
    let text: String = layer.chars.iter().map(|glyph| glyph.ch).collect();
    let at = text
        .find(word)
        .unwrap_or_else(|| panic!("{word:?} is not in {text:?}"));
    let start = text[..at].chars().count();
    (start, start + word.chars().count())
}

fn to64(quads: Vec<[f32; 8]>) -> Vec<[f64; 8]> {
    quads.into_iter().map(|quad| quad.map(f64::from)).collect()
}

fn bounds(quads: &[[f64; 8]]) -> [f64; 4] {
    let (mut l, mut t, mut r, mut b) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for quad in quads {
        for corner in 0..4 {
            l = l.min(quad[corner * 2]);
            r = r.max(quad[corner * 2]);
            t = t.min(quad[corner * 2 + 1]);
            b = b.max(quad[corner * 2 + 1]);
        }
    }
    [l, t, r, b]
}

const YELLOW: [f64; 3] = [1.0, 0.89, 0.42];
const BLUE_INK: [f64; 3] = [0.1, 0.4, 0.85];
const GREEN: [f64; 3] = [0.66, 0.9, 0.63];
const PURPLE: [f64; 3] = [0.78, 0.71, 0.99];
const PINK: [f64; 3] = [0.98, 0.68, 0.84];

/// On every page but the last: a highlight on "coração", an underline on "sublinhar", a box
/// around "caixa", a stroke along line 4 and a note at the start of the last line.
fn marks_on(document: &mut Document<'_, '_>, pages: usize) -> Vec<Annot> {
    let mut annots = Vec::new();
    for page in 0..pages as u32 {
        let layer = document.char_layer(page).unwrap();
        let name = |kind: &str| Some(format!("019ab7c2-0000-7000-8000-{page:04}{kind:0>8}"));
        let base = Annot {
            page,
            opacity: 1.0,
            flags: FLAG_PRINT,
            created: Some(1_795_437_296_000),
            modified: Some(1_795_437_296_000),
            ..Annot::default()
        };
        let (start, end) = word_range(&layer, 0, "coração");
        let quads = to64(line_quads(&layer.chars, start, end));
        annots.push(Annot {
            subtype: Some(Subtype::Highlight),
            rect: bounds(&quads),
            quads,
            color: Some(YELLOW),
            contents: "O coração — com acentos: ação, pão, avó. “Aspas”.".into(),
            name: name("1"),
            subject: Some("Realce".into()),
            ..base.clone()
        });
        let (start, end) = word_range(&layer, 1, "sublinhar");
        let quads = to64(line_quads(&layer.chars, start, end));
        annots.push(Annot {
            subtype: Some(Subtype::Underline),
            rect: bounds(&quads),
            quads,
            color: Some(BLUE_INK),
            contents: "Sublinhado → com seta e ✓, em UTF-16".into(),
            name: name("2"),
            subject: Some("Sublinhado".into()),
            ..base.clone()
        });
        let (start, end) = word_range(&layer, 4, "caixa");
        let rect = bounds(&to64(line_quads(&layer.chars, start, end)));
        annots.push(Annot {
            subtype: Some(Subtype::Square),
            rect,
            color: Some(GREEN),
            interior: Some(GREEN),
            width: Some(0.0),
            name: name("3"),
            ..base.clone()
        });
        let (start, end) = word_range(&layer, 3, "riscar à mão");
        let line = bounds(&to64(line_quads(&layer.chars, start, end)));
        let middle = [(line[0] + line[2]) / 2.0, (line[1] + line[3]) / 2.0];
        annots.push(Annot {
            subtype: Some(Subtype::Ink),
            rect: line,
            ink: vec![vec![
                line[0], middle[1], middle[0], middle[1], line[2], middle[1],
            ]],
            color: Some(PURPLE),
            width: Some(9.0),
            opacity: 0.5,
            name: name("4"),
            ..base.clone()
        });
        let (start, _) = word_range(&layer, 5, "Última");
        let at = layer.chars[start].quad;
        let (u, v) = (f64::from(at[0]), f64::from(at[1]));
        annots.push(Annot {
            subtype: Some(Subtype::Text),
            rect: [u, v, u + 0.03, v + 0.025],
            color: Some(PINK),
            contents: "Uma nota solta".into(),
            icon: Some("Comment".into()),
            flags: 0,
            name: name("5"),
            ..base
        });
    }
    annots
}

fn close(a: f64, b: f64, tolerance: f64) -> bool {
    (a - b).abs() <= tolerance
}

/// Whether what a reader read is what was written.
fn assert_same(found: &Found, wanted: &[Annot], what: &str) {
    for want in wanted {
        let got = found
            .reading
            .annots
            .iter()
            .find(|annot| annot.name == want.name)
            .unwrap_or_else(|| panic!("{what}: {:?} was not found", want.name));
        let tag = format!("{what} page {} {:?}", want.page, want.subtype);
        assert_eq!(got.page, want.page, "{tag}");
        assert_eq!(got.subtype, want.subtype, "{tag}");
        assert_eq!(got.contents, want.contents, "{tag}");
        assert_eq!(got.quads.len(), want.quads.len(), "{tag}");
        for (a, b) in got.quads.iter().flatten().zip(want.quads.iter().flatten()) {
            assert!(close(*a, *b, 2e-5), "{tag}: quad corner {a} for {b}");
        }
        if matches!(want.subtype, Some(Subtype::Square | Subtype::Text)) {
            for (a, b) in got.rect.iter().zip(&want.rect) {
                assert!(close(*a, *b, 2e-5), "{tag}: box {a} for {b}");
            }
        }
        assert_eq!(got.ink.len(), want.ink.len(), "{tag}");
        for (a, b) in got.ink.iter().flatten().zip(want.ink.iter().flatten()) {
            assert!(close(*a, *b, 2e-5), "{tag}: stroke {a} for {b}");
        }
        let color = got.color.unwrap_or_else(|| panic!("{tag}: no color"));
        for (a, b) in color.iter().zip(want.color.unwrap().iter()) {
            assert!(
                close(*a, *b, 1.0 / 255.0 + 1e-9),
                "{tag}: color {a} for {b}"
            );
        }
        assert!(close(got.opacity, want.opacity, 0.01), "{tag}: opacity");
        assert_eq!(got.created, want.created, "{tag}");
        assert_eq!(got.modified, want.modified, "{tag}");
    }
}

/// What a mark's color looks like, pixel by pixel.
type Looks = Box<dyn Fn([u8; 3]) -> bool>;

/// A point of the unrotated frame as a point of the page as shown.
fn shown(u: f64, v: f64, rotation: u8) -> (f64, f64) {
    match rotation % 4 {
        1 => (1.0 - v, u),
        2 => (1.0 - u, 1.0 - v),
        3 => (v, 1.0 - u),
        _ => (u, v),
    }
}

/// How many of a mark's sample points are drawn in a color `looks` accepts, in `pixels` of a page
/// shown `width` by `height`.
fn colored_samples(
    pixels: &[u8],
    (width, height): (u32, u32),
    rotation: u8,
    quad: &[f64; 8],
    looks: impl Fn([u8; 3]) -> bool,
) -> (usize, usize) {
    let corner = |at: usize| (quad[at * 2], quad[at * 2 + 1]);
    let (tl, tr, br, bl) = (corner(0), corner(1), corner(2), corner(3));
    let mut hits = 0;
    let mut total = 0;
    for i in 1..10 {
        for j in 1..4 {
            let (s, t) = (f64::from(i) / 10.0, f64::from(j) / 4.0);
            let top = (tl.0 + (tr.0 - tl.0) * s, tl.1 + (tr.1 - tl.1) * s);
            let bottom = (bl.0 + (br.0 - bl.0) * s, bl.1 + (br.1 - bl.1) * s);
            let (u, v) = (
                top.0 + (bottom.0 - top.0) * t,
                top.1 + (bottom.1 - top.1) * t,
            );
            let (x, y) = shown(u, v, rotation);
            let (px, py) = (
                (x * f64::from(width)) as u32,
                (y * f64::from(height)) as u32,
            );
            if px >= width || py >= height {
                continue;
            }
            let at = ((py * width + px) * 4) as usize;
            total += 1;
            if looks([pixels[at], pixels[at + 1], pixels[at + 2]]) {
                hits += 1;
            }
        }
    }
    (hits, total)
}

#[test]
fn exported_annotations_land_on_their_text_on_every_page_and_layout() {
    let _alone = one_at_a_time();
    let pdfium = Pdfium::load(&worker_exe().with_file_name("pdfium.dll")).unwrap();
    let pages = fixture_pages();
    let mut client = client();
    for layout in [Layout::Classic, Layout::Streams, Layout::Hybrid] {
        let work = dir(&format!("{layout:?}"));
        let original_bytes = annot_fixture(layout);
        let original = work.join("original.pdf");
        fs::write(&original, &original_bytes).unwrap();
        let wanted = {
            let mut document = Document::open(&pdfium, &original_bytes).unwrap();
            marks_on(&mut document, pages.len() - 1)
        };

        let file = fs::File::open(&original).unwrap();
        let before = client.read(&file, Reader::Pdfium).unwrap();
        assert_eq!(
            before.reading.annots.len(),
            2,
            "{layout:?}: the sticky note and the highlight"
        );
        assert!(
            before.not_writable.is_none(),
            "{layout:?}: {:?}",
            before.not_writable
        );
        let exported = client
            .export(
                &file,
                wanted.clone(),
                WriteOptions {
                    author: Some("Leitora de Teste".into()),
                    now: 1_795_437_296_000,
                },
            )
            .unwrap_or_else(|error| panic!("{layout:?}: {error}"));
        drop(file);
        let (tail, written) = (exported.tail, exported.written);
        assert_eq!(exported.base, original_bytes.len() as u64);
        assert_eq!(written.len(), wanted.len());
        assert_eq!(
            fs::read(&original).unwrap(),
            original_bytes,
            "the original is never written"
        );

        let copy = work.join("copy.pdf");
        let mut bytes = original_bytes.clone();
        bytes.extend_from_slice(&tail);
        fs::write(&copy, &bytes).unwrap();
        let copy_file = fs::File::open(&copy).unwrap();
        let by_pdfium = client.read(&copy_file, Reader::Pdfium).unwrap();
        assert_same(&by_pdfium, &wanted, &format!("{layout:?} PDFium"));
        let by_objects = client.read(&copy_file, Reader::Objects).unwrap();
        assert_same(&by_objects, &wanted, &format!("{layout:?} objects"));
        for annot in &by_pdfium.reading.annots {
            if wanted.iter().any(|want| want.name == annot.name) {
                assert_eq!(annot.author.as_deref(), Some("Leitora de Teste"));
            }
        }
        // What was there before is all still there.
        assert_eq!(
            by_pdfium.reading.annots.len(),
            before.reading.annots.len() + wanted.len()
        );
        for old in &before.reading.annots {
            assert!(
                by_pdfium
                    .reading
                    .annots
                    .iter()
                    .any(|annot| annot.name == old.name
                        && annot.contents == old.contents
                        && annot.page == old.page),
                "{layout:?}: {old:?}"
            );
        }
        let last: &BTreeMap<String, u32> = &by_pdfium.counts[6];
        assert_eq!(last.get("Popup"), Some(&1), "{layout:?}");
        assert_eq!(last.get("Link"), Some(&1));
        assert_eq!(by_pdfium.reading.frames, before.reading.frames);

        // Drawn where they are: each mark's color over its text, in PDFium's own rendering of
        // the page as shown, and nowhere in the original.
        let mut drawn_copy = Document::open(&pdfium, &bytes).unwrap();
        let mut drawn_original = Document::open(&pdfium, &original_bytes).unwrap();
        for want in &wanted {
            let page = want.page;
            let frame = by_pdfium.reading.frames[page as usize];
            let rotation = frame.rotation;
            // A sticky note keeps its size and stays upright (NoZoom, NoRotate, as Acrobat makes
            // them), with the top left corner of its box where it was: it is looked for there,
            // upright, at one pixel a point.
            let scale = if want.subtype == Some(Subtype::Text) {
                SCALE_ONE
            } else {
                SCALE_ONE * 2
            };
            let (w, h, after) = drawn_copy.render_page(page, scale).unwrap();
            let (_, _, before_pixels) = drawn_original.render_page(page, scale).unwrap();
            if want.subtype == Some(Subtype::Text) {
                let [l, t, r, b] = want.rect;
                let (x, y) = shown(l, t, rotation);
                let (x, y) = ((x * f64::from(w)) as u32, (y * f64::from(h)) as u32);
                let (icon_w, icon_h) = (
                    ((r - l) * frame.width()) as u32,
                    ((b - t) * frame.height()) as u32,
                );
                // The note is a square of its color with a dark edge and three dark lines: most
                // of its pixels are its color.
                let pink = |pixels: &[u8], px: u32, py: u32| {
                    let at = ((py * w + px) * 4) as usize;
                    let (r, g, b) = (pixels[at], pixels[at + 1], pixels[at + 2]);
                    i32::from(r) > i32::from(g) + 40 && b > 150
                };
                let (mut hits, mut before_hits, mut total) = (0, 0, 0);
                for py in y..(y + icon_h).min(h) {
                    for px in x..(x + icon_w).min(w) {
                        total += 1;
                        hits += usize::from(pink(&after, px, py));
                        before_hits += usize::from(pink(&before_pixels, px, py));
                    }
                }
                assert!(
                    total > 200 && hits * 10 >= total * 5 && before_hits == 0,
                    "{layout:?} page {page} (turned {rotation}) note: {hits} of {total} pixels, {before_hits} before"
                );
                continue;
            }
            let (quad, looks): ([f64; 8], Looks) = match want.subtype.unwrap() {
                Subtype::Highlight => (
                    want.quads[0],
                    Box::new(|[r, g, b]: [u8; 3]| r > 200 && g > 170 && b < 170),
                ),
                Subtype::Underline => (
                    musaeum_pdf_annot::write::underline_band(&want.quads[0]),
                    Box::new(|[r, _, b]: [u8; 3]| i32::from(b) > i32::from(r) + 80),
                ),
                Subtype::Square => {
                    let [l, t, r, b] = want.rect;
                    (
                        [l, t, r, t, r, b, l, b],
                        Box::new(|[r, g, b]: [u8; 3]| {
                            i32::from(g) > i32::from(r) + 25 && i32::from(g) > i32::from(b) + 25
                        }),
                    )
                }
                Subtype::Ink => {
                    let [l, t, r, b] = want.rect;
                    let middle = (t + b) / 2.0;
                    let half = 3.0 / 842.0;
                    (
                        [
                            l + 0.005,
                            middle - half,
                            r - 0.005,
                            middle - half,
                            r - 0.005,
                            middle + half,
                            l + 0.005,
                            middle + half,
                        ],
                        Box::new(|[r, g, b]: [u8; 3]| {
                            i32::from(b) > i32::from(g) + 15 && b > 200 && r < 245
                        }),
                    )
                }
                Subtype::Text => {
                    let [l, t, r, b] = want.rect;
                    (
                        [l, t, r, t, r, b, l, b],
                        Box::new(|[r, g, b]: [u8; 3]| i32::from(r) > i32::from(g) + 40 && b > 150),
                    )
                }
                other => panic!("{other:?}"),
            };
            let (hits, total) = colored_samples(&after, (w, h), rotation, &quad, &looks);
            let (before_hits, _) = colored_samples(&before_pixels, (w, h), rotation, &quad, &looks);
            assert!(
                hits * 10 >= total * 4 && before_hits == 0,
                "{layout:?} page {page} (turned {rotation}) {:?}: {hits} of {total} drawn, {before_hits} before",
                want.subtype
            );
        }
    }
}

#[test]
fn xfdf_goes_and_comes_back_and_what_is_hostile_is_refused() {
    let _alone = one_at_a_time();
    let pdfium = Pdfium::load(&worker_exe().with_file_name("pdfium.dll")).unwrap();
    let pages = fixture_pages();
    let work = dir("xfdf");
    let original_bytes = annot_fixture(Layout::Streams);
    let original = work.join("book.pdf");
    fs::write(&original, &original_bytes).unwrap();
    let wanted = {
        let mut document = Document::open(&pdfium, &original_bytes).unwrap();
        marks_on(&mut document, pages.len() - 1)
    };
    let mut client = client();
    let file = fs::File::open(&original).unwrap();
    let xfdf = client
        .write_xfdf(&file, wanted.clone(), Some("book.pdf".into()))
        .unwrap();
    let text = String::from_utf8(xfdf.clone()).unwrap();
    assert!(text.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
    assert!(
        text.contains("<ids original=\"4D7573616575D2D0A9B1C3E5F7091A2B\""),
        "{text}"
    );
    let found = client.read_xfdf(&file, &xfdf).unwrap();
    assert_eq!(found.reading.annots.len(), wanted.len());
    assert_eq!(found.xfdf_ids, found.id);
    // XFDF carries colors as #RRGGBB: within half a step of 255.
    assert_same(&found, &wanted, "XFDF");

    let hostile = br#"<?xml version="1.0"?>
<!DOCTYPE xfdf [<!ENTITY xxe SYSTEM "file:///C:/Windows/win.ini">]>
<xfdf xmlns="http://ns.adobe.com/xfdf/"><annots><text page="0" rect="1,1,20,20"><contents>&xxe;</contents></text></annots></xfdf>"#;
    let refused = client.read_xfdf(&file, hostile).unwrap_err();
    assert!(refused.contains("document type"), "{refused}");
    let worker = client.worker_id();
    // The worker answered, and is still the same one: a refusal is an answer.
    assert!(client.read_xfdf(&file, &xfdf).is_ok());
    assert_eq!(client.worker_id(), worker);
}

#[test]
fn what_cannot_be_written_into_is_refused_and_said() {
    let work = dir("refused");
    let mut client = client();
    // A page that is not there.
    let original = work.join("book.pdf");
    fs::write(&original, annot_fixture(Layout::Classic)).unwrap();
    let file = fs::File::open(&original).unwrap();
    let off = Annot {
        page: 99,
        subtype: Some(Subtype::Highlight),
        quads: vec![[0.1, 0.1, 0.2, 0.1, 0.2, 0.2, 0.1, 0.2]],
        rect: [0.1, 0.1, 0.2, 0.2],
        opacity: 1.0,
        ..Annot::default()
    };
    let error = client
        .export(&file, vec![off], WriteOptions::default())
        .unwrap_err();
    assert!(error.contains("página 100"), "{error}");
    // Not a PDF at all.
    let junk = work.join("junk.pdf");
    fs::write(&junk, b"%PDF-1.7\nnothing here\n").unwrap();
    let junk_file = fs::File::open(&junk).unwrap();
    let error = client.read(&junk_file, Reader::Auto).unwrap_err();
    assert!(error.contains("PDFium"), "{error}");
    let _ = Path::new(&junk);
}

#[test]
fn characters_drawn_turned_have_their_top_where_the_text_has_it() {
    let _alone = one_at_a_time();
    let pdfium = Pdfium::load(&worker_exe().with_file_name("pdfium.dll")).unwrap();
    let bytes = annot_fixture(Layout::Classic);
    let mut document = Document::open(&pdfium, &bytes).unwrap();
    // Page 3's text runs down the page in user space (a text matrix of a quarter turn clockwise),
    // so that it reads upright once the page is turned by its /Rotate 270.
    let layer = document.char_layer(3).unwrap();
    let (start, end) = word_range(&layer, 0, "coração");
    let quad = |at: usize| layer.chars[at].quad.map(f64::from);
    let center = |q: [f64; 8]| {
        (
            (q[0] + q[2] + q[4] + q[6]) / 4.0,
            (q[1] + q[3] + q[5] + q[7]) / 4.0,
        )
    };
    let (first, last) = (center(quad(start)), center(quad(end - 1)));
    let reading = (last.0 - first.0, last.1 - first.1);
    for at in start..end {
        let q = quad(at);
        // Top left to top right goes the way the text reads.
        let along = (q[2] - q[0], q[3] - q[1]);
        assert!(
            along.0 * reading.0 + along.1 * reading.1 > 0.0,
            "char {at}: {q:?}"
        );
        // The glyph's top is toward +x of user space, which is +u: its bottom left is at a
        // smaller u than its top left, and an underline lies there.
        assert!(q[6] < q[0], "char {at}: {q:?}");
        let band = musaeum_pdf_annot::write::underline_band(&q);
        assert!(
            center(band).0 < center(q).0,
            "the underline of char {at} is under it"
        );
    }
}

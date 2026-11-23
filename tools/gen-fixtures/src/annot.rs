//! PDFs for Phase 7: books whose annotations go out to PDF and come back, written in each of the
//! three ways a PDF lists its objects.
//!
//! Every layout holds the same seven pages of text in Helvetica (WinAnsi, so Portuguese accents
//! are real characters of the text layer), each one placed differently on its page:
//!
//! | Page | Box and turn |
//! |---|---|
//! | 0 | the MediaBox of the page tree's root, upright |
//! | 1 | `/Rotate 90` on the page |
//! | 2 | `/Rotate 180` on the page; generation 1 in the classic layout |
//! | 3 | `/Rotate 270` inherited from a page tree node of its own, its text drawn turned so it reads upright |
//! | 4 | a CropBox well inside the MediaBox |
//! | 5 | a MediaBox away from the origin, and a CropBox inside it |
//! | 6 | annotations of its own already: a highlight with an appearance and its popup, and a link, in an `/Annots` array of its own object |
//!
//! Page 1 also has a sticky note, in an `/Annots` array written in the page itself. The catalog
//! has an outline of two entries and the trailer an `/Info` and an `/ID`.
//!
//! [`Layout::Streams`] puts every dictionary but the streams in an object stream, lists them in a
//! cross-reference stream with a PNG predictor, and then has one incremental update already;
//! [`Layout::Hybrid`] lists the compressed ones in a stream that a classic table points to.

use std::fmt::Write as _;
use std::io::{self, Write};

use flate2::Compression;
use flate2::write::ZlibEncoder;

/// How the file lists its objects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// A classic `xref` table.
    Classic,
    /// Object streams and cross-reference streams, with an incremental update.
    Streams,
    /// A classic table with `/XRefStm`, as files written for old and new readers alike are.
    Hybrid,
}

/// A page as the fixture places it.
#[derive(Clone, Debug, PartialEq)]
pub struct FixturePage {
    /// The MediaBox it ends up with, inherited or its own.
    pub media_box: [f32; 4],
    pub crop_box: Option<[f32; 4]>,
    /// The quarter turns it is shown at.
    pub rotation: u8,
    /// Its lines of text, as the text layer reads them.
    pub lines: Vec<String>,
    /// How many annotations it already has, of every kind.
    pub existing: usize,
    /// Whether its text runs down the page in user space, so that it reads upright once the page
    /// is turned.
    pub turned: bool,
}

/// The font size and the distance between lines.
pub const FONT_SIZE: f32 = 14.0;
pub const LEADING: f32 = 22.0;

/// The pages of every layout.
pub fn fixture_pages() -> Vec<FixturePage> {
    let letter = [0.0, 0.0, 595.0, 842.0];
    let page = |number: usize,
                media_box: [f32; 4],
                crop_box: Option<[f32; 4]>,
                rotation: u8,
                existing: usize| FixturePage {
        media_box,
        crop_box,
        rotation,
        lines: vec![
            format!("Página {number}: a ação do coração não é vã."),
            "Linha dois, com palavras para marcar e sublinhar.".to_owned(),
            "Terceira linha: maçã, avó, pêssego e órgão.".to_owned(),
            "Quarta linha para riscar à mão livre.".to_owned(),
            "Quinta linha, com uma caixa em volta.".to_owned(),
            "Última linha desta página.".to_owned(),
        ],
        existing,
        turned: false,
    };
    vec![
        page(1, letter, None, 0, 0),
        page(2, letter, None, 1, 1),
        page(3, letter, None, 2, 0),
        FixturePage {
            turned: true,
            ..page(4, letter, None, 3, 0)
        },
        page(5, letter, Some([40.0, 50.0, 560.0, 800.0]), 0, 0),
        page(
            6,
            [100.0, 200.0, 695.0, 1042.0],
            Some([120.0, 230.0, 675.0, 1012.0]),
            0,
            0,
        ),
        page(7, letter, None, 0, 3),
    ]
}

/// Where a page's first line starts, in user space: inside its visible box, near the top left as
/// it is shown.
pub fn text_origin(page: &FixturePage) -> (f32, f32) {
    let shown = page.crop_box.unwrap_or(page.media_box);
    if page.turned {
        (shown[2] - 95.0, shown[3] - 60.0)
    } else {
        (shown[0] + 50.0, shown[3] - 80.0)
    }
}

/// Text as a WinAnsi string of a content stream, escaped to ASCII.
fn win_ansi(text: &str) -> String {
    let mut out = String::from("(");
    for character in text.chars() {
        let code = match character {
            '“' => 0x93,
            '”' => 0x94,
            '—' => 0x97,
            c if (c as u32) < 0x100 => c as u32,
            _ => u32::from(b'?'),
        };
        match code {
            0x28 | 0x29 | 0x5C => {
                out.push('\\');
                out.push(code as u8 as char);
            }
            0x20..=0x7E => out.push(code as u8 as char),
            _ => {
                let _ = write!(out, "\\{code:03o}");
            }
        }
    }
    out.push(')');
    out
}

/// PDFDocEncoding bytes of Latin-1 text, as a hex string.
fn doc_hex(text: &str) -> String {
    let mut out = String::from("<");
    for character in text.chars() {
        let _ = write!(out, "{:02X}", (character as u32).min(0xFF));
    }
    out.push('>');
    out
}

fn deflate(data: &[u8]) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::new(6));
    encoder
        .write_all(data)
        .expect("writing to memory cannot fail");
    encoder.finish().expect("writing to memory cannot fail")
}

enum Body {
    Dictionary(String),
    Stream(String, Vec<u8>),
}

struct Object {
    number: u32,
    generation: u16,
    body: Body,
}

const CATALOG: u32 = 1;
const ROOT: u32 = 2;
const OUTLINES: u32 = 3;
const NODE: u32 = 4;
const FONT: u32 = 7;
const INFO: u32 = 8;
const FIRST_PAGE: u32 = 10;
const FIRST_CONTENT: u32 = 20;
const ANNOTS_ARRAY: u32 = 30;
const OBJECT_STREAM: u32 = 40;
const XREF_STREAM: u32 = 41;
const UPDATE_XREF: u32 = 42;

fn objects(layout: Layout) -> Vec<Object> {
    let pages = fixture_pages();
    let generation = |number: u32| -> u16 {
        if layout == Layout::Classic && number == FIRST_PAGE + 2 {
            1
        } else {
            0
        }
    };
    let reference = |number: u32| format!("{number} {} R", generation(number));
    let mut objects = vec![
        Object {
            number: CATALOG,
            generation: 0,
            body: Body::Dictionary(format!(
                "<< /Type /Catalog /Pages {ROOT} 0 R /Outlines {OUTLINES} 0 R /PageMode /UseOutlines >>"
            )),
        },
        Object {
            number: ROOT,
            generation: 0,
            body: Body::Dictionary(format!(
                "<< /Type /Pages /MediaBox [0 0 595 842] /Count 7 /Kids [{} {} {} {NODE} 0 R {} {} {}] >>",
                reference(FIRST_PAGE),
                reference(FIRST_PAGE + 1),
                reference(FIRST_PAGE + 2),
                reference(FIRST_PAGE + 4),
                reference(FIRST_PAGE + 5),
                reference(FIRST_PAGE + 6),
            )),
        },
        Object {
            number: OUTLINES,
            generation: 0,
            body: Body::Dictionary(
                "<< /Type /Outlines /First 5 0 R /Last 6 0 R /Count 2 >>".to_owned(),
            ),
        },
        Object {
            number: NODE,
            generation: 0,
            body: Body::Dictionary(format!(
                "<< /Type /Pages /Parent {ROOT} 0 R /Rotate 270 /Count 1 /Kids [{}] >>",
                reference(FIRST_PAGE + 3)
            )),
        },
        Object {
            number: 5,
            generation: 0,
            body: Body::Dictionary(format!(
                "<< /Title {} /Parent {OUTLINES} 0 R /Next 6 0 R /Dest [{} /Fit] >>",
                doc_hex("Começo"),
                reference(FIRST_PAGE)
            )),
        },
        Object {
            number: 6,
            generation: 0,
            body: Body::Dictionary(format!(
                "<< /Title {} /Parent {OUTLINES} 0 R /Prev 5 0 R /Dest [{} /Fit] >>",
                doc_hex("Página com anotações"),
                reference(FIRST_PAGE + 6)
            )),
        },
        Object {
            number: FONT,
            generation: 0,
            body: Body::Dictionary(
                "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
                    .to_owned(),
            ),
        },
        Object {
            number: INFO,
            generation: 0,
            body: Body::Dictionary(format!(
                "<< /Title {} /Producer (gen-fixtures) >>",
                doc_hex("Livro de anotações")
            )),
        },
    ];
    for (index, page) in pages.iter().enumerate() {
        let number = FIRST_PAGE + index as u32;
        let parent = if index == 3 { NODE } else { ROOT };
        let mut dictionary = format!(
            "<< /Type /Page /Parent {parent} 0 R /Resources << /Font << /F1 {FONT} 0 R >> >> /Contents {} 0 R",
            FIRST_CONTENT + index as u32
        );
        match index {
            1 => dictionary.push_str(" /Rotate 90 /Annots [ 35 0 R ]"),
            2 => dictionary.push_str(" /Rotate 180"),
            4 => dictionary.push_str(" /CropBox [40 50 560 800]"),
            5 => dictionary.push_str(" /MediaBox [100 200 695 1042] /CropBox [120 230 675 1012]"),
            6 => {
                let _ = write!(dictionary, " /Annots {ANNOTS_ARRAY} 0 R");
            }
            _ => {}
        }
        dictionary.push_str(" >>");
        objects.push(Object {
            number,
            generation: generation(number),
            body: Body::Dictionary(dictionary),
        });
        let (x, y) = text_origin(page);
        // Turned text runs down the page, and each line is to the left of the one before.
        let place = if page.turned {
            format!("0 -1 1 0 {x} {y} Tm")
        } else {
            format!("{x} {y} Td")
        };
        let mut content = format!("BT /F1 {FONT_SIZE} Tf {LEADING} TL {place}\n");
        for line in &page.lines {
            let _ = writeln!(content, "{} Tj T*", win_ansi(line));
        }
        content.push_str("ET\n");
        objects.push(Object {
            number: FIRST_CONTENT + index as u32,
            generation: 0,
            body: Body::Stream(
                "/Filter /FlateDecode".to_owned(),
                deflate(content.as_bytes()),
            ),
        });
    }
    // Page 6's own annotations: a highlight over its first line, with its popup, and a link.
    let page = &pages[6];
    let (x, y) = text_origin(page);
    let (top, bottom) = (y + FONT_SIZE * 0.9, y - FONT_SIZE * 0.25);
    let right = x + 120.0;
    let highlight_ap = format!("1 1 0 rg {x} {bottom} {} {} re f", right - x, top - bottom);
    objects.extend([
        Object {
            number: ANNOTS_ARRAY,
            generation: 0,
            body: Body::Dictionary("[31 0 R 32 0 R 33 0 R]".to_owned()),
        },
        Object {
            number: 31,
            generation: 0,
            body: Body::Dictionary(format!(
                "<< /Type /Annot /Subtype /Highlight /Rect [{x} {bottom} {right} {top}] /QuadPoints [{x} {top} {right} {top} {x} {bottom} {right} {bottom}] \
                 /C [1 1 0] /NM (existing-highlight) /T (Outra Pessoa) /Contents (Nota de outro programa) /Popup 32 0 R /F 4 /AP << /N 34 0 R >> >>"
            )),
        },
        Object {
            number: 32,
            generation: 0,
            body: Body::Dictionary(format!(
                "<< /Type /Annot /Subtype /Popup /Parent 31 0 R /Rect [{right} {bottom} {} {}] /Open false >>",
                right + 150.0,
                top + 80.0
            )),
        },
        Object {
            number: 33,
            generation: 0,
            body: Body::Dictionary(format!(
                "<< /Type /Annot /Subtype /Link /Rect [{x} {} {right} {}] /Border [0 0 0] /Dest [{} /Fit] >>",
                bottom - LEADING,
                top - LEADING,
                reference(FIRST_PAGE)
            )),
        },
        Object {
            number: 34,
            generation: 0,
            body: Body::Stream(
                format!("/Type /XObject /Subtype /Form /BBox [{x} {bottom} {right} {top}]"),
                highlight_ap.into_bytes(),
            ),
        },
        Object {
            number: 35,
            generation: 0,
            body: Body::Dictionary(
                "<< /Type /Annot /Subtype /Text /Rect [500 780 520 800] /Name /Note /C [1 0.8 0] /Contents (Uma nota antiga) /F 28 >>".to_owned(),
            ),
        },
    ]);
    objects
}

/// The fixture written in `layout`.
pub fn annot_fixture(layout: Layout) -> Vec<u8> {
    let objects = objects(layout);
    let id = "<4D7573616575D2D0A9B1C3E5F7091A2B> <4D7573616575D2D0A9B1C3E5F7091A2B>";
    let mut out: Vec<u8> = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets: Vec<(u32, u64, u16)> = Vec::new();
    let write_plain = |out: &mut Vec<u8>, offsets: &mut Vec<(u32, u64, u16)>, object: &Object| {
        offsets.push((object.number, out.len() as u64, object.generation));
        out.extend_from_slice(format!("{} {} obj\n", object.number, object.generation).as_bytes());
        match &object.body {
            Body::Dictionary(text) => out.extend_from_slice(text.as_bytes()),
            Body::Stream(dictionary, data) => {
                out.extend_from_slice(
                    format!("<< {dictionary} /Length {} >>\nstream\n", data.len()).as_bytes(),
                );
                out.extend_from_slice(data);
                out.extend_from_slice(b"\nendstream");
            }
        }
        out.extend_from_slice(b"\nendobj\n");
    };

    if layout == Layout::Classic {
        for object in &objects {
            write_plain(&mut out, &mut offsets, object);
        }
        let at = out.len();
        let size = offsets.iter().map(|(n, _, _)| *n).max().unwrap_or(0) + 1;
        let mut table = format!("xref\n0 {size}\n");
        for number in 0..size {
            match offsets.iter().find(|(n, _, _)| *n == number) {
                Some((_, offset, generation)) => {
                    let _ = write!(table, "{offset:010} {generation:05} n\r\n");
                }
                None => table.push_str("0000000000 65535 f\r\n"),
            }
        }
        let _ = write!(
            table,
            "trailer\n<< /Size {size} /Root {CATALOG} 0 R /Info {INFO} 0 R /ID [{id}] >>\nstartxref\n{at}\n%%EOF\n"
        );
        out.extend_from_slice(table.as_bytes());
        return out;
    }

    // Every dictionary goes in the object stream; streams cannot.
    let (compressed, plain): (Vec<&Object>, Vec<&Object>) = objects
        .iter()
        .partition(|object| matches!(object.body, Body::Dictionary(_)));
    let mut header = String::new();
    let mut body = String::new();
    for object in &compressed {
        let Body::Dictionary(text) = &object.body else {
            unreachable!("partitioned above");
        };
        let _ = write!(header, "{} {} ", object.number, body.len());
        body.push_str(text);
        body.push('\n');
    }
    let first = header.len();
    let stream_data = format!("{header}{body}");
    for object in &plain {
        write_plain(&mut out, &mut offsets, object);
    }
    write_plain(
        &mut out,
        &mut offsets,
        &Object {
            number: OBJECT_STREAM,
            generation: 0,
            body: Body::Stream(
                format!(
                    "/Type /ObjStm /N {} /First {first} /Filter /FlateDecode",
                    compressed.len()
                ),
                deflate(stream_data.as_bytes()),
            ),
        },
    );

    // The rows of a cross-reference stream: type, a 4-byte field, a 2-byte field.
    let row = |kind: u8, second: u64, third: u16| -> Vec<u8> {
        let mut row = vec![kind];
        row.extend_from_slice(&(second as u32).to_be_bytes());
        row.extend_from_slice(&third.to_be_bytes());
        row
    };
    let compressed_rows = |number: u32| {
        compressed
            .iter()
            .position(|object| object.number == number)
            .map(|index| row(2, u64::from(OBJECT_STREAM), index as u16))
    };
    let xref_at = out.len() as u64;
    offsets.push((XREF_STREAM, xref_at, 0));
    match layout {
        Layout::Streams => {
            let size = XREF_STREAM + 1;
            let mut rows = Vec::new();
            for number in 0..size {
                let entry = if let Some(entry) = compressed_rows(number) {
                    entry
                } else if let Some((_, offset, generation)) =
                    offsets.iter().find(|(n, _, _)| *n == number)
                {
                    row(1, *offset, *generation)
                } else {
                    row(0, 0, if number == 0 { 65535 } else { 0 })
                };
                rows.push(entry);
            }
            // The PNG "up" predictor, as most writers use it: each row as its difference from the
            // one above.
            let mut predicted = Vec::new();
            let mut previous = vec![0u8; 7];
            for current in &rows {
                predicted.push(2u8);
                predicted.extend(
                    current
                        .iter()
                        .zip(&previous)
                        .map(|(a, b)| a.wrapping_sub(*b)),
                );
                previous.clone_from(current);
            }
            let data = deflate(&predicted);
            out.extend_from_slice(
                format!(
                    "{XREF_STREAM} 0 obj\n<< /Type /XRef /Size {size} /W [1 4 2] /Root {CATALOG} 0 R /Info {INFO} 0 R /ID [{id}] \
                     /Filter /FlateDecode /DecodeParms << /Predictor 12 /Columns 7 >> /Length {} >>\nstream\n",
                    data.len()
                )
                .as_bytes(),
            );
            out.extend_from_slice(&data);
            out.extend_from_slice(
                format!("\nendstream\nendobj\nstartxref\n{xref_at}\n%%EOF\n").as_bytes(),
            );

            // An update that came before Musaeum: a new /Info, in a cross-reference stream of its own.
            let info_at = out.len() as u64;
            out.extend_from_slice(
                format!(
                    "{INFO} 0 obj\n<< /Title {} /Producer (gen-fixtures) /ModDate (D:20261101120000Z) >>\nendobj\n",
                    doc_hex("Livro de anotações")
                )
                .as_bytes(),
            );
            let update_at = out.len() as u64;
            let mut rows = row(1, info_at, 0);
            rows.extend(row(1, update_at, 0));
            out.extend_from_slice(
                format!(
                    "{UPDATE_XREF} 0 obj\n<< /Type /XRef /Size {} /Index [{INFO} 1 {UPDATE_XREF} 1] /W [1 4 2] /Root {CATALOG} 0 R \
                     /Info {INFO} 0 R /ID [{id}] /Prev {xref_at} /Length {} >>\nstream\n",
                    UPDATE_XREF + 1,
                    rows.len()
                )
                .as_bytes(),
            );
            out.extend_from_slice(&rows);
            out.extend_from_slice(
                format!("\nendstream\nendobj\nstartxref\n{update_at}\n%%EOF\n").as_bytes(),
            );
        }
        Layout::Hybrid => {
            // The stream lists only what is compressed; the table lists the rest, and the
            // compressed objects as free, for readers that know no streams.
            let mut rows = Vec::new();
            let mut index = String::new();
            for object in &compressed {
                rows.extend(compressed_rows(object.number).expect("it is compressed"));
                let _ = write!(index, "{} 1 ", object.number);
            }
            out.extend_from_slice(
                format!(
                    "{XREF_STREAM} 0 obj\n<< /Type /XRef /Size {} /Index [{}] /W [1 4 2] /Length {} >>\nstream\n",
                    XREF_STREAM + 1,
                    index.trim_end(),
                    rows.len()
                )
                .as_bytes(),
            );
            out.extend_from_slice(&rows);
            out.extend_from_slice(b"\nendstream\nendobj\n");
            let at = out.len();
            let size = XREF_STREAM + 1;
            let mut table = format!("xref\n0 {size}\n");
            for number in 0..size {
                match offsets.iter().find(|(n, _, _)| *n == number) {
                    Some((_, offset, generation)) => {
                        let _ = writeln!(table, "{offset:010} {generation:05} n ");
                    }
                    None => table.push_str("0000000000 65535 f \n"),
                }
            }
            let _ = write!(
                table,
                "trailer\n<< /Size {size} /Root {CATALOG} 0 R /Info {INFO} 0 R /ID [{id}] /XRefStm {xref_at} >>\nstartxref\n{at}\n%%EOF\n"
            );
            out.extend_from_slice(table.as_bytes());
        }
        Layout::Classic => unreachable!("written above"),
    }
    out
}

/// Writes the three layouts into `dir` as `annot-classic.pdf`, `annot-streams.pdf` and
/// `annot-hybrid.pdf`.
pub fn write_annot_fixtures(dir: &std::path::Path) -> io::Result<()> {
    for (layout, name) in [
        (Layout::Classic, "annot-classic.pdf"),
        (Layout::Streams, "annot-streams.pdf"),
        (Layout::Hybrid, "annot-hybrid.pdf"),
    ] {
        std::fs::write(dir.join(name), annot_fixture(layout))?;
    }
    Ok(())
}

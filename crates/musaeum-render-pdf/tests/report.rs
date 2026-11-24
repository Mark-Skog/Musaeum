//! A study report as PDFium reads it: the file opens, every page draws, the text set in it is the
//! text a reader selects, accents and all (and a character no font draws is still that character
//! when copied), the bands beside the annotations are the colors of their marks, and the works and
//! chapters are bookmarks. Pages are left as PNGs in the target folder to be looked at.

use std::fs;
use std::io::Write as _;
use std::path::PathBuf;

use flate2::Compression;
use flate2::write::ZlibEncoder;
use musaeum_render_pdf::engine::{Document, Pdfium};
use musaeum_report::{
    Creator, Found, Heading, Identifier, Item, Kind, Layout, Mark, Report, WorkReport,
};
use musaeum_shm::tile::SCALE_ONE;
use musaeum_worker::Engine;

fn pdfium_dll() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_musaeum-pdf-annots")).with_file_name("pdfium.dll")
}

const NOTE: &str = "Ação, coração e pão — “aspas” → ✓ e **importante**, *itálico*, `let x = 1;`\n\n- primeiro item\n- segundo item\n\n```\nfn main() {\n    println!(\"olá\");\n}\n```";

fn item(
    index: usize,
    kind: Kind,
    mark: Mark,
    color: &str,
    quote: Option<&str>,
    note: &str,
) -> Item {
    let at = index as f64 * 0.7;
    Item {
        uuid: format!("0192f000-0000-7000-8000-{index:012}"),
        kind,
        mark,
        color: color.into(),
        quote: quote.map(str::to_owned),
        note: note.into(),
        at: Some(at),
        page: Some(at as u32),
        progression: Some(at / 40.0),
        found: Found::Exact,
        created_at: 1_795_466_405_123,
        updated_at: 1_795_466_405_123,
    }
}

fn report() -> Report {
    let long =
        "A física é a ciência que estuda a natureza em seus aspectos mais gerais. ".repeat(70);
    let mut items = vec![
        item(
            0,
            Kind::Highlight,
            Mark::Text,
            "yellow",
            Some("Ação e reação são iguais em módulo e opostas em sentido."),
            NOTE,
        ),
        item(
            1,
            Kind::Underline,
            Mark::Text,
            "blue",
            Some("∫ f(x) dx ≤ ∞, com ε > 0 e 中文"),
            "",
        ),
        item(
            2,
            Kind::Note,
            Mark::Text,
            "green",
            Some(&long),
            "Uma citação longa que atravessa a página.",
        ),
        item(
            3,
            Kind::Highlight,
            Mark::Box,
            "#ff8800",
            None,
            "Uma caixa numa figura.",
        ),
    ];
    for index in 4..34 {
        items.push(item(
            index,
            Kind::Highlight,
            Mark::Text,
            ["yellow", "green", "pink", "purple"][index % 4],
            Some("Um trecho marcado para encher as páginas do relatório."),
            "Nota curta.",
        ));
    }
    let mut orphan = item(
        40,
        Kind::Highlight,
        Mark::Text,
        "yellow",
        Some("Trecho que sumiu nesta edição."),
        "",
    );
    orphan.found = Found::Orphan;
    items.push(orphan);
    Report {
        generated_at: 1_795_466_405_123,
        utc_offset_minutes: -180,
        generator: "Musaeum 0.0.1".into(),
        works: vec![WorkReport {
            title: "Física Básica: Mecânica".into(),
            creators: vec![Creator {
                name: "H. Moysés Nussenzveig".into(),
                role: "author".into(),
            }],
            series: Some("Curso de Física Básica".into()),
            series_position: Some(1.0),
            year: Some(2002),
            language: Some("pt-BR".into()),
            identifiers: vec![Identifier {
                scheme: "isbn".into(),
                value: "9788521202981".into(),
            }],
            format: "pdf".into(),
            layout: Layout::Fixed,
            pages: Some(40),
            contents: vec![
                Heading {
                    label: "Introdução".into(),
                    depth: 0,
                    at: 0.0,
                },
                Heading {
                    label: "Cinemática".into(),
                    depth: 0,
                    at: 2.0,
                },
                Heading {
                    label: "Movimento uniforme".into(),
                    depth: 1,
                    at: 2.5,
                },
                Heading {
                    label: "Dinâmica".into(),
                    depth: 0,
                    at: 12.0,
                },
            ],
            items,
        }],
    }
}

fn crc(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                0xEDB8_8320 ^ (crc >> 1)
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// RGBA rows as a PNG file, to look at.
fn png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity(rgba.len() + height as usize);
    for row in rgba.chunks_exact(width as usize * 4) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&raw).unwrap();
    let data = encoder.finish().unwrap();
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut chunk = |kind: &[u8; 4], body: &[u8]| {
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        let mut checked = kind.to_vec();
        checked.extend_from_slice(body);
        out.extend_from_slice(&checked);
        out.extend_from_slice(&crc(&checked).to_be_bytes());
    };
    let mut header = Vec::new();
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(b"IHDR", &header);
    chunk(b"IDAT", &data);
    chunk(b"IEND", &[]);
    out
}

fn squeeze(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn a_report_opens_in_pdfium_with_its_text_its_colors_and_its_bookmarks() {
    let report = report();
    let bytes = musaeum_report::pdf::pdf(&report);
    assert_eq!(
        musaeum_report::pdf::pdf(&report),
        bytes,
        "the same report, the same bytes"
    );
    let folder = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("report");
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("relatorio.pdf"), &bytes).unwrap();

    let pdfium = Pdfium::load(&pdfium_dll()).unwrap();
    let mut document = Document::open(&pdfium, &bytes).expect("PDFium opens the report");
    let pages = document.page_count();
    assert!(pages >= 3, "{pages} pages");

    let mut text = String::new();
    for page in 0..pages {
        let layer = document.char_layer(page).unwrap();
        let page_text: String = layer.chars.iter().map(|glyph| glyph.ch).collect();
        assert!(
            squeeze(&page_text).contains(&format!("página {} de {pages}", page + 1)),
            "page {page}: {page_text}"
        );
        text.push_str(&page_text);
        text.push('\n');
    }
    let text = squeeze(&text);
    for expected in [
        "Física Básica: Mecânica",
        "H. Moysés Nussenzveig",
        "Série: Curso de Física Básica, n.º 1",
        "Identificadores: ISBN 9788521202981",
        "Ação e reação são iguais em módulo e opostas em sentido.",
        "Ação, coração e pão — “aspas” → ✓ e importante, itálico, let x = 1;",
        "primeiro item",
        "println!(\"olá\");",
        "∫ f(x) dx ≤ ∞, com ε > 0 e 中文",
        "Sem texto: uma marca desenhada na página.",
        "Não encontradas no livro",
        "Trecho que sumiu nesta edição.",
        "Movimento uniforme",
        "Realce · amarelo · p. 1 · 23/11/2026",
    ] {
        assert!(
            text.contains(expected),
            "{expected:?} is not in the text: {text}"
        );
    }
    // The long quote is cut across a page break, not lost.
    assert_eq!(
        text.matches("A física é a ciência que estuda a natureza")
            .count(),
        70,
        "every sentence of the long quote"
    );

    let titles: Vec<(u16, String)> = document
        .outline()
        .into_iter()
        .map(|entry| (entry.level, entry.title))
        .collect();
    assert_eq!(titles[0], (0, "Física Básica: Mecânica".to_owned()));
    for chapter in [
        "Introdução",
        "Cinemática",
        "Dinâmica",
        "Não encontradas no livro",
    ] {
        assert!(titles.contains(&(1, chapter.to_owned())), "{titles:?}");
    }
    assert!(
        titles.contains(&(2, "Movimento uniforme".to_owned())),
        "{titles:?}"
    );

    // Every page draws; the bands are the marks' colors, in the left margin.
    let mut seen = std::collections::BTreeSet::new();
    for page in 0..pages {
        let (width, height, rgba) = document.render_page(page, SCALE_ONE * 2).unwrap();
        fs::write(
            folder.join(format!("pagina-{}.png", page + 1)),
            png(width, height, &rgba),
        )
        .unwrap();
        let dark = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|px| px[0] < 90 && px[1] < 90 && px[2] < 90)
            .count();
        assert!(
            dark > 2000,
            "page {page} has text drawn: {dark} dark pixels"
        );
        // The band: 64 to 67.6 points from the left, at two pixels a point.
        let x = 131;
        for y in 0..height {
            let at = ((y * width + x) * 4) as usize;
            let rgb = [rgba[at], rgba[at + 1], rgba[at + 2]];
            for (name, color) in [
                ("yellow", [0xff, 0xe3, 0x6b]),
                ("blue underline", [26, 102, 217]),
                ("green", [0xa8, 0xe6, 0xa1]),
                ("orange", [0xff, 0x88, 0x00]),
                ("pink", [0xfa, 0xad, 0xd6]),
                ("purple", [0xc7, 0xb5, 0xfc]),
            ] {
                if rgb.iter().zip(color).all(|(a, b)| a.abs_diff(b) <= 6) {
                    seen.insert(name);
                }
            }
        }
    }
    assert_eq!(seen.len(), 6, "bands seen: {seen:?}");
}

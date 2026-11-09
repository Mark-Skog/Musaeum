//! Fixtures for the flowing formats: one book in every format the normalizer reads, so tests can
//! look for the same text in all of them, and a long variant whose third chapter passes 1 MiB.
//!
//! Every paragraph starts with a marker, `[c3p17]` for paragraph 17 of chapter 3, and every chapter
//! has a link to the next and a footnote, so a test can tell whether text, links and order survived.
//! The Mobipocket and KF8 writers here are written from the format's descriptions, apart from the
//! reader; files made by Calibre are the second, independent check (ADR 0005).

use std::fmt::Write as _;
use std::io::{self, Write};
use std::path::Path;

use flate2::Compression;
use flate2::write::ZlibEncoder;

use crate::XorShift;
use crate::epub::{ZipWriter, xml_escape};

/// A 1×1 PNG.
pub const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0xDA, 0x63, 0xF8, 0xCF, 0xC0, 0xF0,
    0x1F, 0x00, 0x05, 0x00, 0x01, 0xFF, 0x89, 0x99, 0x3D, 0x1D, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

/// Not a font a browser could use, but a file every reader takes for TrueType by its first bytes.
fn fake_font() -> Vec<u8> {
    let mut font = vec![0x00, 0x01, 0x00, 0x00];
    let mut rng = XorShift::new(77);
    font.extend((0..3000).map(|_| rng.below(256) as u8));
    font
}

pub struct Chapter {
    pub title: String,
    pub paragraphs: Vec<String>,
}

pub struct Book {
    pub title: String,
    pub author: String,
    pub language: String,
    pub chapters: Vec<Chapter>,
}

const WORDS: [&str; 40] = [
    "the",
    "reader",
    "turned",
    "a",
    "page",
    "and",
    "light",
    "fell",
    "across",
    "river",
    "morning",
    "ação",
    "coração",
    "página",
    "livro",
    "não",
    "é",
    "café",
    "naïve",
    "über",
    "mañana",
    "garçon",
    "of",
    "quiet",
    "stone",
    "window",
    "letters",
    "came",
    "slowly",
    "through",
    "old",
    "glass",
    "garden",
    "with",
    "every",
    "small",
    "voice",
    "kept",
    "its",
    "promise",
];

/// The sample book. With `long`, chapter 3 holds over 1 MiB of text.
pub fn book(long: bool) -> Book {
    let mut rng = XorShift::new(2026);
    let chapters = (1..=6)
        .map(|number| {
            let count = if long && number == 3 { 2600 } else { 40 };
            let paragraphs = (1..=count)
                .map(|p| {
                    let words = 50 + rng.below(40) as usize;
                    let body: Vec<&str> = (0..words)
                        .map(|_| WORDS[rng.below(WORDS.len() as u32) as usize])
                        .collect();
                    format!("[c{number}p{p}] {}.", body.join(" "))
                })
                .collect();
            Chapter {
                title: format!(
                    "Chapter {number}: {}",
                    [
                        "Arrival",
                        "Letters",
                        "The River",
                        "Glass",
                        "Quiet",
                        "Promise"
                    ][number - 1]
                ),
                paragraphs,
            }
        })
        .collect();
    Book {
        title: "The Sample Reader".to_owned(),
        author: "Ana Lima".to_owned(),
        language: "en".to_owned(),
        chapters,
    }
}

fn escape(text: &str) -> String {
    xml_escape(text).replace('>', "&gt;")
}

// ---------------------------------------------------------------------------------------------
// EPUB

fn chapter_xhtml(book: &Book, index: usize, epub3: bool) -> String {
    let chapter = &book.chapters[index];
    let number = index + 1;
    let mut body = String::new();
    let _ = writeln!(
        body,
        "<h2 id=\"ch{number}\">{}</h2>",
        escape(&chapter.title)
    );
    let long = chapter.paragraphs.len() > 100;
    if long {
        body.push_str("<div class=\"chapter\" id=\"long\">\n");
    }
    for (p, paragraph) in chapter.paragraphs.iter().enumerate() {
        let _ = writeln!(
            body,
            "<p id=\"c{number}p{}\">{}</p>",
            p + 1,
            escape(paragraph)
        );
        if long && p % 400 == 199 {
            body.push_str("<table class=\"data\"><tr><th>Key</th><th>Value</th></tr>");
            for row in 0..30 {
                let _ = write!(
                    body,
                    "<tr><td>k{row}</td><td>value {row} of a table that is never cut</td></tr>"
                );
            }
            body.push_str("</table>\n<ol start=\"3\">");
            for item in 0..40 {
                let _ = write!(
                    body,
                    "<li>item {item} of a list that may be cut between items</li>"
                );
            }
            body.push_str("</ol>\n<pre><code>fn main() {\n    println!(\"a code block is never cut\");\n}</code></pre>\n");
            body.push_str("<figure><img src=\"../images/figure.png\" alt=\"a figure\"/><figcaption>A figure stays whole.</figcaption></figure>\n");
        }
    }
    if long {
        body.push_str("</div>\n");
    }
    let note = if epub3 {
        format!(
            "<p>A note<a epub:type=\"noteref\" href=\"notes.xhtml#n{number}\">{number}</a> and <a href=\"https://example.org/\">a web page</a>.</p>\n"
        )
    } else {
        format!("<p>A note<a href=\"notes.xhtml#n{number}\">{number}</a>.</p>\n")
    };
    body.push_str(&note);
    if number < book.chapters.len() {
        let _ = writeln!(
            body,
            "<p><a href=\"ch{:02}.xhtml#ch{}\">Next chapter</a></p>",
            number + 1,
            number + 1
        );
    }
    if number == 2 && epub3 {
        body.push_str("<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"block\"><mi>E</mi><mo>=</mo><mi>m</mi><msup><mi>c</mi><mn>2</mn></msup></math>\n");
        body.push_str("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"20\" height=\"20\"><defs><circle id=\"dot\" r=\"5\"/></defs><use href=\"#dot\" x=\"10\" y=\"10\"/></svg>\n");
    }
    let doctype = if epub3 {
        "<!DOCTYPE html>"
    } else {
        "<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.1//EN\" \"http://www.w3.org/TR/xhtml11/DTD/xhtml11.dtd\">"
    };
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n{doctype}\n<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\" xml:lang=\"en\">\n\
         <head><title>{}</title><link rel=\"stylesheet\" type=\"text/css\" href=\"../styles/book.css\"/></head>\n<body>\n{body}</body>\n</html>\n",
        escape(&chapter.title)
    )
}

fn notes_xhtml(book: &Book) -> String {
    let mut body = String::new();
    for number in 1..=book.chapters.len() {
        let _ = writeln!(
            body,
            "<aside epub:type=\"footnote\" id=\"n{number}\"><p>Note {number}: the [c{number}] footnote.</p></aside>"
        );
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!DOCTYPE html>\n<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\">\n\
         <head><title>Notes</title></head>\n<body>\n<h2>Notes</h2>\n{body}</body>\n</html>\n"
    )
}

const UUID: &str = "urn:uuid:7c1e7f2a-3a51-4c3d-9a9b-0f6e2d4c5b11";

/// The IDPF font obfuscation, which is its own inverse.
fn obfuscate(font: &mut [u8], identifier: &str) {
    let key = sha1_digest(identifier.as_bytes());
    for (index, byte) in font.iter_mut().take(1040).enumerate() {
        *byte ^= key[index % 20];
    }
}

/// SHA-1, for the obfuscation key only.
fn sha1_digest(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let mut message = data.to_vec();
    let bit_length = (data.len() as u64) * 8;
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_length.to_be_bytes());
    for block in message.chunks(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(block[4 * i..4 * i + 4].try_into().unwrap());
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, &word) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        for (slot, value) in h.iter_mut().zip([a, b, c, d, e]) {
            *slot = slot.wrapping_add(value);
        }
    }
    let mut out = [0u8; 20];
    for (i, value) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&value.to_be_bytes());
    }
    out
}

/// EPUB 3, with a navigation document and an NCX, a stylesheet that imports another and uses an
/// obfuscated font and an image, footnotes in a separate document, MathML and SVG.
pub fn epub3(book: &Book) -> Vec<u8> {
    let mut zip = ZipWriter::new(Vec::new());
    zip.add("mimetype", b"application/epub+zip", false).unwrap();
    zip.add(
        "META-INF/container.xml",
        b"<?xml version=\"1.0\"?><container version=\"1.0\" xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\"><rootfiles><rootfile full-path=\"OEBPS/content.opf\" media-type=\"application/oebps-package+xml\"/></rootfiles></container>",
        true,
    )
    .unwrap();
    zip.add(
        "META-INF/encryption.xml",
        b"<?xml version=\"1.0\"?><encryption xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\" xmlns:enc=\"http://www.w3.org/2001/04/xmlenc#\"><enc:EncryptedData><enc:EncryptionMethod Algorithm=\"http://www.idpf.org/2008/embedding\"/><enc:CipherData><enc:CipherReference URI=\"OEBPS/fonts/sample.ttf\"/></enc:CipherData></enc:EncryptedData></encryption>",
        true,
    )
    .unwrap();
    let mut manifest = String::new();
    let mut spine = String::new();
    let mut nav = String::new();
    let mut ncx = String::new();
    for (index, chapter) in book.chapters.iter().enumerate() {
        let number = index + 1;
        let name = format!("ch{number:02}.xhtml");
        zip.add(
            &format!("OEBPS/text/{name}"),
            chapter_xhtml(book, index, true).as_bytes(),
            true,
        )
        .unwrap();
        let properties = if number == 2 {
            " properties=\"mathml svg\""
        } else {
            ""
        };
        let _ = writeln!(
            manifest,
            "<item id=\"c{number}\" href=\"text/{name}\" media-type=\"application/xhtml+xml\"{properties}/>"
        );
        let _ = writeln!(spine, "<itemref idref=\"c{number}\" id=\"ref{number}\"/>");
        let _ = write!(
            nav,
            "<li><a href=\"text/{name}\">{}</a></li>",
            escape(&chapter.title)
        );
        let _ = write!(
            ncx,
            "<navPoint id=\"np{number}\" playOrder=\"{number}\"><navLabel><text>{}</text></navLabel><content src=\"text/{name}\"/></navPoint>",
            escape(&chapter.title)
        );
    }
    zip.add("OEBPS/text/notes.xhtml", notes_xhtml(book).as_bytes(), true)
        .unwrap();
    manifest.push_str(
        "<item id=\"notes\" href=\"text/notes.xhtml\" media-type=\"application/xhtml+xml\"/>\n",
    );
    spine.push_str("<itemref idref=\"notes\" linear=\"no\"/>\n");
    zip.add(
        "OEBPS/nav.xhtml",
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!DOCTYPE html>\n<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\"><head><title>Contents</title></head>\
             <body><nav epub:type=\"toc\"><h1>Contents</h1><ol>{nav}<li><span>Back matter</span><ol><li><a href=\"text/notes.xhtml\">Notes</a></li></ol></li></ol></nav>\
             <nav epub:type=\"landmarks\"><ol><li><a epub:type=\"bodymatter\" href=\"text/ch01.xhtml\">Start</a></li></ol></nav></body></html>"
        )
        .as_bytes(),
        true,
    )
    .unwrap();
    zip.add(
        "OEBPS/toc.ncx",
        format!("<?xml version=\"1.0\"?><ncx xmlns=\"http://www.daisy.org/z3986/2005/ncx/\" version=\"2005-1\"><head/><docTitle><text>{}</text></docTitle><navMap>{ncx}</navMap></ncx>", escape(&book.title)).as_bytes(),
        true,
    )
    .unwrap();
    zip.add(
        "OEBPS/styles/book.css",
        b"@charset \"utf-8\";\n@import url(\"extra.css\");\n@import url(\"https://fonts.example.com/evil.css\");\n\
          @font-face { font-family: \"Sample\"; src: url(../fonts/sample.ttf) format(\"truetype\"), url(https://evil.example/f.woff); }\n\
          body { font-family: \"Sample\", serif; }\nh2 { background: url(../images/figure.png) no-repeat; }\ntable.data { border-collapse: collapse; }\n",
        true,
    )
    .unwrap();
    zip.add(
        "OEBPS/styles/extra.css",
        b"p { text-indent: 1.2em; margin: 0; }\n",
        true,
    )
    .unwrap();
    let mut font = fake_font();
    obfuscate(&mut font, UUID);
    zip.add("OEBPS/fonts/sample.ttf", &font, true).unwrap();
    zip.add("OEBPS/images/cover.png", PNG, false).unwrap();
    zip.add("OEBPS/images/figure.png", PNG, false).unwrap();
    manifest.push_str(
        "<item id=\"nav\" href=\"nav.xhtml\" media-type=\"application/xhtml+xml\" properties=\"nav\"/>\n\
         <item id=\"ncx\" href=\"toc.ncx\" media-type=\"application/x-dtbncx+xml\"/>\n\
         <item id=\"css\" href=\"styles/book.css\" media-type=\"text/css\"/>\n\
         <item id=\"css2\" href=\"styles/extra.css\" media-type=\"text/css\"/>\n\
         <item id=\"font\" href=\"fonts/sample.ttf\" media-type=\"font/ttf\"/>\n\
         <item id=\"cover\" href=\"images/cover.png\" media-type=\"image/png\" properties=\"cover-image\"/>\n\
         <item id=\"figure\" href=\"images/figure.png\" media-type=\"image/png\"/>\n",
    );
    let opf = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\" unique-identifier=\"uid\">\n\
         <metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n<dc:identifier id=\"uid\">{UUID}</dc:identifier>\n\
         <dc:identifier id=\"isbn\">urn:isbn:9780306406157</dc:identifier>\n<dc:title id=\"t\">{}</dc:title>\n<meta refines=\"#t\" property=\"title-type\">main</meta>\n\
         <dc:creator id=\"a\">{}</dc:creator>\n<meta refines=\"#a\" property=\"role\" scheme=\"marc:relators\">aut</meta>\n<meta refines=\"#a\" property=\"file-as\">Lima, Ana</meta>\n\
         <dc:language>{}</dc:language>\n<dc:publisher>Musaeum Fixtures</dc:publisher>\n<dc:date>2026-11-03</dc:date>\n\
         <dc:description>&lt;p&gt;A book for &lt;b&gt;tests&lt;/b&gt;.&lt;/p&gt;</dc:description>\n<dc:subject>Testing</dc:subject>\n\
         <meta property=\"belongs-to-collection\" id=\"s\">Fixtures</meta>\n<meta refines=\"#s\" property=\"collection-type\">series</meta>\n<meta refines=\"#s\" property=\"group-position\">2</meta>\n\
         <meta property=\"dcterms:modified\">2026-11-03T00:00:00Z</meta>\n</metadata>\n<manifest>\n{manifest}</manifest>\n<spine toc=\"ncx\">\n{spine}</spine>\n</package>\n",
        escape(&book.title),
        escape(&book.author),
        book.language
    );
    zip.add("OEBPS/content.opf", opf.as_bytes(), true).unwrap();
    zip.finish().unwrap();
    zip.into_inner()
}

/// EPUB 2, the way old books are: an NCX and no navigation document, `opf:` attributes, a guide,
/// HTML entities that only the XHTML doctype defines, one chapter that is not well-formed XML and
/// a picture in the spine.
pub fn epub2(book: &Book) -> Vec<u8> {
    let mut zip = ZipWriter::new(Vec::new());
    zip.add("mimetype", b"application/epub+zip", false).unwrap();
    zip.add(
        "META-INF/container.xml",
        b"<?xml version=\"1.0\"?><container version=\"1.0\" xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\"><rootfiles><rootfile full-path=\"content.opf\" media-type=\"application/oebps-package+xml\"/></rootfiles></container>",
        true,
    )
    .unwrap();
    let mut manifest = String::new();
    let mut spine = String::new();
    let mut ncx = String::new();
    for (index, chapter) in book.chapters.iter().enumerate().take(3) {
        let number = index + 1;
        let name = format!("ch{number:02}.xhtml");
        let mut text = chapter_xhtml(book, index, false).replace("../styles/book.css", "book.css");
        if number == 1 {
            text = text.replace("</h2>", "&nbsp;&mdash;&nbsp;start</h2>");
        }
        if number == 2 {
            // Not well-formed: a paragraph that never ends and a line break that never closes.
            text = text.replacen(
                "<p id=\"c2p1\">",
                "<p class=\"broken\">An open paragraph<br><p id=\"c2p1\">",
                1,
            );
        }
        zip.add(&name, text.as_bytes(), true).unwrap();
        let _ = writeln!(
            manifest,
            "<item id=\"c{number}\" href=\"{name}\" media-type=\"application/xhtml+xml\"/>"
        );
        let _ = writeln!(spine, "<itemref idref=\"c{number}\"/>");
        let _ = write!(
            ncx,
            "<navPoint id=\"np{number}\" playOrder=\"{number}\"><navLabel><text>{}</text></navLabel><content src=\"{name}#ch{number}\"/></navPoint>",
            escape(&chapter.title)
        );
    }
    zip.add("plate.png", PNG, false).unwrap();
    manifest.push_str("<item id=\"plate\" href=\"plate.png\" media-type=\"image/png\"/>\n");
    spine.push_str("<itemref idref=\"plate\"/>\n");
    zip.add("notes.xhtml", notes_xhtml(book).as_bytes(), true)
        .unwrap();
    manifest.push_str(
        "<item id=\"notes\" href=\"notes.xhtml\" media-type=\"application/xhtml+xml\"/>\n",
    );
    zip.add("book.css", b"p { margin: 0; }\n", true).unwrap();
    manifest.push_str("<item id=\"css\" href=\"book.css\" media-type=\"text/css\"/>\n<item id=\"ncx\" href=\"toc.ncx\" media-type=\"application/x-dtbncx+xml\"/>\n");
    zip.add(
        "toc.ncx",
        format!("<?xml version=\"1.0\"?><ncx xmlns=\"http://www.daisy.org/z3986/2005/ncx/\" version=\"2005-1\"><head/><docTitle><text>{}</text></docTitle><navMap>{ncx}</navMap></ncx>", escape(&book.title)).as_bytes(),
        true,
    )
    .unwrap();
    let opf = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<package xmlns=\"http://www.idpf.org/2007/opf\" version=\"2.0\" unique-identifier=\"BookId\">\n\
         <metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:opf=\"http://www.idpf.org/2007/opf\">\n\
         <dc:title>{} (EPUB 2)</dc:title>\n<dc:creator opf:role=\"aut\" opf:file-as=\"Lima, Ana\">{}</dc:creator>\n\
         <dc:identifier id=\"BookId\" opf:scheme=\"ISBN\">978-0-306-40615-7</dc:identifier>\n<dc:language>en</dc:language>\n\
         <dc:date opf:event=\"publication\">2001</dc:date>\n<meta name=\"cover\" content=\"plate\"/>\n\
         <meta name=\"calibre:series\" content=\"Old Fixtures\"/>\n<meta name=\"calibre:series_index\" content=\"1\"/>\n</metadata>\n\
         <manifest>\n{manifest}</manifest>\n<spine toc=\"ncx\">\n{spine}</spine>\n\
         <guide><reference type=\"text\" title=\"Start\" href=\"ch01.xhtml\"/></guide>\n</package>\n",
        escape(&book.title),
        escape(&book.author)
    );
    zip.add("content.opf", opf.as_bytes(), true).unwrap();
    zip.finish().unwrap();
    zip.into_inner()
}

// ---------------------------------------------------------------------------------------------
// Mobipocket and KF8

/// A variable-width integer, forward: seven bits a byte, the last byte marked.
fn varint(value: u32, out: &mut Vec<u8>) {
    let mut groups = vec![(value & 0x7F) as u8 | 0x80];
    let mut rest = value >> 7;
    while rest > 0 {
        groups.push((rest & 0x7F) as u8);
        rest >>= 7;
    }
    groups.reverse();
    out.extend_from_slice(&groups);
}

/// PalmDOC's LZ77 on one record: back references found through a table of three-byte keys, space
/// pairs, and escaped literals.
fn palmdoc(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut table: std::collections::HashMap<[u8; 3], Vec<usize>> =
        std::collections::HashMap::new();
    let mut i = 0;
    while i < input.len() {
        let mut best = (0usize, 0usize);
        if i + 3 <= input.len() {
            let key = [input[i], input[i + 1], input[i + 2]];
            if let Some(candidates) = table.get(&key) {
                for &j in candidates.iter().rev().take(64) {
                    let distance = i - j;
                    if distance > 2047 {
                        break;
                    }
                    let mut length = 0;
                    while length < 10
                        && i + length < input.len()
                        && input[j + length] == input[i + length]
                    {
                        length += 1;
                    }
                    if length > best.1 {
                        best = (distance, length);
                    }
                }
            }
        }
        let step = if best.1 >= 3 {
            let pair = 0x8000u16 | ((best.0 as u16) << 3) | (best.1 as u16 - 3);
            out.extend_from_slice(&pair.to_be_bytes());
            best.1
        } else if input[i] == b' ' && i + 1 < input.len() && (0x40..=0x7F).contains(&input[i + 1]) {
            out.push(input[i + 1] ^ 0x80);
            2
        } else if input[i] == 0 || (0x09..=0x7F).contains(&input[i]) {
            out.push(input[i]);
            1
        } else {
            let run = input[i..]
                .iter()
                .take(8)
                .take_while(|&&b| !(b == 0 || (0x09..=0x7F).contains(&b)))
                .count()
                .max(1);
            out.push(run as u8);
            out.extend_from_slice(&input[i..i + run]);
            run
        };
        for k in i..i + step {
            if k + 3 <= input.len() {
                table
                    .entry([input[k], input[k + 1], input[k + 2]])
                    .or_default()
                    .push(k);
            }
        }
        i += step;
    }
    out
}

/// The text in records of up to 4 KiB, never splitting a UTF-8 character, each compressed and
/// followed by its trailing entries: a multibyte byte and one entry of one byte.
fn text_records(text: &[u8]) -> Vec<Vec<u8>> {
    let mut records = Vec::new();
    let mut at = 0;
    while at < text.len() {
        let mut end = (at + 4096).min(text.len());
        while end < text.len() && text[end] & 0xC0 == 0x80 {
            end -= 1;
        }
        let mut record = palmdoc(&text[at..end]);
        record.push(0x00);
        record.push(0x81);
        records.push(record);
        at = end;
    }
    records
}

/// A tag of an index entry and its values.
type Tag = (u8, Vec<u32>);

/// An INDX index: a header record with its TAGX, one record of entries, and their strings.
struct Index {
    tags: Vec<(u8, u8, u8, u8)>,
    entries: Vec<(Vec<u8>, Vec<Tag>)>,
    strings: Vec<u8>,
}

impl Index {
    fn new(tags: &[(u8, u8, u8, u8)]) -> Self {
        Self {
            tags: tags.to_vec(),
            entries: Vec::new(),
            strings: Vec::new(),
        }
    }

    /// Adds a string to the CNCX record and returns its offset.
    fn string(&mut self, text: &str) -> u32 {
        let at = self.strings.len() as u32;
        varint(text.len() as u32, &mut self.strings);
        self.strings.extend_from_slice(text.as_bytes());
        at
    }

    fn records(&self) -> Vec<Vec<u8>> {
        let header_length = 192u32;
        let mut tagx = b"TAGX".to_vec();
        tagx.extend_from_slice(&(12 + 4 * (self.tags.len() as u32 + 1)).to_be_bytes());
        tagx.extend_from_slice(&1u32.to_be_bytes());
        for &(tag, count, mask, end) in &self.tags {
            tagx.extend_from_slice(&[tag, count, mask, end]);
        }
        tagx.extend_from_slice(&[0, 0, 0, 1]);
        let mut head = vec![0u8; header_length as usize];
        head[0..4].copy_from_slice(b"INDX");
        head[4..8].copy_from_slice(&header_length.to_be_bytes());
        head[24..28].copy_from_slice(&1u32.to_be_bytes());
        head[28..32].copy_from_slice(&65001u32.to_be_bytes());
        head[36..40].copy_from_slice(&(self.entries.len() as u32).to_be_bytes());
        head[52..56].copy_from_slice(&u32::from(!self.strings.is_empty()).to_be_bytes());
        head[180..184].copy_from_slice(&header_length.to_be_bytes());
        head.extend_from_slice(&tagx);

        let mut record = vec![0u8; header_length as usize];
        record[0..4].copy_from_slice(b"INDX");
        record[4..8].copy_from_slice(&header_length.to_be_bytes());
        let mut offsets = Vec::new();
        for (name, values) in &self.entries {
            offsets.push(record.len() as u16);
            record.push(name.len() as u8);
            record.extend_from_slice(name);
            let mut control = 0u8;
            let mut data = Vec::new();
            for &(tag, per_entry, mask, end) in &self.tags {
                if end == 1 {
                    continue;
                }
                if let Some((_, list)) = values.iter().find(|(t, _)| *t == tag) {
                    // How many entries of values there are, shifted into the mask's bits.
                    let entries = (list.len() / usize::from(per_entry.max(1))) as u8;
                    control |= (entries << mask.trailing_zeros()) & mask;
                    for &value in list {
                        varint(value, &mut data);
                    }
                }
            }
            record.push(control);
            record.extend_from_slice(&data);
        }
        while !record.len().is_multiple_of(4) {
            record.push(0);
        }
        let idxt = record.len() as u32;
        record.extend_from_slice(b"IDXT");
        for offset in &offsets {
            record.extend_from_slice(&offset.to_be_bytes());
        }
        while !record.len().is_multiple_of(4) {
            record.push(0);
        }
        record[20..24].copy_from_slice(&idxt.to_be_bytes());
        record[24..28].copy_from_slice(&(self.entries.len() as u32).to_be_bytes());
        let mut records = vec![head, record];
        if !self.strings.is_empty() {
            records.push(self.strings.clone());
        }
        records
    }
}

fn exth(entries: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut body = Vec::new();
    for (kind, data) in entries {
        body.extend_from_slice(&kind.to_be_bytes());
        body.extend_from_slice(&(data.len() as u32 + 8).to_be_bytes());
        body.extend_from_slice(data);
    }
    let mut out = b"EXTH".to_vec();
    out.extend_from_slice(&(body.len() as u32 + 12).to_be_bytes());
    out.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
    out
}

/// What record 0 needs to know about the rest of the file.
struct Layout {
    version: u32,
    text_length: usize,
    text_records: usize,
    first_resource: u32,
    ncx: u32,
    fdst: u32,
    fragments: u32,
    skeletons: u32,
}

fn record_zero(book: &Book, layout: &Layout, exth_entries: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let header_length: u32 = if layout.version >= 8 { 264 } else { 232 };
    let mut r0 = vec![0u8; 16 + header_length as usize];
    r0[0..2].copy_from_slice(&2u16.to_be_bytes());
    r0[4..8].copy_from_slice(&(layout.text_length as u32).to_be_bytes());
    r0[8..10].copy_from_slice(&(layout.text_records as u16).to_be_bytes());
    r0[10..12].copy_from_slice(&4096u16.to_be_bytes());
    r0[16..20].copy_from_slice(b"MOBI");
    let set = |r0: &mut Vec<u8>, at: usize, value: u32| {
        r0[at..at + 4].copy_from_slice(&value.to_be_bytes())
    };
    set(&mut r0, 20, header_length);
    set(&mut r0, 24, 2);
    set(&mut r0, 28, 65001);
    set(&mut r0, 32, 0x5EED_0001);
    set(&mut r0, 36, layout.version);
    for at in (40..80).step_by(4) {
        set(&mut r0, at, 0xFFFF_FFFF);
    }
    set(&mut r0, 80, layout.first_resource);
    set(&mut r0, 92, 9);
    set(&mut r0, 104, layout.version);
    set(&mut r0, 108, layout.first_resource);
    set(&mut r0, 112, 0);
    set(&mut r0, 116, 0);
    set(&mut r0, 128, 0x50);
    set(&mut r0, 164, 0xFFFF_FFFF);
    set(&mut r0, 168, 0xFFFF_FFFF);
    if layout.version >= 8 {
        set(&mut r0, 0xC0, layout.fdst);
        set(&mut r0, 0xC4, 2);
    } else {
        r0[0xC0..0xC2].copy_from_slice(&1u16.to_be_bytes());
        r0[0xC2..0xC4].copy_from_slice(&(layout.text_records as u16).to_be_bytes());
    }
    set(&mut r0, 0xE0, 0xFFFF_FFFF);
    set(&mut r0, 0xE8, 0xFFFF_FFFF);
    set(&mut r0, 0xEC, 0xFFFF_FFFF);
    // Multibyte bytes and one trailing entry after every text record.
    set(&mut r0, 0xF0, 0b11);
    set(&mut r0, 0xF4, layout.ncx);
    if layout.version >= 8 {
        set(&mut r0, 0xF8, layout.fragments);
        set(&mut r0, 0xFC, layout.skeletons);
        set(&mut r0, 0x100, 0xFFFF_FFFF);
        set(&mut r0, 0x104, 0xFFFF_FFFF);
    }
    r0.extend_from_slice(&exth(exth_entries));
    let name_at = r0.len() as u32;
    r0.extend_from_slice(book.title.as_bytes());
    set(&mut r0, 84, name_at);
    set(&mut r0, 88, book.title.len() as u32);
    r0.extend_from_slice(&[0; 4]);
    while !r0.len().is_multiple_of(4) {
        r0.push(0);
    }
    r0
}

fn exth_entries(book: &Book, cover: Option<u32>) -> Vec<(u32, Vec<u8>)> {
    let mut entries = vec![
        (100, book.author.as_bytes().to_vec()),
        (101, b"Musaeum Fixtures".to_vec()),
        (103, b"A book for tests.".to_vec()),
        (104, b"9780306406157".to_vec()),
        (106, b"2026-11-03".to_vec()),
        (503, book.title.as_bytes().to_vec()),
        (524, book.language.as_bytes().to_vec()),
        (501, b"EBOK".to_vec()),
    ];
    if let Some(cover) = cover {
        entries.push((201, cover.to_be_bytes().to_vec()));
    }
    entries
}

/// A Palm database around `records`.
fn pdb(name: &str, records: &[Vec<u8>]) -> Vec<u8> {
    let mut out = vec![0u8; 78];
    let short: Vec<u8> = name
        .bytes()
        .filter(|b| b.is_ascii_alphanumeric() || *b == b'_')
        .take(31)
        .collect();
    out[..short.len()].copy_from_slice(&short);
    out[36..40].copy_from_slice(&0x5EED_0000u32.to_be_bytes());
    out[40..44].copy_from_slice(&0x5EED_0000u32.to_be_bytes());
    out[60..68].copy_from_slice(b"BOOKMOBI");
    out[68..72].copy_from_slice(&(2 * records.len() as u32 - 1).to_be_bytes());
    out[76..78].copy_from_slice(&(records.len() as u16).to_be_bytes());
    let mut offset = 78 + 8 * records.len() + 2;
    for (index, record) in records.iter().enumerate() {
        out.extend_from_slice(&(offset as u32).to_be_bytes());
        out.extend_from_slice(&(2 * index as u32).to_be_bytes());
        offset += record.len();
    }
    out.extend_from_slice(&[0, 0]);
    for record in records {
        out.extend_from_slice(record);
    }
    out
}

const EOF_RECORD: &[u8] = b"\xE9\x8E\r\n";

/// MOBI 6: one HTML stream with page breaks, `filepos` links into it by byte offset, a guide, a
/// picture by record index, and an NCX.
pub fn mobi6(book: &Book) -> Vec<u8> {
    // Written with ten-digit placeholders, filled in once every offset is known.
    let mut html = String::from(
        "<html><head><guide><reference title=\"Contents\" type=\"toc\" filepos=TOC_______/></guide></head><body>",
    );
    html.push_str("<p><img recindex=\"00001\"/></p><mbp:pagebreak/><h1>Contents</h1>");
    for number in 1..=book.chapters.len() {
        let _ = write!(
            html,
            "<p><a filepos=CH{number:08}>{}</a></p>",
            escape(&book.chapters[number - 1].title)
        );
    }
    html.push_str("<mbp:pagebreak/>");
    let mut starts = Vec::new();
    for (index, chapter) in book.chapters.iter().enumerate() {
        starts.push(html.len());
        let _ = write!(html, "<h2>{}</h2>", escape(&chapter.title));
        for paragraph in &chapter.paragraphs {
            let _ = write!(html, "<p width=\"0pt\">{}</p>", escape(paragraph));
        }
        if index + 1 < book.chapters.len() {
            let _ = write!(
                html,
                "<p><a filepos=CH{:08}>Next chapter</a></p>",
                index + 2
            );
            html.push_str("<mbp:pagebreak/>");
        }
    }
    html.push_str("</body></html>");
    let contents = html.find("<h1>Contents</h1>").expect("contents");
    let mut text = html.replace("TOC_______", &format!("{contents:010}"));
    for (index, start) in starts.iter().enumerate() {
        text = text.replace(&format!("CH{:08}", index + 1), &format!("{start:010}"));
    }
    let text = text.into_bytes();
    let records = text_records(&text);
    let mut ncx = Index::new(&[
        (1, 1, 0x01, 0),
        (2, 1, 0x02, 0),
        (3, 1, 0x04, 0),
        (4, 1, 0x08, 0),
    ]);
    for (index, chapter) in book.chapters.iter().enumerate() {
        let label = ncx.string(&chapter.title);
        let end = starts.get(index + 1).copied().unwrap_or(text.len());
        ncx.entries.push((
            format!("{index:x}").into_bytes(),
            vec![
                (1, vec![starts[index] as u32]),
                (2, vec![(end - starts[index]) as u32]),
                (3, vec![label]),
                (4, vec![0]),
            ],
        ));
    }
    let first_resource = 1 + records.len() as u32;
    let ncx_at = first_resource + 1;
    let layout = Layout {
        version: 6,
        text_length: text.len(),
        text_records: records.len(),
        first_resource,
        ncx: ncx_at,
        fdst: 0xFFFF_FFFF,
        fragments: 0xFFFF_FFFF,
        skeletons: 0xFFFF_FFFF,
    };
    let mut all = vec![record_zero(book, &layout, &exth_entries(book, Some(0)))];
    all.extend(records);
    all.push(PNG.to_vec());
    all.extend(ncx.records());
    all.push(EOF_RECORD.to_vec());
    pdb(&book.title, &all)
}

fn base32(value: usize, width: usize) -> String {
    const DIGITS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUV";
    let mut out = vec![b'0'; width];
    let mut rest = value;
    for slot in out.iter_mut().rev() {
        *slot = DIGITS[rest % 32];
        rest /= 32;
    }
    String::from_utf8(out).expect("ASCII")
}

/// KF8, as an AZW3 file: one skeleton and one fragment per chapter, a stylesheet in a second flow,
/// pictures and an obfuscated, compressed font by `kindle:embed`, links by `kindle:pos`, and an NCX.
pub fn kf8(book: &Book) -> Vec<u8> {
    // Fragment bodies first, so link offsets inside them are known.
    let mut bodies: Vec<String> = Vec::new();
    let mut note_offsets: Vec<usize> = Vec::new();
    for (index, chapter) in book.chapters.iter().enumerate() {
        let number = index + 1;
        let mut body = String::new();
        let _ = write!(
            body,
            "<h2 id=\"ch{number}\" aid=\"H{number}\">{}</h2>",
            escape(&chapter.title)
        );
        if index == 0 {
            body.push_str(
                "<p aid=\"IMG\"><img src=\"kindle:embed:0002?mime=image/png\" alt=\"\"/></p>",
            );
        }
        for (p, paragraph) in chapter.paragraphs.iter().enumerate() {
            let _ = write!(body, "<p aid=\"P{number}x{p}\">{}</p>", escape(paragraph));
        }
        let _ = write!(
            body,
            "<p>See <a href=\"kindle:pos:fid:{}:off:NOTEOFFSET\">the note</a>.</p>",
            base32(index, 4)
        );
        note_offsets.push(body.len());
        let _ = write!(
            body,
            "<p aid=\"N{number}\">Note {number}: the [c{number}] footnote.</p>"
        );
        if number < book.chapters.len() {
            let _ = write!(
                body,
                "<p><a href=\"kindle:pos:fid:{}:off:{}\">Next chapter</a></p>",
                base32(index + 1, 4),
                base32(0, 10)
            );
        }
        bodies.push(body);
    }
    // The note link's offset: where the note's tag starts within the fragment.
    // The placeholder is as long as the number, so nothing after it moves.
    for (index, body) in bodies.iter_mut().enumerate() {
        *body = body.replace("NOTEOFFSET", &base32(note_offsets[index], 10));
    }

    let css = "@font-face { font-family: \"Sample\"; src: url(kindle:embed:0003?mime=application/x-font-ttf); }\nbody { font-family: \"Sample\", serif; }\nh2 { background: url(kindle:embed:0002?mime=image/png); }\n";
    let mut flow = Vec::new();
    let mut skeletons = Index::new(&[(1, 1, 0x03, 0), (6, 2, 0x0C, 0)]);
    let mut fragments = Index::new(&[
        (2, 1, 0x01, 0),
        (3, 1, 0x02, 0),
        (4, 1, 0x04, 0),
        (6, 2, 0x08, 0),
    ]);
    let mut ncx = Index::new(&[
        (1, 1, 0x01, 0),
        (2, 1, 0x02, 0),
        (3, 1, 0x04, 0),
        (4, 1, 0x08, 0),
        (6, 2, 0x10, 0),
    ]);
    for (index, chapter) in book.chapters.iter().enumerate() {
        let skeleton = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>{}</title>\
             <link rel=\"stylesheet\" type=\"text/css\" href=\"kindle:flow:0001?mime=text/css\"/></head><body aid=\"B{index}\"></body></html>",
            escape(&chapter.title)
        );
        let start = flow.len();
        let insert = start + skeleton.find("</body>").expect("a body");
        flow.extend_from_slice(skeleton.as_bytes());
        let fragment_start = flow.len();
        flow.extend_from_slice(bodies[index].as_bytes());
        skeletons.entries.push((
            format!("SKEL{index:07}").into_bytes(),
            vec![(1, vec![1]), (6, vec![start as u32, skeleton.len() as u32])],
        ));
        let selector = fragments.string(&format!("P-//*[@aid='B{index}']"));
        fragments.entries.push((
            format!("{insert:010}").into_bytes(),
            vec![
                (2, vec![selector]),
                (3, vec![index as u32]),
                (4, vec![index as u32]),
                (
                    6,
                    vec![(fragment_start - start) as u32, bodies[index].len() as u32],
                ),
            ],
        ));
        let label = ncx.string(&chapter.title);
        ncx.entries.push((
            format!("{index:x}").into_bytes(),
            vec![
                (1, vec![insert as u32]),
                (2, vec![bodies[index].len() as u32]),
                (3, vec![label]),
                (4, vec![0]),
                (6, vec![index as u32, 0]),
            ],
        ));
    }
    let flow0 = flow.len();
    flow.extend_from_slice(css.as_bytes());
    let records = text_records(&flow);

    // The font: compressed with zlib, then its first bytes XORed with a key.
    let font = fake_font();
    let mut compressed = {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::new(6));
        encoder.write_all(&font).unwrap();
        encoder.finish().unwrap()
    };
    let key: Vec<u8> = (0..16u8)
        .map(|b| b.wrapping_mul(37).wrapping_add(11))
        .collect();
    for (index, byte) in compressed.iter_mut().take(1040).enumerate() {
        *byte ^= key[index % key.len()];
    }
    let mut font_record = b"FONT".to_vec();
    font_record.extend_from_slice(&(font.len() as u32).to_be_bytes());
    font_record.extend_from_slice(&3u32.to_be_bytes());
    font_record.extend_from_slice(&(24 + key.len() as u32).to_be_bytes());
    font_record.extend_from_slice(&(key.len() as u32).to_be_bytes());
    font_record.extend_from_slice(&24u32.to_be_bytes());
    font_record.extend_from_slice(&key);
    font_record.extend_from_slice(&compressed);

    let first_resource = 1 + records.len() as u32;
    let resources = [PNG.to_vec(), PNG.to_vec(), font_record];
    let fragments_at = first_resource + resources.len() as u32;
    let fragment_records = fragments.records();
    let skeletons_at = fragments_at + fragment_records.len() as u32;
    let skeleton_records = skeletons.records();
    let ncx_at = skeletons_at + skeleton_records.len() as u32;
    let ncx_records = ncx.records();
    let fdst_at = ncx_at + ncx_records.len() as u32;
    let mut fdst = b"FDST".to_vec();
    fdst.extend_from_slice(&12u32.to_be_bytes());
    fdst.extend_from_slice(&2u32.to_be_bytes());
    for (start, end) in [(0, flow0), (flow0, flow.len())] {
        fdst.extend_from_slice(&(start as u32).to_be_bytes());
        fdst.extend_from_slice(&(end as u32).to_be_bytes());
    }
    let layout = Layout {
        version: 8,
        text_length: flow.len(),
        text_records: records.len(),
        first_resource,
        ncx: ncx_at,
        fdst: fdst_at,
        fragments: fragments_at,
        skeletons: skeletons_at,
    };
    let mut all = vec![record_zero(book, &layout, &exth_entries(book, Some(0)))];
    all.extend(records);
    all.extend(resources);
    all.extend(fragment_records);
    all.extend(skeleton_records);
    all.extend(ncx_records);
    all.push(fdst);
    all.push(EOF_RECORD.to_vec());
    pdb(&book.title, &all)
}

// ---------------------------------------------------------------------------------------------
// FictionBook, text and Markdown

/// FictionBook 2, in `encoding`: nested sections, an epigraph, a poem, a table, notes in their own
/// body and pictures in base64.
pub fn fb2(book: &Book, encoding: &str) -> Vec<u8> {
    let mut sections = String::new();
    for (index, chapter) in book.chapters.iter().enumerate() {
        let number = index + 1;
        let _ = write!(
            sections,
            "<section id=\"ch{number}\"><title><p>{}</p></title>",
            escape(&chapter.title)
        );
        if number == 1 {
            sections.push_str("<epigraph><p>An epigraph.</p><text-author>Someone</text-author></epigraph><image l:href=\"#figure.png\"/>");
        }
        let (first, rest) = chapter.paragraphs.split_at(chapter.paragraphs.len() / 2);
        for paragraph in first {
            let _ = write!(sections, "<p>{}</p>", escape(paragraph));
        }
        let _ = write!(sections, "<section><title><p>Part {number}.2</p></title>");
        for paragraph in rest {
            let _ = write!(sections, "<p>{}</p>", escape(paragraph));
        }
        let _ = write!(
            sections,
            "<p>A note<a l:href=\"#n{number}\" type=\"note\">{number}</a>.</p></section>"
        );
        if number == 2 {
            sections.push_str("<poem><stanza><v>A line of verse</v><v>and another</v></stanza></poem><empty-line/>");
            sections.push_str(
                "<table><tr><th>Key</th><th>Value</th></tr><tr><td>a</td><td>1</td></tr></table>",
            );
        }
        sections.push_str("</section>\n");
    }
    let mut notes = String::new();
    for number in 1..=book.chapters.len() {
        let _ = write!(
            notes,
            "<section id=\"n{number}\"><title><p>{number}</p></title><p>Note {number}: the [c{number}] footnote.</p></section>"
        );
    }
    let png = base64(PNG);
    let text = format!(
        "<?xml version=\"1.0\" encoding=\"{encoding}\"?>\n<FictionBook xmlns=\"http://www.gribuser.ru/xml/fictionbook/2.0\" xmlns:l=\"http://www.w3.org/1999/xlink\">\n\
         <description><title-info><genre>sf</genre><author><first-name>Ana</first-name><last-name>Lima</last-name></author>\
         <book-title>{}</book-title><annotation><p>A book for tests.</p></annotation><date value=\"2026-11-03\">2026</date><lang>{}</lang>\
         <coverpage><image l:href=\"#cover.png\"/></coverpage><sequence name=\"Fixtures\" number=\"3\"/></title-info>\
         <document-info><id>musaeum-fixture-fb2</id><version>1.0</version></document-info>\
         <publish-info><publisher>Musaeum Fixtures</publisher><year>2026</year><isbn>978-0-306-40615-7</isbn></publish-info></description>\n\
         <body><title><p>{}</p></title>\n{sections}</body>\n<body name=\"notes\"><title><p>Notes</p></title>{notes}</body>\n\
         <binary id=\"cover.png\" content-type=\"image/png\">{png}</binary>\n<binary id=\"figure.png\" content-type=\"image/png\">{png}</binary>\n</FictionBook>\n",
        escape(&book.title),
        book.language,
        escape(&book.title)
    );
    encode(&text, encoding)
}

/// A book in Russian, for a FictionBook in Windows-1251.
pub fn russian_book() -> Book {
    let paragraphs: Vec<String> = (1..=30)
        .map(|p| format!("[c1p{p}] Это предложение номер {p} в книге о тихой реке и старом окне."))
        .collect();
    Book {
        title: "Тихая река".to_owned(),
        author: "Анна Лима".to_owned(),
        language: "ru".to_owned(),
        chapters: vec![Chapter {
            title: "Глава 1".to_owned(),
            paragraphs,
        }],
    }
}

fn encode(text: &str, encoding: &str) -> Vec<u8> {
    match encoding {
        "windows-1251" => text
            .chars()
            .map(|c| match c {
                'А'..='я' => (c as u32 - 'А' as u32 + 0xC0) as u8,
                'Ё' => 0xA8,
                'ё' => 0xB8,
                c if c.is_ascii() => c as u8,
                _ => b'?',
            })
            .collect(),
        "windows-1252" => text
            .chars()
            .map(|c| {
                if (c as u32) < 0x100 {
                    c as u32 as u8
                } else {
                    b'?'
                }
            })
            .collect(),
        _ => text.as_bytes().to_vec(),
    }
}

fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let bytes = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let value = (u32::from(bytes[0]) << 16) | (u32::from(bytes[1]) << 8) | u32::from(bytes[2]);
        for index in 0..4 {
            if index <= chunk.len() {
                out.push(ALPHABET[((value >> (18 - 6 * index)) & 0x3F) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Plain text, hard-wrapped at 70 columns with blank lines between paragraphs, headings the
/// heuristics know, and a contents list they must not take for chapters.
pub fn txt(book: &Book, encoding: &str) -> Vec<u8> {
    let mut text = format!("{}\n\nby {}\n\nContents\n\n", book.title, book.author);
    for chapter in &book.chapters {
        let _ = write!(text, "{}\n\n", chapter.title);
    }
    for chapter in &book.chapters {
        let _ = write!(text, "\n{}\n\n", chapter.title);
        for paragraph in &chapter.paragraphs {
            let mut line = String::new();
            for word in paragraph.split(' ') {
                if line.chars().count() + word.chars().count() > 70 {
                    text.push_str(line.trim_end());
                    text.push_str("\r\n");
                    line.clear();
                }
                line.push_str(word);
                line.push(' ');
            }
            text.push_str(line.trim_end());
            text.push_str("\r\n\r\n");
        }
    }
    encode(&text, encoding)
}

/// Markdown with front matter, headings, a table, code, a footnote, an image as a data URL and raw
/// HTML with a script in it.
pub fn markdown(book: &Book) -> Vec<u8> {
    let mut text = format!(
        "---\ntitle: \"{}\"\nauthor: {}\nlang: {}\n---\n\n",
        book.title, book.author, book.language
    );
    for (index, chapter) in book.chapters.iter().enumerate() {
        let number = index + 1;
        let _ = write!(text, "# {}\n\n", chapter.title);
        for (p, paragraph) in chapter.paragraphs.iter().enumerate() {
            let _ = write!(text, "{paragraph}\n\n");
            if p == 2 {
                let _ = write!(
                    text,
                    "## Section {number}.1\n\n| Key | Value |\n|-----|-------|\n| a | {number} |\n\n```rust\nfn main() {{ println!(\"{number}\"); }}\n```\n\n"
                );
            }
        }
        let _ = write!(
            text,
            "A note.[^n{number}]\n\n[^n{number}]: Note {number}: the [c{number}] footnote.\n\n"
        );
        if number == 1 {
            let _ = write!(
                text,
                "![a dot](data:image/png;base64,{})\n\n<div onclick=\"alert(1)\"><script>alert(1)</script>Raw HTML</div>\n\n",
                base64(PNG)
            );
        }
    }
    text.into_bytes()
}

/// Writes every flowing fixture into `out`.
pub fn write_flow_fixtures(out: &Path) -> io::Result<()> {
    let short = book(false);
    let long = book(true);
    std::fs::write(out.join("flow.epub"), epub3(&long))?;
    std::fs::write(out.join("flow-epub2.epub"), epub2(&short))?;
    std::fs::write(out.join("flow.mobi"), mobi6(&short))?;
    std::fs::write(out.join("flow.azw3"), kf8(&short))?;
    std::fs::write(out.join("flow.fb2"), fb2(&short, "utf-8"))?;
    std::fs::write(
        out.join("flow-1251.fb2"),
        fb2(&russian_book(), "windows-1251"),
    )?;
    std::fs::write(out.join("flow.txt"), txt(&long, "windows-1252"))?;
    std::fs::write(out.join("flow.md"), markdown(&short))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_matches_a_known_digest() {
        let digest = sha1_digest(b"abc");
        assert_eq!(digest[..4], [0xA9, 0x99, 0x3E, 0x36]);
    }

    #[test]
    fn varints_end_with_their_marked_byte() {
        let mut out = Vec::new();
        varint(300, &mut out);
        assert_eq!(out, [0x02, 0xAC]);
    }
}

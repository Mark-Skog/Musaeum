//! Small real books, for testing a library and not a reader: what matters in them is what they say
//! about themselves, and that each has bytes of its own. Deterministic, like everything here.

use std::fmt::Write as _;

use crate::XorShift;
use crate::epub::{ZipWriter, xml_escape};

/// What an EPUB says about itself.
#[derive(Clone, Debug, Default)]
pub struct EpubMeta<'a> {
    /// `dc:title`.
    pub title: &'a str,
    /// `dc:creator`, in order.
    pub creators: &'a [&'a str],
    /// The `dc:identifier` the package names as unique.
    pub identifier: Option<&'a str>,
    /// `dc:language`.
    pub language: Option<&'a str>,
    /// `dc:date`.
    pub date: Option<&'a str>,
}

/// Text of about `bytes` bytes that differs for each `seed`.
fn filler(bytes: usize, seed: u64) -> String {
    const WORDS: [&str; 8] = [
        "lorem", "ipsum", "dolor", "sit", "amet", "sed", "magna", "aliqua",
    ];
    let mut random = XorShift::new(seed);
    let mut text = String::with_capacity(bytes + 8);
    while text.len() < bytes {
        text.push_str(WORDS[random.below(8) as usize]);
        text.push(' ');
    }
    text
}

/// An EPUB with this metadata and a chapter of about `chapter_bytes`, whose text comes from `seed`.
pub fn epub_bytes(meta: &EpubMeta<'_>, chapter_bytes: usize, seed: u64) -> Vec<u8> {
    let mut metadata = String::new();
    let _ = writeln!(metadata, "<dc:title>{}</dc:title>", xml_escape(meta.title));
    for creator in meta.creators {
        let _ = writeln!(metadata, "<dc:creator>{}</dc:creator>", xml_escape(creator));
    }
    if let Some(identifier) = meta.identifier {
        let _ = writeln!(
            metadata,
            "<dc:identifier id=\"BookId\">{}</dc:identifier>",
            xml_escape(identifier)
        );
    }
    if let Some(language) = meta.language {
        let _ = writeln!(
            metadata,
            "<dc:language>{}</dc:language>",
            xml_escape(language)
        );
    }
    if let Some(date) = meta.date {
        let _ = writeln!(metadata, "<dc:date>{}</dc:date>", xml_escape(date));
    }
    let package = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" unique-identifier="BookId" version="3.0">
<metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
{metadata}</metadata>
<manifest><item id="c" href="c.xhtml" media-type="application/xhtml+xml"/></manifest>
<spine><itemref idref="c"/></spine>
</package>"#
    );
    let chapter = format!(
        "<html xmlns=\"http://www.w3.org/1999/xhtml\"><body><p>{}</p></body></html>",
        filler(chapter_bytes, seed)
    );

    let mut bytes = Vec::with_capacity(chapter.len() + package.len() + 1024);
    let mut zip = ZipWriter::new(&mut bytes);
    let added = zip
        .add("mimetype", b"application/epub+zip", false)
        .and_then(|()| {
            zip.add(
                "META-INF/container.xml",
                br#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
<rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"#,
                false,
            )
        })
        .and_then(|()| zip.add("OEBPS/content.opf", package.as_bytes(), false))
        .and_then(|()| zip.add("OEBPS/c.xhtml", chapter.as_bytes(), false))
        .and_then(|()| zip.finish());
    added.expect("writing to memory does not fail");
    bytes
}

/// A PDF with this `/ID[0]`, if any, and about `padding` bytes of comment whose text comes from
/// `seed`. Enough for what looks at bytes, with no page anyone could render.
pub fn pdf_bytes(id0: Option<&str>, padding: usize, seed: u64) -> Vec<u8> {
    let mut text = String::with_capacity(padding + 256);
    text.push_str("%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\n% ");
    text.push_str(&filler(padding, seed));
    text.push('\n');
    match id0 {
        Some(id) => {
            let _ = write!(
                text,
                "trailer\n<< /Size 2 /Root 1 0 R /ID [<{id}> <{id}>] >>\n"
            );
        }
        None => text.push_str("trailer\n<< /Size 2 /Root 1 0 R >>\n"),
    }
    text.push_str("%%EOF\n");
    text.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_gives_the_same_bytes() {
        let meta = EpubMeta {
            title: "T",
            ..EpubMeta::default()
        };
        assert_eq!(epub_bytes(&meta, 100, 1), epub_bytes(&meta, 100, 1));
        assert_ne!(epub_bytes(&meta, 100, 1), epub_bytes(&meta, 100, 2));
        assert_eq!(pdf_bytes(None, 100, 1), pdf_bytes(None, 100, 1));
        assert_ne!(pdf_bytes(None, 100, 1), pdf_bytes(None, 100, 2));
    }

    #[test]
    fn a_pdf_has_what_a_reader_looks_for() {
        let pdf = pdf_bytes(Some("00ab"), 10, 1);
        assert!(pdf.starts_with(b"%PDF-"));
        assert!(pdf.windows(9).any(|bytes| bytes == b"/ID [<00a"));
    }
}

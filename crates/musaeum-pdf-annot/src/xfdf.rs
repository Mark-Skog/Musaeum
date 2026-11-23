//! XFDF (ISO 19444-1), the XML Acrobat exchanges a PDF's annotations in.
//!
//! The reader is strict, because an XFDF file comes from anywhere: a document type declaration is
//! refused outright, so no entity is ever defined and nothing outside the file is ever read (XXE),
//! nor expanded a billion times; a reference other than XML's five entities and character
//! references is refused; so is a processing instruction, and anything larger, deeper or longer
//! than the limits. Elements it does not know are skipped with what is in them; annotations of
//! kinds it does not keep are counted.
//!
//! Places in XFDF are in the page's user space, as in the PDF.

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};

use quick_xml::Reader;
use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::{BytesRef, BytesStart, Event};

use crate::model::{
    Annot, FLAG_HIDDEN, FLAG_NO_ROTATE, FLAG_NO_VIEW, FLAG_NO_ZOOM, FLAG_PRINT, MAX_CONTENTS,
    MAX_NAME, MAX_QUADS, MAX_STROKE_POINTS, MAX_STROKES, Subtype,
};
use crate::read::{ours_from_quad_points, quad_points_from_ours};
use crate::text::{format_date, parse_date};
use crate::write::number;

/// The largest XFDF file read.
pub const MAX_XFDF_BYTES: usize = 64 << 20;
/// The most annotations one file may hold, and the deepest its elements may nest.
pub const MAX_XFDF_ANNOTATIONS: usize = 100_000;
const MAX_DEPTH: usize = 32;
/// The longest attribute value.
const MAX_ATTRIBUTE: usize = 4 << 20;

/// Why an XFDF file is refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XfdfError(pub String);

impl fmt::Display for XfdfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for XfdfError {}

fn refuse<T>(what: impl Into<String>) -> Result<T, XfdfError> {
    Err(XfdfError(what.into()))
}

/// What an XFDF file says: the PDF it is about, and its annotations in user space.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Xfdf {
    /// `<f href>`.
    pub href: Option<String>,
    /// `<ids original modified>`, as hex.
    pub ids: Option<(String, String)>,
    pub annots: Vec<Annot>,
    /// Annotations of other kinds, by element, and `"(broken)"` for those that could not be read.
    pub skipped: BTreeMap<String, u32>,
}

/// Text for XML: what XML cannot hold dropped, and the rest escaped.
fn escape(text: &str, attribute: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attribute => out.push_str("&quot;"),
            '\n' if attribute => out.push_str("&#10;"),
            '\r' if attribute => out.push_str("&#13;"),
            '\t' if attribute => out.push_str("&#9;"),
            '\t' | '\n' | '\r' => out.push(character),
            c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' => {}
            c => out.push(c),
        }
    }
    out
}

fn color(value: [f64; 3]) -> String {
    let byte = |channel: f64| (channel.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02X}{:02X}{:02X}",
        byte(value[0]),
        byte(value[1]),
        byte(value[2])
    )
}

fn flags_text(flags: u32) -> String {
    let names = [
        (FLAG_HIDDEN, "hidden"),
        (FLAG_PRINT, "print"),
        (FLAG_NO_ZOOM, "nozoom"),
        (FLAG_NO_ROTATE, "norotate"),
        (FLAG_NO_VIEW, "noview"),
    ];
    names
        .iter()
        .filter(|(bit, _)| flags & bit != 0)
        .map(|(_, name)| *name)
        .collect::<Vec<_>>()
        .join(",")
}

fn joined(values: impl IntoIterator<Item = f64>) -> String {
    values.into_iter().map(number).collect::<Vec<_>>().join(",")
}

/// An XFDF file of `annots`, whose places are in user space.
pub fn write_xfdf(xfdf: &Xfdf) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<xfdf xmlns=\"http://ns.adobe.com/xfdf/\" xml:space=\"preserve\">\n<annots>\n",
    );
    for annot in &xfdf.annots {
        let Some(subtype) = annot.subtype else {
            continue;
        };
        let element = subtype.xfdf();
        let _ = write!(
            out,
            "<{element} page=\"{}\" rect=\"{}\"",
            annot.page,
            joined(annot.rect)
        );
        if let Some(value) = annot.color {
            let _ = write!(out, " color=\"{}\"", color(value));
        }
        if let Some(value) = annot.interior {
            let _ = write!(out, " interior-color=\"{}\"", color(value));
        }
        let _ = write!(out, " opacity=\"{}\"", number(annot.opacity));
        if let Some(width) = annot.width {
            let _ = write!(out, " width=\"{}\"", number(width));
        }
        let flags = if annot.flags == 0 {
            FLAG_PRINT
        } else {
            annot.flags
        };
        let _ = write!(out, " flags=\"{}\"", flags_text(flags));
        for (key, value) in [
            ("name", &annot.name),
            ("title", &annot.author),
            ("subject", &annot.subject),
        ] {
            if let Some(value) = value {
                let _ = write!(out, " {key}=\"{}\"", escape(value, true));
            }
        }
        if let Some(modified) = annot.modified {
            let _ = write!(out, " date=\"{}\"", format_date(modified));
        }
        if let Some(created) = annot.created {
            let _ = write!(out, " creationdate=\"{}\"", format_date(created));
        }
        if let Some(icon) = &annot.icon {
            let _ = write!(out, " icon=\"{}\"", escape(icon, true));
        }
        if subtype.is_markup() {
            let points = annot.quads.iter().flat_map(quad_points_from_ours);
            let _ = write!(out, " coords=\"{}\"", joined(points));
        }
        out.push('>');
        if !annot.contents.is_empty() {
            let _ = write!(
                out,
                "<contents>{}</contents>",
                escape(&annot.contents, false)
            );
        }
        if subtype == Subtype::Ink {
            out.push_str("<inklist>");
            for stroke in &annot.ink {
                let points: Vec<String> = stroke
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|point| format!("{},{}", number(point[0]), number(point[1])))
                    .collect();
                let _ = write!(out, "<gesture>{}</gesture>", points.join(";"));
            }
            out.push_str("</inklist>");
        }
        let _ = writeln!(out, "</{element}>");
    }
    out.push_str("</annots>\n");
    if let Some(href) = &xfdf.href {
        let _ = writeln!(out, "<f href=\"{}\"/>", escape(href, true));
    }
    if let Some((original, modified)) = &xfdf.ids {
        let _ = writeln!(
            out,
            "<ids original=\"{}\" modified=\"{}\"/>",
            escape(original, true),
            escape(modified, true)
        );
    }
    out.push_str("</xfdf>\n");
    out
}

/// What an element's attributes say, by local name.
fn attributes(element: &BytesStart<'_>) -> Result<BTreeMap<String, String>, XfdfError> {
    let mut found = BTreeMap::new();
    for attribute in element.attributes() {
        let attribute =
            attribute.map_err(|error| XfdfError(format!("a broken attribute: {error}")))?;
        let key = attribute.key.local_name().as_ref().to_owned();
        if attribute.value.len() > MAX_ATTRIBUTE {
            return refuse(format!("the attribute {key} is too long"));
        }
        let value = attribute
            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .map_err(|error| XfdfError(format!("the attribute {key} cannot be read: {error}")))?;
        found.insert(key, value.into_owned());
    }
    Ok(found)
}

fn numbers(text: &str) -> Option<Vec<f64>> {
    text.split([',', ' ', ';', '\n', '\r', '\t'])
        .filter(|part| !part.is_empty())
        .map(|part| part.parse::<f64>().ok().filter(|value| value.is_finite()))
        .collect()
}

fn parse_color(text: &str) -> Option<[f64; 3]> {
    let hex = text.trim().strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |at: usize| {
        u8::from_str_radix(&hex[at..at + 2], 16)
            .ok()
            .map(|v| f64::from(v) / 255.0)
    };
    Some([channel(0)?, channel(2)?, channel(4)?])
}

fn parse_flags(text: &str) -> u32 {
    text.split(',')
        .map(str::trim)
        .map(|name| match name.to_ascii_lowercase().as_str() {
            "hidden" => FLAG_HIDDEN,
            "print" => FLAG_PRINT,
            "nozoom" => FLAG_NO_ZOOM,
            "norotate" => FLAG_NO_ROTATE,
            "noview" => FLAG_NO_VIEW,
            _ => 0,
        })
        .fold(0, |flags, bit| flags | bit)
}

/// The annotation an element starts, from its attributes; `None` when they do not make one.
fn annot_of(subtype: Subtype, found: &BTreeMap<String, String>) -> Option<Annot> {
    let page: u32 = found.get("page")?.trim().parse().ok()?;
    let rect = numbers(found.get("rect")?)?;
    let [a, b, c, d] = <[f64; 4]>::try_from(rect).ok()?;
    let short = |key: &str| {
        found
            .get(key)
            .map(|value| value.chars().take(MAX_NAME).collect::<String>())
    };
    let mut annot = Annot {
        page,
        subtype: Some(subtype),
        rect: [a.min(c), b.min(d), a.max(c), b.max(d)],
        color: found.get("color").and_then(|value| parse_color(value)),
        interior: found
            .get("interior-color")
            .and_then(|value| parse_color(value)),
        opacity: found
            .get("opacity")
            .and_then(|value| value.trim().parse::<f64>().ok())
            .filter(|value| value.is_finite())
            .map_or(1.0, |value| value.clamp(0.0, 1.0)),
        width: found
            .get("width")
            .and_then(|value| value.trim().parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value >= 0.0),
        name: short("name"),
        author: short("title"),
        subject: short("subject"),
        modified: found.get("date").and_then(|value| parse_date(value)),
        created: found
            .get("creationdate")
            .and_then(|value| parse_date(value)),
        icon: if subtype == Subtype::Text {
            short("icon")
        } else {
            None
        },
        flags: found.get("flags").map_or(0, |value| parse_flags(value)),
        reply: found.contains_key("inreplyto"),
        ..Annot::default()
    };
    if subtype.is_markup() {
        let points = numbers(found.get("coords")?)?;
        if points.is_empty() || points.len() % 8 != 0 || points.len() / 8 > MAX_QUADS {
            return None;
        }
        annot.quads = points
            .as_chunks::<8>()
            .0
            .iter()
            .map(|chunk| {
                let mut quad = [0.0; 8];
                quad.copy_from_slice(chunk);
                ours_from_quad_points(quad)
            })
            .collect();
    }
    Some(annot)
}

/// What is being read inside an annotation's element.
enum Inside {
    Nothing,
    Contents,
    Gesture,
}

/// Reads an XFDF file, refusing anything that is not plainly XFDF.
pub fn read_xfdf(bytes: &[u8]) -> Result<Xfdf, XfdfError> {
    if bytes.len() > MAX_XFDF_BYTES {
        return refuse("the file is too large");
    }
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let text = std::str::from_utf8(bytes).map_err(|_| XfdfError("the file is not UTF-8".into()))?;
    let mut reader = Reader::from_str(text);
    reader.config_mut().check_end_names = true;
    reader.config_mut().expand_empty_elements = true;

    let mut xfdf = Xfdf::default();
    let mut depth = 0usize;
    let mut root_seen = false;
    let mut in_annots = false;
    // The annotation being read, the depth of its element, and what part of it.
    let mut current: Option<(Annot, usize)> = None;
    let mut inside = Inside::Nothing;
    let mut buffer = String::new();
    loop {
        let event = reader
            .read_event()
            .map_err(|error| XfdfError(format!("the file is not XML: {error}")))?;
        match event {
            Event::DocType(_) => {
                return refuse(
                    "the file declares a document type; XFDF needs none, and none is read",
                );
            }
            Event::PI(_) => return refuse("the file holds a processing instruction"),
            Event::Decl(_) | Event::Comment(_) => {}
            Event::Start(element) => {
                depth += 1;
                if depth > MAX_DEPTH {
                    return refuse("the elements nest too deep");
                }
                let name = element.local_name().as_ref().to_owned();
                if depth == 1 {
                    if name != "xfdf" {
                        return refuse("the file is not XFDF");
                    }
                    root_seen = true;
                    continue;
                }
                match (&current, name.as_str()) {
                    (None, "annots") if depth == 2 => in_annots = true,
                    (None, "f") if depth == 2 => {
                        xfdf.href = attributes(&element)?.get("href").cloned();
                    }
                    (None, "ids") if depth == 2 => {
                        let found = attributes(&element)?;
                        if let (Some(original), Some(modified)) =
                            (found.get("original"), found.get("modified"))
                        {
                            xfdf.ids = Some((original.clone(), modified.clone()));
                        }
                    }
                    (None, element_name) if in_annots && depth == 3 => {
                        if xfdf.annots.len() + xfdf.skipped.values().sum::<u32>() as usize
                            >= MAX_XFDF_ANNOTATIONS
                        {
                            return refuse("too many annotations");
                        }
                        match Subtype::from_xfdf(element_name) {
                            Some(subtype) => match annot_of(subtype, &attributes(&element)?) {
                                Some(annot) => current = Some((annot, depth)),
                                None => {
                                    *xfdf.skipped.entry("(broken)".to_owned()).or_default() += 1
                                }
                            },
                            None => *xfdf.skipped.entry(element_name.to_owned()).or_default() += 1,
                        }
                    }
                    (Some(_), "contents") => {
                        inside = Inside::Contents;
                        buffer.clear();
                    }
                    (Some(_), "gesture") => {
                        inside = Inside::Gesture;
                        buffer.clear();
                    }
                    _ => {}
                }
            }
            Event::End(_) => {
                match inside {
                    Inside::Contents => {
                        if let Some((annot, _)) = &mut current {
                            annot.contents = buffer.chars().take(MAX_CONTENTS).collect();
                        }
                    }
                    Inside::Gesture => {
                        if let Some((annot, _)) = &mut current {
                            match numbers(&buffer) {
                                Some(points)
                                    if points.len() >= 2
                                        && points.len() % 2 == 0
                                        && points.len() / 2 <= MAX_STROKE_POINTS
                                        && annot.ink.len() < MAX_STROKES =>
                                {
                                    annot.ink.push(points);
                                }
                                _ => {
                                    // A stroke that cannot be read spoils the annotation.
                                    annot.subtype = None;
                                }
                            }
                        }
                    }
                    Inside::Nothing => {}
                }
                inside = Inside::Nothing;
                if let Some((_, at)) = &current
                    && *at == depth
                    && let Some((annot, _)) = current.take()
                {
                    let fine = match annot.subtype {
                        Some(Subtype::Ink) => !annot.ink.is_empty(),
                        Some(_) => true,
                        None => false,
                    };
                    if fine {
                        xfdf.annots.push(annot);
                    } else {
                        *xfdf.skipped.entry("(broken)".to_owned()).or_default() += 1;
                    }
                }
                if depth == 2 {
                    in_annots = false;
                }
                depth = depth.saturating_sub(1);
            }
            Event::Text(text) => {
                if !matches!(inside, Inside::Nothing) {
                    push_limited(&mut buffer, &text.xml10_content());
                }
            }
            Event::CData(text) => {
                if !matches!(inside, Inside::Nothing) {
                    push_limited(&mut buffer, &text.xml10_content());
                }
            }
            Event::GeneralRef(reference) => {
                let resolved = resolve(&reference)?;
                if !matches!(inside, Inside::Nothing) {
                    push_limited(&mut buffer, &resolved);
                }
            }
            Event::Empty(_) => {
                // Empty elements are expanded into a start and an end.
            }
            Event::Eof => break,
        }
    }
    if !root_seen {
        return refuse("the file is not XFDF");
    }
    Ok(xfdf)
}

fn push_limited(buffer: &mut String, text: &str) {
    if buffer.len() + text.len() <= MAX_CONTENTS * 4 {
        buffer.push_str(text);
    }
}

/// A character reference or one of XML's five entities; any other is refused, since no entity
/// can be defined in a file without a document type.
fn resolve(reference: &BytesRef<'_>) -> Result<String, XfdfError> {
    if let Ok(Some(character)) = reference.resolve_char_ref() {
        return Ok(character.to_string());
    }
    match resolve_predefined_entity(reference) {
        Some(text) => Ok(text.to_owned()),
        None => refuse(format!("the entity {} is not XML's", reference.as_ref())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Xfdf {
        Xfdf {
            href: Some("Livro & Cia.pdf".into()),
            ids: Some(("00AB".into(), "CDEF".into())),
            annots: vec![
                Annot {
                    page: 2,
                    subtype: Some(Subtype::Highlight),
                    rect: [10.0, 100.0, 50.0, 112.0],
                    quads: vec![[10.0, 112.0, 50.0, 112.0, 50.0, 100.0, 10.0, 100.0]],
                    color: Some([1.0, 0.89, 0.42]),
                    opacity: 1.0,
                    contents: "Ação & <coração>\nsegunda linha “aspas” → ✓".into(),
                    name: Some("0193a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b".into()),
                    author: Some("Mark".into()),
                    subject: Some("Realce".into()),
                    created: Some(1_795_437_296_000),
                    modified: Some(1_795_437_300_000),
                    flags: FLAG_PRINT,
                    ..Annot::default()
                },
                Annot {
                    page: 0,
                    subtype: Some(Subtype::Ink),
                    rect: [5.0, 5.0, 60.0, 60.0],
                    ink: vec![vec![10.0, 10.0, 20.5, 30.25, 50.0, 55.0], vec![7.0, 8.0]],
                    color: Some([0.6, 0.79, 0.99]),
                    opacity: 0.5,
                    width: Some(9.0),
                    flags: FLAG_PRINT,
                    ..Annot::default()
                },
                Annot {
                    page: 1,
                    subtype: Some(Subtype::Text),
                    rect: [300.0, 700.0, 320.0, 720.0],
                    color: Some([0.98, 0.68, 0.84]),
                    opacity: 1.0,
                    icon: Some("Comment".into()),
                    contents: "uma nota".into(),
                    flags: FLAG_PRINT | FLAG_NO_ZOOM | FLAG_NO_ROTATE,
                    ..Annot::default()
                },
            ],
            skipped: BTreeMap::new(),
        }
    }

    #[test]
    fn what_is_written_is_read_back() {
        let written = write_xfdf(&sample());
        let read = read_xfdf(written.as_bytes()).unwrap();
        let expected = sample();
        assert_eq!(read.href, expected.href);
        assert_eq!(read.ids, expected.ids);
        assert_eq!(read.annots.len(), 3);
        for (got, want) in read.annots.iter().zip(&expected.annots) {
            assert_eq!(got.subtype, want.subtype);
            assert_eq!(got.page, want.page);
            assert_eq!(got.rect, want.rect);
            assert_eq!(got.quads, want.quads);
            assert_eq!(got.ink, want.ink);
            assert_eq!(got.contents, want.contents);
            assert_eq!(got.name, want.name);
            assert_eq!(got.author, want.author);
            assert_eq!(got.subject, want.subject);
            assert_eq!(got.created, want.created);
            assert_eq!(got.modified, want.modified);
            assert_eq!(got.icon, want.icon);
            assert_eq!(got.flags, want.flags);
            assert_eq!(got.width, want.width);
            assert_eq!(got.opacity, want.opacity);
            for (a, b) in got.color.unwrap().iter().zip(want.color.unwrap().iter()) {
                assert!((a - b).abs() <= 0.5 / 255.0 + 1e-9, "{a} {b}");
            }
        }
    }

    #[test]
    fn a_document_type_is_refused_so_no_entity_reaches_anything() {
        let xxe = r#"<?xml version="1.0"?>
<!DOCTYPE xfdf [<!ENTITY xxe SYSTEM "file:///C:/Windows/win.ini">]>
<xfdf xmlns="http://ns.adobe.com/xfdf/"><annots>
<text page="0" rect="1,1,20,20"><contents>&xxe;</contents></text>
</annots></xfdf>"#;
        let error = read_xfdf(xxe.as_bytes()).unwrap_err();
        assert!(error.0.contains("document type"), "{error}");

        let laughs = r#"<?xml version="1.0"?>
<!DOCTYPE lolz [<!ENTITY lol "lol"><!ENTITY lol2 "&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;">]>
<xfdf><annots/></xfdf>"#;
        assert!(read_xfdf(laughs.as_bytes()).is_err());

        let parameter =
            r#"<!DOCTYPE x [<!ENTITY % remote SYSTEM "http://127.0.0.1/x.dtd"> %remote;]><xfdf/>"#;
        assert!(read_xfdf(parameter.as_bytes()).is_err());
    }

    #[test]
    fn an_entity_never_defined_is_refused_and_the_five_are_read() {
        let undefined = r#"<xfdf><annots><text page="0" rect="1,1,2,2"><contents>&xxe;</contents></text></annots></xfdf>"#;
        assert!(read_xfdf(undefined.as_bytes()).is_err());
        let in_attribute =
            r#"<xfdf><annots><text page="0" rect="1,1,2,2" title="&xxe;"/></annots></xfdf>"#;
        assert!(read_xfdf(in_attribute.as_bytes()).is_err());
        let fine = r#"<xfdf><annots><text page="0" rect="1,1,2,2" title="a &amp; b"><contents>&lt;&amp;&gt;&quot;&apos;&#233;&#x2713;</contents></text></annots></xfdf>"#;
        let read = read_xfdf(fine.as_bytes()).unwrap();
        assert_eq!(read.annots[0].contents, "<&>\"'é✓");
        assert_eq!(read.annots[0].author.as_deref(), Some("a & b"));
    }

    #[test]
    fn what_is_not_xfdf_is_refused_and_what_is_unknown_is_counted() {
        assert!(read_xfdf(b"<html/>").is_err());
        assert!(read_xfdf(b"not xml").is_err());
        assert!(read_xfdf(b"<?php echo 1 ?><xfdf/>").is_err());
        assert!(read_xfdf(&[0xFF, 0xFE, 0x3C, 0x00]).is_err());
        let deep = format!("<xfdf>{}{}</xfdf>", "<a>".repeat(40), "</a>".repeat(40));
        assert!(read_xfdf(deep.as_bytes()).is_err());
        let mixed = r#"<xfdf xmlns="http://ns.adobe.com/xfdf/"><annots>
<stamp page="0" rect="1,1,2,2"/><polygon page="0" rect="1,1,2,2"/>
<highlight page="0" rect="1,1,2,2"/>
<highlight page="0" rect="1,1,2,2" coords="1,2,3,4,5,6,7,8"><contents-richtext><body>x</body></contents-richtext></highlight>
</annots><fields/></xfdf>"#;
        let read = read_xfdf(mixed.as_bytes()).unwrap();
        assert_eq!(read.annots.len(), 1);
        assert_eq!(read.skipped.get("stamp"), Some(&1));
        assert_eq!(read.skipped.get("polygon"), Some(&1));
        assert_eq!(
            read.skipped.get("(broken)"),
            Some(&1),
            "a highlight with no coords"
        );
    }
}

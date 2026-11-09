//! One reader per format, each turning a file's bytes into a [`SourceBook`](crate::source::SourceBook).

pub mod epub;
pub mod fb2;
pub mod markdown;
pub mod mobi;
pub mod txt;
pub mod zip;

use crate::dom::{Attr, Dom, Name, NodeId};
use crate::encoding;
use crate::source::{Limits, Parsed};
use crate::{html, xml};

/// How markup should be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Markup {
    /// XHTML or SVG: XML, with the HTML parser if it is not well-formed (XHTML only).
    Xhtml,
    Svg,
    Html,
}

/// Bytes of markup to text: a byte order mark, an XML declaration, a `<meta charset>`, then UTF-8
/// if it is valid and Windows-1252 if not.
pub fn decode_markup(bytes: &[u8]) -> String {
    let declared = encoding::decode_xml(bytes);
    if !declared.replaced || bytes.starts_with(b"<?xml") {
        return declared.text;
    }
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(2048)]).to_ascii_lowercase();
    if let Some(at) = head.find("charset=") {
        let label: String = head[at + 8..]
            .trim_start_matches(['"', '\''])
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
            .collect();
        if !label.is_empty() {
            return encoding::decode_label(bytes, &label).text;
        }
    }
    encoding::decode_label(bytes, "windows-1252").text
}

/// Parses markup. Returns `None` only for what cannot be a document at all.
pub fn parse_markup(
    text: &str,
    markup: Markup,
    limits: &Limits,
    warning: &mut Option<String>,
) -> Option<(Dom, Parsed)> {
    let xml_limits = xml::Limits {
        max_nodes: limits.max_nodes,
        ..xml::Limits::default()
    };
    if markup != Markup::Html {
        match xml::parse(text, &xml_limits) {
            Ok(dom) => return Some((dom, Parsed::Xml)),
            Err(error) => {
                *warning = Some(error.to_string());
                if markup == Markup::Svg {
                    return None;
                }
            }
        }
    }
    let parsed = html::parse(text, limits.max_nodes)?;
    // The HTML parser does not limit depth, and every walk afterwards would pay for it.
    if parsed.dom.depth(Dom::DOCUMENT) > xml_limits.max_depth + 2 {
        *warning = Some(format!("nested more than {} deep", xml_limits.max_depth));
        return None;
    }
    Some((parsed.dom, Parsed::Html))
}

/// An XHTML page with a title and nothing in its body yet: what generated documents start from.
pub fn empty_page(title: &str, lang: Option<&str>) -> (Dom, NodeId) {
    let mut dom = Dom::new();
    let mut attrs = Vec::new();
    if let Some(lang) = lang {
        attrs.push(Attr {
            name: Name::plain("lang"),
            value: lang.to_owned(),
        });
    }
    let html = dom.create_element(Name::html("html"), attrs);
    dom.append(Dom::DOCUMENT, html);
    let head = dom.create_element(Name::html("head"), Vec::new());
    dom.append(html, head);
    let title_element = dom.create_element(Name::html("title"), Vec::new());
    dom.append(head, title_element);
    dom.append_text(title_element, title);
    let body = dom.create_element(Name::html("body"), Vec::new());
    dom.append(html, body);
    (dom, body)
}

/// Adds an element with attributes under `parent` and returns it.
pub fn add(dom: &mut Dom, parent: NodeId, local: &str, attrs: &[(&str, &str)]) -> NodeId {
    let element = dom.create_element(
        Name::html(local),
        attrs
            .iter()
            .map(|(name, value)| Attr {
                name: Name::plain(name),
                value: (*value).to_owned(),
            })
            .collect(),
    );
    dom.append(parent, element);
    element
}

/// What a file's first bytes say it is, as a media type and an extension.
pub fn sniff(data: &[u8]) -> Option<(&'static str, &'static str)> {
    let starts = |signature: &[u8]| data.starts_with(signature);
    if starts(b"\xFF\xD8\xFF") {
        Some(("image/jpeg", "jpg"))
    } else if starts(b"\x89PNG\r\n\x1A\n") {
        Some(("image/png", "png"))
    } else if starts(b"GIF87a") || starts(b"GIF89a") {
        Some(("image/gif", "gif"))
    } else if data.len() > 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some(("image/webp", "webp"))
    } else if data.len() > 12
        && &data[4..8] == b"ftyp"
        && (&data[8..12] == b"avif" || &data[8..12] == b"avis")
    {
        Some(("image/avif", "avif"))
    } else if starts(b"BM") && data.len() > 26 {
        Some(("image/bmp", "bmp"))
    } else if starts(b"OTTO") {
        Some(("font/otf", "otf"))
    } else if starts(b"\x00\x01\x00\x00") || starts(b"true") {
        Some(("font/ttf", "ttf"))
    } else if starts(b"wOFF") {
        Some(("font/woff", "woff"))
    } else if starts(b"wOF2") {
        Some(("font/woff2", "woff2"))
    } else {
        let head = String::from_utf8_lossy(&data[..data.len().min(1024)]);
        let head = head.trim_start_matches('\u{feff}').trim_start();
        if (head.starts_with("<?xml")
            || head.starts_with("<svg")
            || head.starts_with("<!DOCTYPE svg"))
            && head.contains("<svg")
        {
            Some(("image/svg+xml", "svg"))
        } else {
            None
        }
    }
}

/// Where a kind of file goes in the normalized book.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Style,
    Image,
    Font,
    Media,
}

/// The kind, media type and extension of a file, from its bytes first and its declared type after.
/// `None` for what has no place in a normalized book: scripts, and anything unknown.
pub fn classify(declared: &str, path: &str, data: &[u8]) -> Option<(Kind, String, String)> {
    let declared = declared.trim().to_ascii_lowercase();
    let extension = path
        .rsplit('/')
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    if declared == "text/css" || (declared.is_empty() && extension == "css") {
        return Some((Kind::Style, "text/css".to_owned(), "css".to_owned()));
    }
    if let Some((media_type, ext)) = sniff(data) {
        let kind = if media_type.starts_with("font/") {
            Kind::Font
        } else {
            Kind::Image
        };
        return Some((kind, media_type.to_owned(), ext.to_owned()));
    }
    let fontish = declared.starts_with("font/")
        || declared.contains("font-")
        || declared.contains("opentype")
        || declared.contains("truetype")
        || matches!(extension.as_str(), "ttf" | "otf" | "woff" | "woff2");
    if fontish {
        let ext = if extension.is_empty() {
            "ttf".to_owned()
        } else {
            extension
        };
        return Some((Kind::Font, declared, ext));
    }
    if declared.starts_with("audio/") || declared.starts_with("video/") || declared == "text/vtt" {
        let ext = if extension.chars().all(|c| c.is_ascii_alphanumeric()) && !extension.is_empty() {
            extension
        } else {
            "bin".to_owned()
        };
        return Some((Kind::Media, declared, ext));
    }
    None
}

/// Points `#id` links at the document that has the id, for formats that write every link as a
/// fragment of one whole book (FB2, Markdown, MOBI) and are then read as several documents.
pub fn retarget_fragment_links(documents: &mut [crate::source::SourceDocument]) {
    use std::collections::HashMap;
    let mut owner: HashMap<String, usize> = HashMap::new();
    for (index, document) in documents.iter().enumerate() {
        for node in document.dom.descendants(Dom::DOCUMENT) {
            if let Some(id) = document.dom.attr(node, "id") {
                owner.entry(id.to_owned()).or_insert(index);
            }
        }
    }
    let paths: Vec<String> = documents
        .iter()
        .map(|document| document.path.clone())
        .collect();
    for (index, document) in documents.iter_mut().enumerate() {
        let nodes: Vec<NodeId> = document.dom.descendants(Dom::DOCUMENT).collect();
        for node in nodes {
            let Some(attrs) = document.dom.attrs_mut(node) else {
                continue;
            };
            for attr in attrs.iter_mut() {
                if attr.name.local != "href" {
                    continue;
                }
                let Some(fragment) = attr.value.strip_prefix('#') else {
                    continue;
                };
                let id = crate::url::percent_decode(fragment);
                if let Some(&target) = owner.get(&id)
                    && target != index
                {
                    let file = paths[target].rsplit('/').next().unwrap_or("");
                    attr.value = format!(
                        "{}#{}",
                        crate::url::encode_path(file),
                        crate::url::encode_fragment(&id)
                    );
                }
            }
        }
    }
}

/// Standard base64, white space ignored. `None` if anything else is in the way.
pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            b' ' | b'\t' | b'\r' | b'\n' => continue,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64_decodes_with_white_space() {
        assert_eq!(super::base64_decode("TXVz\nYWV1bQ==").unwrap(), b"Musaeum");
        assert_eq!(super::base64_decode("TWE=").unwrap(), b"Ma");
        assert!(super::base64_decode("TW*E").is_none());
    }
}

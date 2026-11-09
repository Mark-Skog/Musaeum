//! Bytes to text: from what the bytes declare, and when they declare nothing, from what they look like.

use encoding_rs::{Encoding, UTF_8, UTF_16BE, UTF_16LE};

/// Text and how it was decoded.
#[derive(Debug)]
pub struct Decoded {
    pub text: String,
    /// The encoding's name, for the record.
    pub encoding: &'static str,
    /// Whether some bytes were not valid in that encoding and became U+FFFD.
    pub replaced: bool,
}

fn decode(bytes: &[u8], encoding: &'static Encoding) -> Decoded {
    // A byte order mark wins over anything else, as XML and the WHATWG both say.
    let (text, used, replaced) = encoding.decode(bytes);
    Decoded {
        text: text.into_owned(),
        encoding: used.name(),
        replaced,
    }
}

/// The encoding a byte order mark announces.
fn bom(bytes: &[u8]) -> Option<&'static Encoding> {
    Encoding::for_bom(bytes).map(|(encoding, _)| encoding)
}

/// The `encoding` of an XML declaration, read as ASCII.
fn declared_xml_encoding(bytes: &[u8]) -> Option<&'static Encoding> {
    let head = &bytes[..bytes.len().min(1024)];
    if !head.starts_with(b"<?xml") {
        return None;
    }
    let end = head.windows(2).position(|pair| pair == b"?>")?;
    let declaration = std::str::from_utf8(&head[..end]).ok()?;
    let at = declaration.find("encoding")?;
    let rest = declaration[at + "encoding".len()..].trim_start();
    let rest = rest.strip_prefix('=')?.trim_start();
    let quote = rest.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let value = &rest[1..];
    let label = &value[..value.find(quote)?];
    Encoding::for_label(label.trim().as_bytes())
}

/// Decodes an XML document: by its byte order mark, then its declaration, then as UTF-8.
pub fn decode_xml(bytes: &[u8]) -> Decoded {
    if let Some(encoding) = bom(bytes) {
        return decode(bytes, encoding);
    }
    // UTF-16 without a byte order mark still starts with "<?" or "<" and a zero byte.
    if bytes.starts_with(&[b'<', 0]) {
        return decode(bytes, UTF_16LE);
    }
    if bytes.starts_with(&[0, b'<']) {
        return decode(bytes, UTF_16BE);
    }
    match declared_xml_encoding(bytes) {
        // A UTF-16 label on bytes that are not UTF-16 is a lie; they are ASCII-compatible.
        Some(encoding) if encoding != UTF_16LE && encoding != UTF_16BE => decode(bytes, encoding),
        _ => decode(bytes, UTF_8),
    }
}

/// Decodes text in a known encoding, given by its label, or UTF-8 when the label is unknown.
pub fn decode_label(bytes: &[u8], label: &str) -> Decoded {
    decode(
        bytes,
        Encoding::for_label(label.as_bytes()).unwrap_or(UTF_8),
    )
}

/// Decodes text that says nothing about its encoding: a byte order mark, then UTF-8 if it is valid,
/// then a guess from the bytes, as a browser guesses for a plain text file.
pub fn decode_guess(bytes: &[u8]) -> Decoded {
    if let Some(encoding) = bom(bytes) {
        return decode(bytes, encoding);
    }
    if std::str::from_utf8(bytes).is_ok() {
        return decode(bytes, UTF_8);
    }
    let mut detector = chardetng::EncodingDetector::new();
    detector.feed(bytes, true);
    decode(bytes, detector.guess(None, true))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_declaration_decides_without_a_byte_order_mark() {
        let mut bytes = b"<?xml version=\"1.0\" encoding=\"windows-1251\"?><a>".to_vec();
        bytes.extend_from_slice(&[0xCF, 0xF0, 0xE8]);
        bytes.extend_from_slice(b"</a>");
        let decoded = decode_xml(&bytes);
        assert_eq!(decoded.encoding, "windows-1251");
        assert!(decoded.text.contains("При"));
    }

    #[test]
    fn a_byte_order_mark_wins() {
        let mut bytes = vec![0xFF, 0xFE];
        for unit in "<?xml version=\"1.0\" encoding=\"iso-8859-1\"?><a>é</a>".encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        let decoded = decode_xml(&bytes);
        assert_eq!(decoded.encoding, "UTF-16LE");
        assert!(decoded.text.ends_with("<a>é</a>"));
    }

    #[test]
    fn plain_text_is_guessed() {
        assert_eq!(decode_guess("ação".as_bytes()).encoding, "UTF-8");
        let latin: Vec<u8> =
            b"Cora\xe7\xe3o de le\xe3o, cora\xe7\xe3o valente, a\xe7\xe3o e emo\xe7\xe3o".to_vec();
        let decoded = decode_guess(&latin);
        assert_eq!(decoded.encoding, "windows-1252");
        assert!(decoded.text.starts_with("Coração"));
    }
}

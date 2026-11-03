//! What a file's first bytes say it is, whatever it is called.
//!
//! Comparing a few bytes with known signatures is not parsing, so this runs here and not in the
//! isolated worker. It answers one question: whether a file that took another's place at its path
//! is still the same kind of thing, which a new revision of the same document has to be.

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};

/// Where the most lenient readers still look for a PDF's header.
const PDF_HEADER_WITHIN: usize = 1024;

/// The kind of file a revision's bytes are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContentKind {
    Pdf,
    /// EPUB, CBZ and anything else in a ZIP container.
    Zip,
    DjVu,
    Rar,
    SevenZip,
    /// MOBI, AZW and AZW3, which are Palm databases.
    Mobi,
    /// FictionBook, which is plain XML.
    Xml,
    /// None of the above, such as an error page saved under a book's name.
    Unknown,
}

impl ContentKind {
    /// The kind of the bytes that begin with `head`.
    pub fn of(head: &[u8]) -> Self {
        let starts = |signature: &[u8]| head.starts_with(signature);
        if head[..head.len().min(PDF_HEADER_WITHIN)]
            .windows(5)
            .any(|bytes| bytes == b"%PDF-")
        {
            Self::Pdf
        } else if starts(b"PK\x03\x04") || starts(b"PK\x05\x06") {
            Self::Zip
        } else if starts(b"AT&TFORM") {
            Self::DjVu
        } else if starts(b"Rar!\x1A\x07") {
            Self::Rar
        } else if starts(b"7z\xBC\xAF\x27\x1C") {
            Self::SevenZip
        } else if head.get(60..68).is_some_and(|kind| kind == b"BOOKMOBI") {
            Self::Mobi
        } else if is_xml(head) {
            Self::Xml
        } else {
            Self::Unknown
        }
    }

    /// The kind a file with this format, as its name gives it, should be.
    pub fn expected_for(format: &str) -> Self {
        match format {
            "pdf" => Self::Pdf,
            "epub" | "cbz" => Self::Zip,
            "djvu" | "djv" => Self::DjVu,
            "cbr" => Self::Rar,
            "mobi" | "azw" | "azw3" => Self::Mobi,
            "fb2" => Self::Xml,
            _ => Self::Unknown,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Zip => "zip",
            Self::DjVu => "djvu",
            Self::Rar => "rar",
            Self::SevenZip => "7z",
            Self::Mobi => "mobi",
            Self::Xml => "xml",
            Self::Unknown => "unknown",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "pdf" => Self::Pdf,
            "zip" => Self::Zip,
            "djvu" => Self::DjVu,
            "rar" => Self::Rar,
            "7z" => Self::SevenZip,
            "mobi" => Self::Mobi,
            "xml" => Self::Xml,
            "unknown" => Self::Unknown,
            _ => return None,
        })
    }
}

fn is_xml(head: &[u8]) -> bool {
    let text = head.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(head);
    let start = text
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .map_or(&[][..], |at| &text[at..]);
    start.starts_with(b"<?xml") || start.starts_with(b"<FictionBook")
}

impl ToSql for ContentKind {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(self.as_str()))
    }
}

impl FromSql for ContentKind {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let text = value.as_str()?;
        Self::parse(text).ok_or_else(|| FromSqlError::Other(format!("content kind {text}").into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_are_recognized() {
        assert_eq!(ContentKind::of(b"%PDF-1.7\n"), ContentKind::Pdf);
        assert_eq!(
            ContentKind::of(b"\x00\x00junk before it %PDF-1.4"),
            ContentKind::Pdf
        );
        assert_eq!(ContentKind::of(b"PK\x03\x04mimetype"), ContentKind::Zip);
        assert_eq!(
            ContentKind::of(b"AT&TFORM\x00\x00\x00\x10DJVM"),
            ContentKind::DjVu
        );
        assert_eq!(ContentKind::of(b"Rar!\x1A\x07\x01\x00"), ContentKind::Rar);
        assert_eq!(
            ContentKind::of(b"7z\xBC\xAF\x27\x1C\x00\x04"),
            ContentKind::SevenZip
        );
        let mut mobi = vec![0; 80];
        mobi[60..68].copy_from_slice(b"BOOKMOBI");
        assert_eq!(ContentKind::of(&mobi), ContentKind::Mobi);
        assert_eq!(
            ContentKind::of(b"\xEF\xBB\xBF  <?xml version=\"1.0\"?>"),
            ContentKind::Xml
        );
        assert_eq!(ContentKind::of(b"<FictionBook xmlns=..."), ContentKind::Xml);
    }

    #[test]
    fn anything_else_is_unknown() {
        assert_eq!(
            ContentKind::of(b"<!DOCTYPE html><html>404</html>"),
            ContentKind::Unknown
        );
        assert_eq!(ContentKind::of(b""), ContentKind::Unknown);
        let mut late = vec![b' '; PDF_HEADER_WITHIN];
        late.extend_from_slice(b"%PDF-1.7");
        assert_eq!(ContentKind::of(&late), ContentKind::Unknown);
    }

    #[test]
    fn every_format_the_library_indexes_has_an_expected_kind() {
        for (format, kind) in [
            ("pdf", ContentKind::Pdf),
            ("epub", ContentKind::Zip),
            ("cbz", ContentKind::Zip),
            ("cbr", ContentKind::Rar),
            ("djvu", ContentKind::DjVu),
            ("djv", ContentKind::DjVu),
            ("mobi", ContentKind::Mobi),
            ("azw", ContentKind::Mobi),
            ("azw3", ContentKind::Mobi),
            ("fb2", ContentKind::Xml),
        ] {
            assert_eq!(ContentKind::expected_for(format), kind, "{format}");
            assert_eq!(ContentKind::parse(kind.as_str()), Some(kind));
        }
    }
}

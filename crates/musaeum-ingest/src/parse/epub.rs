//! Reads an EPUB's package document: where it is, then what it says about the book.

use quick_xml::Reader;
use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::{BytesRef, BytesStart, Event};

use super::zip::{self, Entry};
use super::{Extracted, ParseError, ReadAt};

const MAX_CONTAINER_BYTES: usize = 64 * 1024;
const MAX_PACKAGE_BYTES: usize = 2 * 1024 * 1024;
/// A very long title is real. Anything after this is not going to be read by anyone.
const MAX_FIELD_CHARS: usize = 2048;
/// More than any real book, and few enough that a hostile one cannot fill the reply with them.
const MAX_CREATORS: usize = 64;

pub(super) fn extract(source: &(impl ReadAt + ?Sized)) -> Result<Extracted, ParseError> {
    let entries = zip::read_directory(source)?;
    let container = find(&entries, b"META-INF/container.xml")
        .ok_or(ParseError::NotThisFormat("EPUB: it has no container"))?;
    let container = zip::read_entry(source, container, MAX_CONTAINER_BYTES)?;
    let package_path = package_path(&container)?;
    let package = find(&entries, package_path.as_bytes())
        .ok_or(ParseError::Malformed("the package document is not there"))?;
    parse_package(&zip::read_entry(source, package, MAX_PACKAGE_BYTES)?)
}

fn find<'a>(entries: &'a [Entry], name: &[u8]) -> Option<&'a Entry> {
    entries.iter().find(|entry| entry.name == name)
}

fn utf8(bytes: &[u8]) -> Result<&str, ParseError> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    std::str::from_utf8(bytes).map_err(|_| ParseError::Malformed("the XML is not UTF-8"))
}

fn package_path(container: &[u8]) -> Result<String, ParseError> {
    let mut reader = Reader::from_str(utf8(container)?);
    loop {
        match reader.read_event() {
            Ok(Event::Start(element) | Event::Empty(element))
                if element.local_name().as_ref() == "rootfile" =>
            {
                if let Some(path) = attribute(&element, "full-path") {
                    return Ok(path);
                }
            }
            Ok(Event::Eof) => return Err(ParseError::Malformed("the container names no package")),
            Err(_) => return Err(ParseError::Malformed("the container is not XML")),
            Ok(_) => {}
        }
    }
}

/// The value of an attribute, by its name without a namespace prefix.
fn attribute(element: &BytesStart<'_>, local_name: &str) -> Option<String> {
    element
        .attributes()
        .flatten()
        .find(|attribute| attribute.key.local_name().as_ref() == local_name)
        .and_then(|attribute| {
            attribute
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .ok()
                .map(|value| value.into_owned())
        })
}

/// One `dc:identifier`, with what its attributes say about it.
#[derive(Debug, Default)]
struct Identifier {
    value: String,
    id: Option<String>,
    scheme: Option<String>,
}

#[derive(Debug)]
enum Field {
    Title,
    Creator,
    Language,
    Date,
    Identifier {
        id: Option<String>,
        scheme: Option<String>,
    },
}

impl Field {
    fn of(element: &BytesStart<'_>) -> Option<Self> {
        match element.local_name().as_ref() {
            "title" => Some(Self::Title),
            "creator" => Some(Self::Creator),
            "language" => Some(Self::Language),
            "date" => Some(Self::Date),
            "identifier" => Some(Self::Identifier {
                id: attribute(element, "id"),
                scheme: attribute(element, "scheme"),
            }),
            _ => None,
        }
    }
}

fn parse_package(package: &[u8]) -> Result<Extracted, ParseError> {
    let mut reader = Reader::from_str(utf8(package)?);
    reader.config_mut().check_end_names = false;

    let mut unique_id = None;
    let mut in_metadata = false;
    let mut current: Option<(Field, String)> = None;
    let mut extracted = Extracted::default();
    let mut identifiers = Vec::new();

    loop {
        let event = reader
            .read_event()
            .map_err(|_| ParseError::Malformed("the package document is not XML"))?;
        match event {
            Event::Start(element) => match element.local_name().as_ref() {
                "package" => unique_id = attribute(&element, "unique-identifier"),
                "metadata" => in_metadata = true,
                _ if in_metadata && current.is_none() => {
                    current = Field::of(&element).map(|field| (field, String::new()));
                }
                _ => {}
            },
            Event::End(element) => match element.local_name().as_ref() {
                "metadata" => break,
                _ => {
                    if let Some((field, text)) = current.take() {
                        record(field, text, &mut extracted, &mut identifiers);
                    }
                }
            },
            Event::Text(text) => {
                if let Some((_, value)) = &mut current {
                    append(value, &text.xml10_content());
                }
            }
            Event::CData(text) => {
                if let Some((_, value)) = &mut current {
                    append(value, &text.xml10_content());
                }
            }
            Event::GeneralRef(reference) => {
                if let Some((_, value)) = &mut current {
                    append_reference(value, &reference);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }

    let (dc_identifier, isbn) = choose_identifiers(identifiers, unique_id.as_deref());
    extracted.dc_identifier = dc_identifier;
    extracted.isbn = isbn;
    Ok(extracted)
}

fn append(value: &mut String, text: &str) {
    if value.chars().count() < MAX_FIELD_CHARS {
        value.push_str(text);
    }
}

/// A character or one of XML's five entities. Nothing else is expanded, so an entity defined in
/// the file itself, which is how a billion laughs works, is simply left out.
fn append_reference(value: &mut String, reference: &BytesRef<'_>) {
    if let Ok(Some(character)) = reference.resolve_char_ref() {
        value.push(character);
    } else if let Some(text) = resolve_predefined_entity(reference) {
        value.push_str(text);
    }
}

/// Whitespace folded and control characters dropped, or `None` if nothing is left.
fn clean(text: &str) -> Option<String> {
    let visible: String = text
        .chars()
        .filter(|character| character.is_whitespace() || !character.is_control())
        .collect();
    let folded: String = visible
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_FIELD_CHARS)
        .collect();
    (!folded.is_empty()).then_some(folded)
}

fn record(
    field: Field,
    text: String,
    extracted: &mut Extracted,
    identifiers: &mut Vec<Identifier>,
) {
    let Some(text) = clean(&text) else {
        return;
    };
    match field {
        Field::Title => {
            extracted.title.get_or_insert(text);
        }
        Field::Creator => {
            if extracted.creators.len() < MAX_CREATORS {
                extracted.creators.push(text);
            }
        }
        Field::Language => {
            extracted.language.get_or_insert(text);
        }
        Field::Date => {
            if extracted.year.is_none() {
                extracted.year = year_of(&text);
            }
        }
        Field::Identifier { id, scheme } => identifiers.push(Identifier {
            value: text,
            id,
            scheme,
        }),
    }
}

fn year_of(date: &str) -> Option<i32> {
    let digits = date.get(..4)?;
    if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits
        .parse()
        .ok()
        .filter(|year| (1000..=2999).contains(year))
}

/// The identifier that stands for this book, and the ISBN if one of them is.
fn choose_identifiers(
    identifiers: Vec<Identifier>,
    unique_id: Option<&str>,
) -> (Option<String>, Option<String>) {
    let isbn = identifiers.iter().find_map(|identifier| {
        let marked = identifier
            .scheme
            .as_deref()
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("isbn"));
        isbn_of(&identifier.value, marked)
    });
    let usable = |identifier: &&Identifier| is_meaningful(&identifier.value);
    let named = identifiers
        .iter()
        .filter(usable)
        .find(|identifier| identifier.id.is_some() && identifier.id.as_deref() == unique_id);
    let chosen = named.or_else(|| identifiers.iter().find(usable));
    (chosen.map(|identifier| identifier.value.clone()), isbn)
}

/// Templates leave values like these in the books they make, and they say nothing about the book.
fn is_meaningful(value: &str) -> bool {
    value.chars().count() >= 4
        && !["unknown", "none", "null", "n/a", "isbn", "uuid"]
            .iter()
            .any(|placeholder| value.eq_ignore_ascii_case(placeholder))
}

/// The digits of an ISBN, if this is one. Without a mark saying so, it has to say `isbn` itself.
fn isbn_of(value: &str, marked: bool) -> Option<String> {
    let lower = value.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("urn:isbn:")
        .or_else(|| lower.strip_prefix("isbn:"))
        .or_else(|| lower.strip_prefix("isbn "));
    let digits = match (rest, marked) {
        (Some(rest), _) => rest,
        (None, true) => lower.as_str(),
        (None, false) => return None,
    };
    let digits: String = digits
        .chars()
        .filter(|character| !matches!(character, '-' | ' '))
        .map(|character| character.to_ascii_uppercase())
        .collect();
    is_valid_isbn(&digits).then_some(digits)
}

fn is_valid_isbn(digits: &str) -> bool {
    let bytes = digits.as_bytes();
    match bytes.len() {
        10 => {
            let mut sum = 0;
            for (position, &byte) in bytes.iter().enumerate() {
                let value = match byte {
                    b'0'..=b'9' => u32::from(byte - b'0'),
                    b'X' if position == 9 => 10,
                    _ => return false,
                };
                sum += value * (10 - position as u32);
            }
            sum % 11 == 0
        }
        13 => {
            let mut sum = 0;
            for (position, &byte) in bytes.iter().enumerate() {
                if !byte.is_ascii_digit() {
                    return false;
                }
                let weight = if position % 2 == 0 { 1 } else { 3 };
                sum += u32::from(byte - b'0') * weight;
            }
            sum % 10 == 0
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::super::zip::test_support::archive;
    use super::*;

    const CONTAINER: &[u8] = br#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"#;

    fn epub(opf: &str) -> Vec<u8> {
        archive(&[
            ("mimetype", b"application/epub+zip", false),
            ("META-INF/container.xml", CONTAINER, true),
            ("OEBPS/content.opf", opf.as_bytes(), true),
        ])
    }

    fn package(metadata: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" unique-identifier="BookId" version="3.0">
<metadata xmlns:dc="http://purl.org/dc/elements/1.1/">{metadata}</metadata>
<manifest/></package>"#
        )
    }

    fn read(metadata: &str) -> Extracted {
        extract(epub(&package(metadata)).as_slice()).unwrap()
    }

    #[test]
    fn the_usual_fields() {
        let found = read(
            r#"<dc:identifier id="other">urn:uuid:aaaa-bbbb</dc:identifier>
               <dc:identifier id="BookId">urn:uuid:1234-5678</dc:identifier>
               <dc:identifier opf:scheme="ISBN">978-3-16-148410-0</dc:identifier>
               <dc:title>Dom Casmurro</dc:title>
               <dc:creator>Machado de Assis</dc:creator>
               <dc:creator>Outra Pessoa</dc:creator>
               <dc:language>pt-BR</dc:language>
               <dc:date>1899-01-01</dc:date>"#,
        );
        assert_eq!(found.dc_identifier.as_deref(), Some("urn:uuid:1234-5678"));
        assert_eq!(found.isbn.as_deref(), Some("9783161484100"));
        assert_eq!(found.title.as_deref(), Some("Dom Casmurro"));
        assert_eq!(found.creators, ["Machado de Assis", "Outra Pessoa"]);
        assert_eq!(found.language.as_deref(), Some("pt-BR"));
        assert_eq!(found.year, Some(1899));
    }

    #[test]
    fn references_and_character_data_become_text() {
        let found =
            read("<dc:title>Crime &amp; Castigo &#8212; <![CDATA[<edi\u{e7}\u{e3}o>]]></dc:title>");
        assert_eq!(
            found.title.as_deref(),
            Some("Crime & Castigo \u{2014} <edi\u{e7}\u{e3}o>")
        );
    }

    #[test]
    fn whitespace_is_folded_and_control_characters_go() {
        let found = read("<dc:title>\n  Um\t\tTítulo &#7; Longo\n</dc:title>");
        assert_eq!(found.title.as_deref(), Some("Um Título Longo"));
    }

    #[test]
    fn without_a_named_identifier_the_first_good_one_stands() {
        let found =
            read(r#"<dc:identifier>unknown</dc:identifier><dc:identifier>abc123</dc:identifier>"#);
        assert_eq!(found.dc_identifier.as_deref(), Some("abc123"));
    }

    #[test]
    fn an_isbn_with_a_wrong_check_digit_is_not_an_isbn() {
        let found = read(r#"<dc:identifier>urn:isbn:9783161484101</dc:identifier>"#);
        assert_eq!(found.isbn, None);
        assert_eq!(
            found.dc_identifier.as_deref(),
            Some("urn:isbn:9783161484101")
        );
    }

    #[test]
    fn check_digits() {
        assert!(is_valid_isbn("0306406152"));
        assert!(is_valid_isbn("080442957X"));
        assert!(!is_valid_isbn("0306406153"));
        assert!(is_valid_isbn("9780306406157"));
        assert!(!is_valid_isbn("9780306406158"));
        assert!(!is_valid_isbn("978030640615"));
        assert!(!is_valid_isbn("abcdefghij"));
    }

    #[test]
    fn a_number_alone_is_not_called_an_isbn() {
        let found = read(r#"<dc:identifier>9780306406157</dc:identifier>"#);
        assert_eq!(found.isbn, None);
    }

    #[test]
    fn a_billion_laughs_expands_to_nothing() {
        let opf = r#"<?xml version="1.0"?>
<!DOCTYPE package [
  <!ENTITY a "aaaaaaaaaa">
  <!ENTITY b "&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;">
  <!ENTITY c "&b;&b;&b;&b;&b;&b;&b;&b;&b;&b;">
]>
<package><metadata><dc:title>x&c;y</dc:title></metadata></package>"#;
        let found = extract(epub(opf).as_slice()).unwrap();
        assert_eq!(found.title.as_deref(), Some("xy"));
    }

    #[test]
    fn a_huge_field_is_cut_short() {
        let long = "x".repeat(MAX_PACKAGE_BYTES / 2);
        let found = read(&format!("<dc:title>{long}</dc:title>"));
        assert_eq!(found.title.unwrap().chars().count(), MAX_FIELD_CHARS);
    }

    #[test]
    fn a_flood_of_creators_is_cut_short() {
        let flood = "<dc:creator>x</dc:creator>".repeat(10_000);
        assert_eq!(read(&flood).creators.len(), MAX_CREATORS);
    }

    #[test]
    fn deep_nesting_does_not_recurse() {
        let deep = "<a>".repeat(200_000);
        let found = extract(epub(&format!("<package><metadata>{deep}")).as_slice()).unwrap();
        assert_eq!(found, Extracted::default());
    }

    #[test]
    fn a_package_too_large_is_refused() {
        let filler = "x".repeat(MAX_PACKAGE_BYTES);
        let result = extract(epub(&package(&format!("<!-- {filler} -->"))).as_slice());
        assert!(matches!(result, Err(ParseError::TooLarge(_))));
    }

    #[test]
    fn what_is_not_an_epub_is_refused() {
        assert!(extract(b"%PDF-1.7".as_slice()).is_err());
        let not_epub = archive(&[("readme.txt", b"hello", false)]);
        assert!(matches!(
            extract(not_epub.as_slice()),
            Err(ParseError::NotThisFormat(_))
        ));
    }

    #[test]
    fn a_container_naming_something_missing_is_refused() {
        let no_package = archive(&[("META-INF/container.xml", CONTAINER, false)]);
        assert!(matches!(
            extract(no_package.as_slice()),
            Err(ParseError::Malformed(_))
        ));
    }

    #[test]
    fn broken_xml_is_an_error_and_not_a_panic() {
        for opf in [
            "<package",
            "<package><metadata><dc:title>x",
            "\u{0}\u{1}\u{2}",
        ] {
            let _ = extract(epub(opf).as_slice());
        }
        let not_utf8 = archive(&[
            ("META-INF/container.xml", CONTAINER, false),
            ("OEBPS/content.opf", b"\xFF\xFE<\x00p\x00", false),
        ]);
        assert!(extract(not_utf8.as_slice()).is_err());
    }

    #[test]
    fn every_truncation_of_a_real_file_ends_quietly() {
        let bytes = epub(&package(
            "<dc:identifier id=\"BookId\">abc123</dc:identifier><dc:title>T</dc:title>",
        ));
        for length in 0..bytes.len() {
            let _ = extract(&bytes[..length]);
        }
    }
}

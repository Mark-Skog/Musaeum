//! Text strings and dates as PDF writes them (ISO 32000-1, 7.9.2 and 7.9.4).
//!
//! A text string is PDFDocEncoding, a superset of Latin-1's letters with Portuguese's accents in
//! it, or UTF-16BE behind the byte order mark `FE FF` (and, since PDF 2.0, UTF-8 behind `EF BB BF`).
//! What is written is PDFDocEncoding whenever every character has a code in it, and UTF-16BE
//! otherwise, as Acrobat writes it. A date is `D:YYYYMMDDHHmmSS` and an offset from UTC.

/// PDFDocEncoding's characters for the codes that are not Latin-1's (Annex D, Table D.2): `0x18`
/// to `0x1F` and `0x80` to `0xA0`. `None` is a code with no character.
const SPECIAL: &[(u8, Option<char>)] = &[
    (0x18, Some('\u{02D8}')),
    (0x19, Some('\u{02C7}')),
    (0x1A, Some('\u{02C6}')),
    (0x1B, Some('\u{02D9}')),
    (0x1C, Some('\u{02DD}')),
    (0x1D, Some('\u{02DB}')),
    (0x1E, Some('\u{02DA}')),
    (0x1F, Some('\u{02DC}')),
    (0x7F, None),
    (0x80, Some('\u{2022}')),
    (0x81, Some('\u{2020}')),
    (0x82, Some('\u{2021}')),
    (0x83, Some('\u{2026}')),
    (0x84, Some('\u{2014}')),
    (0x85, Some('\u{2013}')),
    (0x86, Some('\u{0192}')),
    (0x87, Some('\u{2044}')),
    (0x88, Some('\u{2039}')),
    (0x89, Some('\u{203A}')),
    (0x8A, Some('\u{2212}')),
    (0x8B, Some('\u{2030}')),
    (0x8C, Some('\u{201E}')),
    (0x8D, Some('\u{201C}')),
    (0x8E, Some('\u{201D}')),
    (0x8F, Some('\u{2018}')),
    (0x90, Some('\u{2019}')),
    (0x91, Some('\u{201A}')),
    (0x92, Some('\u{2122}')),
    (0x93, Some('\u{FB01}')),
    (0x94, Some('\u{FB02}')),
    (0x95, Some('\u{0141}')),
    (0x96, Some('\u{0152}')),
    (0x97, Some('\u{0160}')),
    (0x98, Some('\u{0178}')),
    (0x99, Some('\u{017D}')),
    (0x9A, Some('\u{0131}')),
    (0x9B, Some('\u{0142}')),
    (0x9C, Some('\u{0153}')),
    (0x9D, Some('\u{0161}')),
    (0x9E, Some('\u{017E}')),
    (0x9F, None),
    (0xA0, Some('\u{20AC}')),
    (0xAD, None),
    // Codes below 0x18 other than tab, line feed and carriage return have no character either.
    (0x00, None),
    (0x01, None),
    (0x02, None),
    (0x03, None),
    (0x04, None),
    (0x05, None),
];

/// The character a PDFDocEncoding code stands for.
fn doc_char(code: u8) -> Option<char> {
    if let Some((_, character)) = SPECIAL.iter().find(|(special, _)| *special == code) {
        return *character;
    }
    match code {
        b'\t' | b'\n' | b'\r' => Some(code as char),
        0x00..=0x17 => None,
        // The rest is Latin-1.
        _ => Some(code as char),
    }
}

/// The PDFDocEncoding code of a character, if it has one. A soft hyphen has none: PDF 1.7 leaves
/// its code undefined, though files use it and it is read as one.
fn doc_code(character: char) -> Option<u8> {
    let code = u32::from(character);
    if code == 0xAD {
        return None;
    }
    if code < 0x100 {
        let byte = code as u8;
        if doc_char(byte) == Some(character) {
            return Some(byte);
        }
    }
    SPECIAL
        .iter()
        .find(|(_, special)| *special == Some(character))
        .map(|(code, _)| *code)
}

/// What a text string says.
pub fn decode_text(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect();
        return strip_language_marks(&String::from_utf16_lossy(&units));
    }
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    bytes
        .iter()
        .map(|&code| doc_char(code).unwrap_or('\u{FFFD}'))
        .collect()
}

/// UTF-16 text strings may hold a language code between two `U+001B`s (7.9.2.2), which is not text.
fn strip_language_marks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut inside = false;
    for character in text.chars() {
        if character == '\u{1B}' {
            inside = !inside;
        } else if !inside {
            out.push(character);
        }
    }
    out
}

/// The bytes of a text string: PDFDocEncoding when every character has a code in it, UTF-16BE
/// with its byte order mark otherwise.
pub fn encode_text(text: &str) -> Vec<u8> {
    if let Some(bytes) = text.chars().map(doc_code).collect::<Option<Vec<u8>>>() {
        return bytes;
    }
    let mut bytes = vec![0xFE, 0xFF];
    for unit in text.encode_utf16() {
        bytes.extend_from_slice(&unit.to_be_bytes());
    }
    bytes
}

/// A string object for `bytes`: literal and escaped when it reads as text, hex otherwise.
pub fn string_object(bytes: &[u8]) -> String {
    let printable = bytes
        .iter()
        .all(|&byte| (0x20..0x7F).contains(&byte) || byte == b'\n');
    if printable {
        let mut out = String::with_capacity(bytes.len() + 2);
        out.push('(');
        for &byte in bytes {
            match byte {
                b'(' | b')' | b'\\' => {
                    out.push('\\');
                    out.push(byte as char);
                }
                b'\n' => out.push_str("\\n"),
                _ => out.push(byte as char),
            }
        }
        out.push(')');
        out
    } else {
        hex_string(bytes)
    }
}

/// `<…>`.
pub fn hex_string(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2 + 2);
    out.push('<');
    for byte in bytes {
        out.push_str(&format!("{byte:02X}"));
    }
    out.push('>');
    out
}

/// A text string as a string object.
pub fn text_object(text: &str) -> String {
    string_object(&encode_text(text))
}

/// Days since 1970-01-01 of a date in the proleptic Gregorian calendar.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let of_year = (153 * month_index + 2) / 5 + day - 1;
    let of_era_days = of_era * 365 + of_era / 4 - of_era / 100 + of_year;
    era * 146_097 + of_era_days - 719_468
}

/// The date of a day since 1970-01-01.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let of_era = days - era * 146_097;
    let year_of_era = (of_era - of_era / 1460 + of_era / 36_524 - of_era / 146_096) / 365;
    let of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * of_year + 2) / 153;
    let day = of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// A moment in Unix milliseconds as a PDF date, in UTC.
pub fn format_date(milliseconds: i64) -> String {
    let seconds = milliseconds.div_euclid(1000);
    let (year, month, day) = civil_from_days(seconds.div_euclid(86_400));
    let of_day = seconds.rem_euclid(86_400);
    format!(
        "D:{year:04}{month:02}{day:02}{:02}{:02}{:02}Z",
        of_day / 3600,
        of_day / 60 % 60,
        of_day % 60
    )
}

/// A PDF date as Unix milliseconds. Fields after the year may be missing; an offset may follow as
/// `Z`, `+HH'mm'` or `-HH'mm'`, with or without the apostrophes. `None` when it is no date.
pub fn parse_date(text: &str) -> Option<i64> {
    let text = text.trim();
    let text = text.strip_prefix("D:").unwrap_or(text);
    let digits: String = text.chars().take_while(char::is_ascii_digit).collect();
    if digits.len() < 4 || digits.len() > 14 || !digits.len().is_multiple_of(2) {
        return None;
    }
    let field = |from: usize, default: i64| -> i64 {
        digits
            .get(from..from + 2)
            .and_then(|value| value.parse().ok())
            .unwrap_or(default)
    };
    let year: i64 = digits[..4].parse().ok()?;
    let (month, day) = (field(4, 1), field(6, 1));
    let (hour, minute, second) = (field(8, 0), field(10, 0), field(12, 0));
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    let rest = &text[digits.len()..];
    let offset_minutes = match rest.chars().next() {
        None | Some('Z') => 0,
        Some(sign @ ('+' | '-')) => {
            let numbers: Vec<i64> = rest[1..]
                .split(|c: char| !c.is_ascii_digit())
                .filter(|part| !part.is_empty())
                .take(2)
                .map(|part| part.parse().ok())
                .collect::<Option<_>>()?;
            let hours = *numbers.first().unwrap_or(&0);
            let minutes = *numbers.get(1).unwrap_or(&0);
            if hours > 23 || minutes > 59 {
                return None;
            }
            let total = hours * 60 + minutes;
            if sign == '-' { -total } else { total }
        }
        Some(_) => return None,
    };
    let seconds = days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second
        - offset_minutes * 60;
    Some(seconds * 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portuguese_stays_in_pdfdocencoding_and_the_rest_goes_to_utf16() {
        let accents = "Ação, coração, pão — “Ótimo!” é já… à vovó";
        let bytes = encode_text(accents);
        assert_ne!(&bytes[..2], &[0xFE, 0xFF], "every one of these has a code");
        assert_eq!(bytes.len(), accents.chars().count());
        assert_eq!(decode_text(&bytes), accents);

        for outside in ["seta → aqui", "ok ✓", "日本語", "Őrség", "😀"] {
            let bytes = encode_text(outside);
            assert_eq!(&bytes[..2], &[0xFE, 0xFF], "{outside}");
            assert_eq!(decode_text(&bytes), outside);
        }
    }

    #[test]
    fn every_pdfdocencoding_code_comes_back_as_itself() {
        for code in 0u8..=255 {
            if code == 0xAD {
                continue;
            }
            if let Some(character) = doc_char(code) {
                assert_eq!(doc_code(character), Some(code), "{code:#04x}");
            }
        }
        assert_eq!(
            decode_text(&[0x80, 0xA0, 0x8D, 0x18]),
            "\u{2022}\u{20AC}\u{201C}\u{02D8}"
        );
    }

    #[test]
    fn utf16_language_marks_are_not_text_and_utf8_is_read() {
        let mut bytes = vec![0xFE, 0xFF];
        for unit in "\u{1B}pt\u{1B}olá".encode_utf16() {
            bytes.extend_from_slice(&unit.to_be_bytes());
        }
        assert_eq!(decode_text(&bytes), "olá");
        assert_eq!(decode_text(b"\xEF\xBB\xBFol\xC3\xA1"), "olá");
    }

    #[test]
    fn strings_are_escaped_or_hex() {
        assert_eq!(string_object(b"a(b)\\c\nd"), "(a\\(b\\)\\\\c\\nd)");
        assert_eq!(string_object(&[0xFE, 0xFF, 0, 0x41]), "<FEFF0041>");
        assert_eq!(text_object("ação"), "<61E7E36F>");
    }

    #[test]
    fn dates_go_and_come_back() {
        // 2026-11-23T12:34:56Z.
        let moment = 1_795_437_296_000;
        let written = format_date(moment);
        assert_eq!(written, "D:20261123123456Z");
        assert_eq!(parse_date(&written), Some(moment));
        assert_eq!(parse_date("D:20261123093456-03'00'"), Some(moment));
        assert_eq!(parse_date("D:20261123093456-03'00"), Some(moment));
        assert_eq!(parse_date("D:20261123153456+03'00'"), Some(moment));
        assert_eq!(parse_date("20261123"), Some(1_795_392_000_000));
        assert_eq!(parse_date("D:1970"), Some(0));
        assert_eq!(format_date(0), "D:19700101000000Z");
        for bad in [
            "",
            "D:",
            "D:20261",
            "D:20261318",
            "D:2026111825",
            "D:20261118x",
            "D:20261118+99",
        ] {
            assert_eq!(parse_date(bad), None, "{bad}");
        }
    }
}

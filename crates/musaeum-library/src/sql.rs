//! How values that SQLite has no type for are stored.

use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::time::{SystemTime, UNIX_EPOCH};

use musaeum_fs::FileId;
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, Value, ValueRef};

/// Milliseconds since the Unix epoch.
pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

/// A file name or path. Windows allows names that are not valid Unicode, and those have to come
/// back exactly as they were or the file can no longer be found. Text where possible, so the
/// database stays readable, and the UTF-16 as a BLOB otherwise.
pub(crate) fn name_to_sql(name: &OsStr) -> Value {
    match name.to_str() {
        Some(text) => Value::Text(text.to_owned()),
        None => Value::Blob(name.encode_wide().flat_map(u16::to_le_bytes).collect()),
    }
}

/// The inverse of [`name_to_sql`].
pub(crate) fn name_from_sql(value: ValueRef<'_>) -> FromSqlResult<OsString> {
    match value {
        ValueRef::Text(bytes) => std::str::from_utf8(bytes)
            .map(OsString::from)
            .map_err(|error| FromSqlError::Other(Box::new(error))),
        ValueRef::Blob(bytes) => {
            let wide: Vec<u16> = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect();
            Ok(OsString::from_wide(&wide))
        }
        _ => Err(FromSqlError::InvalidType),
    }
}

/// A name read from a query, for use with [`rusqlite::Row::get`].
pub(crate) struct StoredName(pub OsString);

impl FromSql for StoredName {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        name_from_sql(value).map(Self)
    }
}

/// SQLite integers are signed, and a volume's serial number uses all 64 bits. Only equality
/// matters, so the bits are stored as they are.
pub(crate) fn volume_to_sql(volume: u64) -> i64 {
    volume as i64
}

/// The inverse of [`volume_to_sql`].
pub(crate) fn volume_from_sql(volume: i64) -> u64 {
    volume as u64
}

/// A File ID as a BLOB.
pub(crate) fn file_id_to_sql(file_id: Option<FileId>) -> Option<Vec<u8>> {
    file_id.map(|id| id.0.to_vec())
}

/// The inverse of [`file_id_to_sql`].
pub(crate) fn file_id_from_sql(value: Option<Vec<u8>>) -> Option<FileId> {
    let bytes: [u8; 16] = value?.try_into().ok()?;
    Some(FileId(bytes))
}

/// Text as a person compares it: lower case, without the accents of the Latin alphabets, and one
/// space between words. "  História  da Arte" and "historia da arte" fold alike. Other scripts are
/// lowered and kept.
pub(crate) fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for character in text.chars().flat_map(char::to_lowercase) {
        if character.is_whitespace() {
            space = !out.is_empty();
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        let plain = match character {
            'á' | 'à' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => 'a',
            'ç' | 'ć' | 'č' | 'ĉ' | 'ċ' => 'c',
            'ď' | 'đ' => 'd',
            'é' | 'è' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => 'e',
            'ğ' | 'ĝ' | 'ġ' | 'ģ' => 'g',
            'í' | 'ì' | 'î' | 'ï' | 'ī' | 'ĭ' | 'į' | 'ı' => 'i',
            'ñ' | 'ń' | 'ň' | 'ņ' => 'n',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => 'o',
            'ŕ' | 'ř' => 'r',
            'ś' | 'š' | 'ş' | 'ŝ' | 'ș' => 's',
            'ť' | 'ţ' | 'ț' => 't',
            'ú' | 'ù' | 'û' | 'ü' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => 'u',
            'ý' | 'ÿ' => 'y',
            'ź' | 'ż' | 'ž' => 'z',
            'ß' => {
                out.push('s');
                's'
            }
            other => other,
        };
        out.push(plain);
    }
    out
}

/// A date written `YYYY-MM-DD`, at midnight UTC, in milliseconds since the Unix epoch. None if it
/// is no such date.
pub(crate) fn date_ms(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let number = |range: std::ops::Range<usize>| -> Option<i64> {
        let part = &text[range];
        part.bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| part.parse().ok())
            .flatten()
    };
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let days_in_month = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if !(1..=12).contains(&month) || day < 1 || day > days_in_month[(month - 1) as usize] {
        return None;
    }
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let year_of_era = y - era * 400;
    let m = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * m + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    Some(days * 86_400_000)
}

/// The functions the library's SQL uses beyond SQLite's own: `fold(text)`, as [`fold`].
pub(crate) fn register_functions(connection: &rusqlite::Connection) -> rusqlite::Result<()> {
    use rusqlite::functions::FunctionFlags;
    connection.create_scalar_function(
        "fold",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |context| {
            Ok(match context.get_raw(0) {
                ValueRef::Text(bytes) => Some(fold(&String::from_utf8_lossy(bytes))),
                ValueRef::Null => None,
                ValueRef::Integer(number) => Some(number.to_string()),
                ValueRef::Real(number) => Some(number.to_string()),
                ValueRef::Blob(_) => None,
            })
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_folds_as_a_person_compares_it() {
        assert_eq!(fold("  História  da\tARTE "), "historia da arte");
        assert_eq!(fold("Ação, Coração e Ñandú"), "acao, coracao e nandu");
        assert_eq!(fold("Straße"), "strasse");
        assert_eq!(fold("日本語"), "日本語");
    }

    #[test]
    fn dates_are_days_since_the_epoch() {
        assert_eq!(date_ms("1970-01-01"), Some(0));
        assert_eq!(date_ms("2000-03-01"), Some(951_868_800_000));
        assert_eq!(date_ms("2024-02-29"), Some(1_709_164_800_000));
        assert_eq!(date_ms("2023-02-29"), None);
        assert_eq!(date_ms("2023-13-01"), None);
        assert_eq!(date_ms("2023-1-01"), None);
    }

    #[test]
    fn sql_can_fold_too() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        register_functions(&connection).unwrap();
        let folded: String = connection
            .query_row("SELECT fold('Física Quântica')", [], |row| row.get(0))
            .unwrap();
        assert_eq!(folded, "fisica quantica");
    }

    fn round_trip(name: &OsStr) -> OsString {
        let value = name_to_sql(name);
        let reference = match &value {
            Value::Text(text) => ValueRef::Text(text.as_bytes()),
            Value::Blob(bytes) => ValueRef::Blob(bytes),
            _ => panic!("names are text or bytes"),
        };
        name_from_sql(reference).unwrap()
    }

    #[test]
    fn text_stays_text() {
        assert!(matches!(
            name_to_sql(OsStr::new("Crime e Castigo.pdf")),
            Value::Text(_)
        ));
        let name = OsString::from("Introdução à Ótica — 日本語.pdf");
        assert_eq!(round_trip(&name), name);
    }

    #[test]
    fn a_name_that_is_not_unicode_comes_back_exactly() {
        let name = OsString::from_wide(&[0xD800, b'x' as u16, 0xDC00, b'.' as u16]);
        assert!(matches!(name_to_sql(&name), Value::Blob(_)));
        assert_eq!(round_trip(&name), name);
    }

    #[test]
    fn every_bit_of_a_volume_survives() {
        for volume in [0, 1, u64::MAX, 1 << 63] {
            assert_eq!(volume_from_sql(volume_to_sql(volume)), volume);
        }
    }

    #[test]
    fn file_ids_of_any_other_length_are_not_file_ids() {
        assert_eq!(file_id_from_sql(Some(vec![1; 8])), None);
        assert_eq!(file_id_from_sql(None), None);
        let id = FileId([7; 16]);
        assert_eq!(file_id_from_sql(file_id_to_sql(Some(id))), Some(id));
    }
}

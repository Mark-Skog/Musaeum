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

#[cfg(test)]
mod tests {
    use super::*;

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

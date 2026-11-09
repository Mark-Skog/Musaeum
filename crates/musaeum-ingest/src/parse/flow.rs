//! What MOBI, AZW3, FB2 and Markdown files say about themselves, read by `musaeum-flow` from
//! their headers alone: no text is decompressed and no picture decoded for this.

use super::epub::{isbn_of, year_of};
use super::{Extracted, ParseError, ReadAt, read_up_to};
use crate::protocol::Format;

/// Files larger than this are not read for their metadata alone.
const MAX_BYTES: u64 = 256 << 20;

pub(super) fn extract(
    format: Format,
    source: &(impl ReadAt + ?Sized),
) -> Result<Extracted, ParseError> {
    let flow_format = format
        .flow()
        .ok_or(ParseError::NotThisFormat("a flowing book"))?;
    let size = source.size()?;
    if size > MAX_BYTES {
        return Err(ParseError::TooLarge("the file, for its metadata"));
    }
    let bytes = read_up_to(source, 0, size as usize)?;
    let metadata = musaeum_flow::metadata(
        flow_format,
        &bytes,
        &musaeum_flow::source::Limits::default(),
    )
    .map_err(|_| ParseError::Malformed("its metadata cannot be read"))?;
    let isbn = metadata.identifiers.iter().find_map(|identifier| {
        let marked = identifier
            .scheme
            .as_deref()
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("isbn"));
        isbn_of(&identifier.value, marked)
    });
    Ok(Extracted {
        dc_identifier: metadata.unique_identifier.clone(),
        isbn,
        title: metadata.title().map(str::to_owned),
        creators: metadata
            .creators
            .iter()
            .map(|creator| creator.name.clone())
            .collect(),
        language: metadata.languages.first().cloned(),
        year: metadata.published.as_deref().and_then(year_of),
        ..Extracted::default()
    })
}

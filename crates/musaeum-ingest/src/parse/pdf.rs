//! Finds a PDF's `/ID` without parsing the PDF.
//!
//! PDFium is what parses PDFs, in a process of its own. This only looks for one array near the
//! ends of the file, which is where every producer writes the trailer, including the dictionary
//! of a cross-reference stream, which is not compressed. A linearized PDF also has one at the start.
//! What the catalog asks of a reader takes following the file's structure, in [`super::pdf_catalog`].

use super::{Extracted, ParseError, ReadAt, read_up_to};

const WINDOW: usize = 64 * 1024;
/// Where the header must be, in the most lenient readers' terms.
const HEADER_WITHIN: usize = 1024;
/// A real one is 16 bytes. This is for the ones that are not, without taking just anything.
const MAX_ID_BYTES: usize = 64;

pub(super) fn extract(source: &(impl ReadAt + ?Sized)) -> Result<Extracted, ParseError> {
    let size = source.size()?;
    let head = read_up_to(source, 0, WINDOW)?;
    let header_area = &head[..head.len().min(HEADER_WITHIN)];
    if !header_area.windows(5).any(|bytes| bytes == b"%PDF-") {
        return Err(ParseError::NotThisFormat("PDF"));
    }
    let tail = read_up_to(source, size.saturating_sub(WINDOW as u64), WINDOW)?;
    let id = last_id(&tail).or_else(|| last_id(&head));
    let hints = super::pdf_catalog::catalog_hints(source);
    Ok(Extracted {
        pdf_id0: id.map(|bytes| hex(&bytes)),
        page_layout: hints.page_layout,
        direction: hints.direction,
        ..Extracted::default()
    })
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
}

/// The first element of the last well-formed `/ID` array. An incremental update appends a new
/// trailer, and the last one is the one in force.
fn last_id(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut end = bytes.len();
    while let Some(position) = bytes[..end].windows(3).rposition(|name| name == b"/ID") {
        if let Some(id) = id_array(&bytes[position + 3..]) {
            return Some(id);
        }
        end = position;
    }
    None
}

fn id_array(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut cursor = Cursor { bytes, position: 0 };
    cursor.skip_whitespace();
    if cursor.next()? != b'[' {
        return None;
    }
    cursor.skip_whitespace();
    let first = cursor.string()?;
    cursor.skip_whitespace();
    if !matches!(cursor.peek()?, b'<' | b'(' | b']') {
        return None;
    }
    let meaningful = !first.is_empty()
        && first.len() <= MAX_ID_BYTES
        && first.iter().any(|&byte| byte != first[0]);
    meaningful.then_some(first)
}

struct Cursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl Cursor<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.position += 1;
        Some(byte)
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(0 | b'\t' | b'\n' | 0x0C | b'\r' | b' ')) {
            self.position += 1;
        }
    }

    fn string(&mut self) -> Option<Vec<u8>> {
        match self.next()? {
            b'<' => self.hex_string(),
            b'(' => self.literal_string(),
            _ => None,
        }
    }

    fn hex_string(&mut self) -> Option<Vec<u8>> {
        let mut result = Vec::new();
        let mut pending = None;
        loop {
            let byte = self.next()?;
            let digit = match byte {
                b'>' => break,
                b'0'..=b'9' => byte - b'0',
                b'a'..=b'f' => byte - b'a' + 10,
                b'A'..=b'F' => byte - b'A' + 10,
                0 | b'\t' | b'\n' | 0x0C | b'\r' | b' ' => continue,
                _ => return None,
            };
            match pending.take() {
                Some(high) => result.push(high << 4 | digit),
                None => pending = Some(digit),
            }
            if result.len() > MAX_ID_BYTES {
                return None;
            }
        }
        if let Some(high) = pending {
            // The specification says a missing last digit counts as zero.
            result.push(high << 4);
        }
        Some(result)
    }

    fn literal_string(&mut self) -> Option<Vec<u8>> {
        let mut result = Vec::new();
        let mut depth = 1;
        loop {
            let byte = self.next()?;
            match byte {
                b'(' => {
                    depth += 1;
                    result.push(byte);
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(result);
                    }
                    result.push(byte);
                }
                b'\\' => self.escape(&mut result)?,
                _ => result.push(byte),
            }
            if result.len() > MAX_ID_BYTES {
                return None;
            }
        }
    }

    fn escape(&mut self, result: &mut Vec<u8>) -> Option<()> {
        match self.next()? {
            b'n' => result.push(b'\n'),
            b'r' => result.push(b'\r'),
            b't' => result.push(b'\t'),
            b'b' => result.push(8),
            b'f' => result.push(0x0C),
            first @ b'0'..=b'7' => {
                let mut value = u32::from(first - b'0');
                for _ in 0..2 {
                    match self.peek() {
                        Some(digit @ b'0'..=b'7') => {
                            value = value * 8 + u32::from(digit - b'0');
                            self.position += 1;
                        }
                        _ => break,
                    }
                }
                result.push(value as u8);
            }
            b'\r' => {
                if self.peek() == Some(b'\n') {
                    self.position += 1;
                }
            }
            b'\n' => {}
            other => result.push(other),
        }
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id_of(bytes: &[u8]) -> Option<String> {
        extract(bytes).unwrap().pdf_id0
    }

    #[test]
    fn a_classic_trailer() {
        let pdf = b"%PDF-1.7\n1 0 obj\nendobj\ntrailer\n<< /Size 2 /ID [<00AB><CD12>] >>\n%%EOF";
        assert_eq!(id_of(pdf).as_deref(), Some("00ab"));
    }

    #[test]
    fn the_dictionary_of_a_cross_reference_stream() {
        let pdf = b"%PDF-1.5\n9 0 obj\n<</Type/XRef/ID[<aabb><ccdd>]/W[1 2 1]>>\nstream\nxx";
        assert_eq!(id_of(pdf).as_deref(), Some("aabb"));
    }

    #[test]
    fn the_last_trailer_is_the_one_in_force() {
        let pdf = b"%PDF-1.7\n/ID [<1111aa><2222>]\ntrailer\n/ID [<3333aa><4444>]\n%%EOF";
        assert_eq!(id_of(pdf).as_deref(), Some("3333aa"));
    }

    #[test]
    fn a_literal_string_with_escapes() {
        let pdf = b"%PDF-1.4\ntrailer << /ID [(a\\)\\101(b)) (x)] >>";
        assert_eq!(id_of(pdf), Some(hex(b"a)A(b)")));
    }

    #[test]
    fn whitespace_in_a_hexadecimal_string_and_a_missing_last_digit() {
        let pdf = b"%PDF-1.4\n/ID\n[ <A B\n1> <00> ]";
        assert_eq!(id_of(pdf).as_deref(), Some("ab10"));
    }

    #[test]
    fn other_names_that_start_the_same_are_not_this() {
        let pdf = b"%PDF-1.4\n/IDTree [<aa11><bb>] /IDS (nope)";
        assert_eq!(id_of(pdf), None);
    }

    #[test]
    fn a_placeholder_id_says_nothing() {
        let pdf = b"%PDF-1.4\n/ID [<00000000000000000000000000000000><00>]";
        assert_eq!(id_of(pdf), None);
    }

    #[test]
    fn an_absurdly_long_id_is_ignored() {
        let mut pdf = b"%PDF-1.4\n/ID [<".to_vec();
        pdf.extend(std::iter::repeat_n(b'a', 100_000));
        pdf.extend_from_slice(b"><00>]");
        assert_eq!(extract(pdf.as_slice()).unwrap().pdf_id0, None);
    }

    #[test]
    fn a_linearized_file_has_it_at_the_start() {
        let mut pdf = b"%PDF-1.4\n<</ID[<abcd><ef01>]>>\n".to_vec();
        pdf.extend(std::iter::repeat_n(b' ', 3 * WINDOW));
        assert_eq!(
            extract(pdf.as_slice()).unwrap().pdf_id0.as_deref(),
            Some("abcd")
        );
    }

    #[test]
    fn something_else_is_refused() {
        assert!(matches!(
            extract(b"PK\x03\x04 not a pdf".as_slice()),
            Err(ParseError::NotThisFormat("PDF"))
        ));
    }

    #[test]
    fn truncated_strings_end_quietly() {
        for pdf in [
            &b"%PDF-1.4\n/ID ["[..],
            b"%PDF-1.4\n/ID [<aa",
            b"%PDF-1.4\n/ID [(aa",
            b"%PDF-1.4\n/ID [(aa\\",
            b"%PDF-1.4\n/ID [<zz><yy>]",
            b"%PDF-1.4\n/ID",
        ] {
            assert_eq!(id_of(pdf), None, "{pdf:?}");
        }
    }
}

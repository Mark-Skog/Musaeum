//! PDF objects as they are written (ISO 32000-1, 7.2 and 7.3), read with where each one is.
//!
//! A dictionary keeps the byte range of every value and of its own `>>`, so a later version of an
//! object can be the very bytes of the old one with one entry changed: nothing the writer does not
//! understand is rewritten, so nothing it does not understand can be damaged. Everything has a
//! limit, and anything malformed is an error, never a guess.

use std::fmt;

/// Deeper nesting of arrays and dictionaries than this is not a real file.
pub const MAX_DEPTH: usize = 64;
/// The most items one array or dictionary may hold.
pub const MAX_ITEMS: usize = 1 << 20;

/// Why bytes are not the object they should be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntaxError {
    pub at: usize,
    pub what: &'static str,
}

impl fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at byte {}", self.what, self.at)
    }
}

impl std::error::Error for SyntaxError {}

pub type Parsed<T> = Result<T, SyntaxError>;

/// `number generation R`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Ref {
    pub number: u32,
    pub generation: u16,
}

impl fmt::Display for Ref {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {} R", self.number, self.generation)
    }
}

/// One entry of a dictionary, with where its value is in the bytes it was read from.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub key: Vec<u8>,
    pub value: Object,
    /// Where the key's `/` is.
    pub key_start: usize,
    /// Where the value's first byte is.
    pub value_start: usize,
    /// Just past the value's last byte.
    pub value_end: usize,
}

/// A dictionary, with where its `<<` and its closing `>>` are.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dictionary {
    pub entries: Vec<Entry>,
    /// Where `<<` is.
    pub open: usize,
    /// Where the closing `>>` starts.
    pub close: usize,
}

impl Dictionary {
    /// The value of `key`: the last one, when a file repeats a key, as readers do.
    pub fn get(&self, key: &[u8]) -> Option<&Object> {
        self.entry(key).map(|entry| &entry.value)
    }

    /// The entry of `key`.
    pub fn entry(&self, key: &[u8]) -> Option<&Entry> {
        self.entries.iter().rev().find(|entry| entry.key == key)
    }

    pub fn integer(&self, key: &[u8]) -> Option<i64> {
        self.get(key).and_then(Object::as_integer)
    }

    pub fn number(&self, key: &[u8]) -> Option<f64> {
        self.get(key).and_then(Object::as_number)
    }

    pub fn name(&self, key: &[u8]) -> Option<&[u8]> {
        self.get(key).and_then(Object::as_name)
    }
}

/// A PDF object.
#[derive(Clone, Debug, PartialEq)]
pub enum Object {
    Null,
    Bool(bool),
    Integer(i64),
    Real(f64),
    /// The bytes of a string, escapes undone. What they mean depends on where it is.
    String(Vec<u8>),
    Name(Vec<u8>),
    Array(Vec<Object>),
    Dictionary(Dictionary),
    Reference(Ref),
}

impl Object {
    pub fn as_integer(&self) -> Option<i64> {
        match self {
            Self::Integer(value) => Some(*value),
            _ => None,
        }
    }

    /// An integer or a real, as a finite number.
    pub fn as_number(&self) -> Option<f64> {
        match self {
            Self::Integer(value) => Some(*value as f64),
            Self::Real(value) if value.is_finite() => Some(*value),
            _ => None,
        }
    }

    pub fn as_name(&self) -> Option<&[u8]> {
        match self {
            Self::Name(name) => Some(name),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Object]> {
        match self {
            Self::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_dictionary(&self) -> Option<&Dictionary> {
        match self {
            Self::Dictionary(dictionary) => Some(dictionary),
            _ => None,
        }
    }

    pub fn as_reference(&self) -> Option<Ref> {
        match self {
            Self::Reference(reference) => Some(*reference),
            _ => None,
        }
    }

    pub fn as_string(&self) -> Option<&[u8]> {
        match self {
            Self::String(bytes) => Some(bytes),
            _ => None,
        }
    }

    /// An array of numbers, all of them.
    pub fn as_numbers(&self) -> Option<Vec<f64>> {
        self.as_array()?.iter().map(Object::as_number).collect()
    }
}

pub fn is_whitespace(byte: u8) -> bool {
    matches!(byte, 0 | b'\t' | b'\n' | 0x0C | b'\r' | b' ')
}

pub fn is_delimiter(byte: u8) -> bool {
    matches!(
        byte,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

pub fn is_regular(byte: u8) -> bool {
    !is_whitespace(byte) && !is_delimiter(byte)
}

/// Reads objects out of bytes in memory, keeping positions relative to the start of `bytes`.
pub struct Lexer<'a> {
    pub bytes: &'a [u8],
    pub position: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    pub fn at(bytes: &'a [u8], position: usize) -> Self {
        Self { bytes, position }
    }

    fn fail<T>(&self, what: &'static str) -> Parsed<T> {
        Err(SyntaxError {
            at: self.position,
            what,
        })
    }

    pub fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }

    /// Skips whitespace and comments.
    pub fn skip_space(&mut self) {
        while let Some(byte) = self.peek() {
            if is_whitespace(byte) {
                self.position += 1;
            } else if byte == b'%' {
                while self
                    .peek()
                    .is_some_and(|byte| byte != b'\n' && byte != b'\r')
                {
                    self.position += 1;
                }
            } else {
                break;
            }
        }
    }

    /// A run of regular characters: a keyword or a number.
    pub fn token(&mut self) -> &'a [u8] {
        let start = self.position;
        while self.peek().is_some_and(is_regular) {
            self.position += 1;
        }
        &self.bytes[start..self.position]
    }

    /// The keyword `expected`, after any space.
    pub fn keyword(&mut self, expected: &[u8]) -> Parsed<()> {
        self.skip_space();
        let at = self.position;
        if self.token() == expected {
            Ok(())
        } else {
            self.position = at;
            self.fail("a keyword was expected")
        }
    }

    /// An unsigned integer, after any space.
    pub fn unsigned(&mut self) -> Parsed<u64> {
        self.skip_space();
        let at = self.position;
        let token = self.token();
        if token.is_empty() || token.len() > 19 || !token.iter().all(u8::is_ascii_digit) {
            self.position = at;
            return self.fail("a number was expected");
        }
        Ok(token
            .iter()
            .fold(0, |value, digit| value * 10 + u64::from(digit - b'0')))
    }

    /// `number generation obj`.
    pub fn object_header(&mut self) -> Parsed<Ref> {
        let number = u32::try_from(self.unsigned()?).or_else(|_| self.fail("object number"))?;
        let generation =
            u16::try_from(self.unsigned()?).or_else(|_| self.fail("generation number"))?;
        self.keyword(b"obj")?;
        Ok(Ref { number, generation })
    }

    /// The next object.
    pub fn object(&mut self) -> Parsed<Object> {
        self.object_at_depth(0)
    }

    fn object_at_depth(&mut self, depth: usize) -> Parsed<Object> {
        if depth > MAX_DEPTH {
            return self.fail("objects nested too deep");
        }
        self.skip_space();
        let Some(first) = self.peek() else {
            return self.fail("the bytes end before an object");
        };
        match first {
            b'/' => {
                self.position += 1;
                Ok(Object::Name(self.name()))
            }
            b'<' if self.bytes.get(self.position + 1) == Some(&b'<') => {
                self.dictionary(depth).map(Object::Dictionary)
            }
            b'<' => self.hex_string().map(Object::String),
            b'(' => self.literal_string().map(Object::String),
            b'[' => {
                self.position += 1;
                let mut items = Vec::new();
                loop {
                    self.skip_space();
                    match self.peek() {
                        Some(b']') => {
                            self.position += 1;
                            return Ok(Object::Array(items));
                        }
                        None => return self.fail("an array does not end"),
                        _ => {}
                    }
                    if items.len() >= MAX_ITEMS {
                        return self.fail("an array too long");
                    }
                    items.push(self.object_at_depth(depth + 1)?);
                }
            }
            _ => self.number_or_keyword(),
        }
    }

    /// The name after a `/`, with `#xx` escapes undone.
    fn name(&mut self) -> Vec<u8> {
        let hex = |byte: Option<&u8>| byte.and_then(|&byte| (byte as char).to_digit(16));
        let mut name = Vec::new();
        while let Some(byte) = self.peek().filter(|&byte| is_regular(byte)) {
            self.position += 1;
            if byte == b'#'
                && let (Some(high), Some(low)) = (
                    hex(self.bytes.get(self.position)),
                    hex(self.bytes.get(self.position + 1)),
                )
            {
                name.push((high * 16 + low) as u8);
                self.position += 2;
            } else {
                name.push(byte);
            }
        }
        name
    }

    /// A dictionary, from its `<<`.
    pub fn dictionary(&mut self, depth: usize) -> Parsed<Dictionary> {
        let open = self.position;
        if !self.bytes[self.position..].starts_with(b"<<") {
            return self.fail("a dictionary was expected");
        }
        self.position += 2;
        let mut entries = Vec::new();
        loop {
            self.skip_space();
            match self.peek() {
                Some(b'>') if self.bytes.get(self.position + 1) == Some(&b'>') => {
                    let close = self.position;
                    self.position += 2;
                    return Ok(Dictionary {
                        entries,
                        open,
                        close,
                    });
                }
                Some(b'/') => {
                    if entries.len() >= MAX_ITEMS {
                        return self.fail("a dictionary too long");
                    }
                    let key_start = self.position;
                    self.position += 1;
                    let key = self.name();
                    self.skip_space();
                    let value_start = self.position;
                    let value = self.object_at_depth(depth + 1)?;
                    entries.push(Entry {
                        key,
                        value,
                        key_start,
                        value_start,
                        value_end: self.position,
                    });
                }
                None => return self.fail("a dictionary does not end"),
                _ => return self.fail("a dictionary key was expected"),
            }
        }
    }

    /// `<hex digits>`: whitespace between digits is ignored, and an odd last digit is followed by 0.
    fn hex_string(&mut self) -> Parsed<Vec<u8>> {
        self.position += 1;
        let mut bytes = Vec::new();
        let mut high: Option<u8> = None;
        loop {
            let Some(byte) = self.peek() else {
                return self.fail("a hex string does not end");
            };
            self.position += 1;
            if byte == b'>' {
                if let Some(high) = high {
                    bytes.push(high << 4);
                }
                return Ok(bytes);
            }
            if is_whitespace(byte) {
                continue;
            }
            let Some(digit) = (byte as char).to_digit(16) else {
                return self.fail("a hex string holds something else");
            };
            match high.take() {
                Some(high) => bytes.push(high << 4 | digit as u8),
                None => high = Some(digit as u8),
            }
        }
    }

    /// `(…)`, with balanced parentheses, escapes and line ends as 7.3.4.2 says.
    fn literal_string(&mut self) -> Parsed<Vec<u8>> {
        self.position += 1;
        let mut bytes = Vec::new();
        let mut depth = 1usize;
        loop {
            let Some(byte) = self.peek() else {
                return self.fail("a string does not end");
            };
            self.position += 1;
            match byte {
                b'(' => {
                    depth += 1;
                    bytes.push(byte);
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(bytes);
                    }
                    bytes.push(byte);
                }
                // An end of line in a string is a line feed, however it is written.
                b'\r' => {
                    if self.peek() == Some(b'\n') {
                        self.position += 1;
                    }
                    bytes.push(b'\n');
                }
                b'\\' => {
                    let Some(escaped) = self.peek() else {
                        return self.fail("a string does not end");
                    };
                    self.position += 1;
                    match escaped {
                        b'n' => bytes.push(b'\n'),
                        b'r' => bytes.push(b'\r'),
                        b't' => bytes.push(b'\t'),
                        b'b' => bytes.push(0x08),
                        b'f' => bytes.push(0x0C),
                        b'0'..=b'7' => {
                            let mut value = u32::from(escaped - b'0');
                            for _ in 0..2 {
                                match self.peek() {
                                    Some(digit @ b'0'..=b'7') => {
                                        value = value * 8 + u32::from(digit - b'0');
                                        self.position += 1;
                                    }
                                    _ => break,
                                }
                            }
                            bytes.push((value & 0xFF) as u8);
                        }
                        // A backslash at the end of a line continues the string on the next.
                        b'\r' => {
                            if self.peek() == Some(b'\n') {
                                self.position += 1;
                            }
                        }
                        b'\n' => {}
                        // `\(`, `\)`, `\\`, and an unknown escape, which stands for its character.
                        other => bytes.push(other),
                    }
                }
                other => bytes.push(other),
            }
        }
    }

    fn number_or_keyword(&mut self) -> Parsed<Object> {
        let at = self.position;
        let token = self.token();
        match token {
            b"true" => return Ok(Object::Bool(true)),
            b"false" => return Ok(Object::Bool(false)),
            b"null" => return Ok(Object::Null),
            b"" => {
                self.position = at;
                return self.fail("an object was expected");
            }
            _ => {}
        }
        let Some(text) = std::str::from_utf8(token).ok() else {
            self.position = at;
            return self.fail("an object was expected");
        };
        if let Ok(value) = text.parse::<i64>() {
            // `number generation R` is a reference.
            let after_number = self.position;
            if let Ok(number) = u32::try_from(value)
                && let Ok(generation) = self.unsigned()
                && let Ok(generation) = u16::try_from(generation)
            {
                self.skip_space();
                let after_r = self.bytes.get(self.position + 1).copied();
                if self.peek() == Some(b'R') && after_r.is_none_or(|byte| !is_regular(byte)) {
                    self.position += 1;
                    return Ok(Object::Reference(Ref { number, generation }));
                }
            }
            self.position = after_number;
            return Ok(Object::Integer(value));
        }
        // Reals have no exponent in PDF; a lone sign or point, or two of either, is no number.
        let digits = text.bytes().filter(u8::is_ascii_digit).count();
        let fine = digits > 0
            && text
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'.' | b'+' | b'-'))
            && text.bytes().filter(|&byte| byte == b'.').count() <= 1
            && text[1..].bytes().all(|byte| byte != b'+' && byte != b'-');
        match text.parse::<f64>() {
            Ok(value) if fine && value.is_finite() => Ok(Object::Real(value)),
            _ => {
                self.position = at;
                self.fail("an object was expected")
            }
        }
    }
}

/// Parses one complete object from `bytes`.
pub fn parse(bytes: &[u8]) -> Parsed<Object> {
    Lexer::new(bytes).object()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_of_object_is_read() {
        let source = b"<< /Type /Annot /Rect [10 20.5 -3 .5] /Open false /Ref 12 0 R \
                       /Num 7 /Str (a\\(b\\) \\101\\n) /Hex <48 65 6c6C 6> /Null null /N#20x 1 >>";
        let Object::Dictionary(dictionary) = parse(source).unwrap() else {
            panic!("not a dictionary");
        };
        assert_eq!(dictionary.name(b"Type"), Some(&b"Annot"[..]));
        assert_eq!(
            dictionary.get(b"Rect").unwrap().as_numbers(),
            Some(vec![10.0, 20.5, -3.0, 0.5])
        );
        assert_eq!(dictionary.get(b"Open"), Some(&Object::Bool(false)));
        assert_eq!(
            dictionary.get(b"Ref"),
            Some(&Object::Reference(Ref {
                number: 12,
                generation: 0
            }))
        );
        assert_eq!(dictionary.integer(b"Num"), Some(7));
        assert_eq!(
            dictionary.get(b"Str").unwrap().as_string(),
            Some(&b"a(b) A\n"[..])
        );
        assert_eq!(
            dictionary.get(b"Hex").unwrap().as_string(),
            Some(&b"Hell`"[..])
        );
        assert_eq!(dictionary.get(b"Null"), Some(&Object::Null));
        assert_eq!(dictionary.integer(b"N x"), Some(1));
        assert_eq!(&source[dictionary.close..dictionary.close + 2], b">>");
    }

    #[test]
    fn spans_point_at_the_bytes_of_each_value() {
        let source = b"<</Annots [1 0 R 2 0 R]/Rotate 90>>";
        let Object::Dictionary(dictionary) = parse(source).unwrap() else {
            panic!("not a dictionary");
        };
        let annots = dictionary.entry(b"Annots").unwrap();
        assert_eq!(
            &source[annots.key_start..annots.value_end],
            b"/Annots [1 0 R 2 0 R]"
        );
        assert_eq!(
            &source[annots.value_start..annots.value_end],
            b"[1 0 R 2 0 R]"
        );
        let rotate = dictionary.entry(b"Rotate").unwrap();
        assert_eq!(&source[rotate.key_start..rotate.value_end], b"/Rotate 90");
        assert_eq!(dictionary.close, source.len() - 2);
    }

    #[test]
    fn line_ends_in_strings_are_line_feeds() {
        assert_eq!(
            parse(b"(one\r\ntwo\rthree\\\r\nfour)").unwrap(),
            Object::String(b"one\ntwo\nthreefour".to_vec())
        );
    }

    #[test]
    fn what_is_broken_is_an_error() {
        for broken in [
            &b"<< /A 1"[..],
            b"[1 2",
            b"(open",
            b"<12 zz>",
            b"<< 1 2 >>",
            b"--1",
            b"1.2.3",
            b"",
            b")",
        ] {
            assert!(
                parse(broken).is_err(),
                "{:?}",
                String::from_utf8_lossy(broken)
            );
        }
        let deep = "[".repeat(MAX_DEPTH + 2);
        assert!(parse(deep.as_bytes()).is_err());
    }

    #[test]
    fn a_number_followed_by_another_is_not_a_reference_without_r() {
        assert_eq!(
            parse(b"[1 0 2 0 R]").unwrap(),
            Object::Array(vec![
                Object::Integer(1),
                Object::Integer(0),
                Object::Reference(Ref {
                    number: 2,
                    generation: 0
                }),
            ])
        );
        assert!(parse(b"[3 0 RG]").is_err(), "RG is an operator, not R");
    }
}

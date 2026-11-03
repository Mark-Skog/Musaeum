//! Reads what a PDF's catalog asks of a reader: `/PageLayout`, and `/Direction` in
//! `/ViewerPreferences`.
//!
//! PDFium has no way to ask for the first, so this finds the catalog itself, the way a PDF reader
//! does: from `startxref` to the cross-reference section that lists it, classic or a stream, and
//! from there to the object, which may be inside a compressed object stream. Newer sections come
//! first, so the catalog of the last incremental update is the one read. It reads only what that
//! path needs, every step has a limit, and anything it does not understand means no answer rather
//! than a guess, since a wrong layout is worse than the default one.

use std::collections::HashSet;
use std::io::Read;

use flate2::read::ZlibDecoder;
use serde::{Deserialize, Serialize};

use super::{ReadAt, read_up_to};

/// Where `startxref` must be, counting back from the end.
const STARTXREF_WITHIN: usize = 4096;
/// Most cross-reference sections followed through `/Prev`.
const MAX_SECTIONS: usize = 64;
/// Most bytes read to parse one object's dictionary.
const MAX_OBJECT_BYTES: usize = 1 << 20;
/// Most bytes a stream may take in the file, and once decoded.
const MAX_STREAM_BYTES: usize = 64 << 20;
/// Most nesting of arrays and dictionaries.
const MAX_DEPTH: usize = 32;
/// Most objects looked up for one answer.
const MAX_LOOKUPS: usize = 8;
/// Most entries of a classic cross-reference subsection, which is 20 bytes for each.
const MAX_TABLE_ENTRIES: u64 = 8 << 20;

/// How the pages should be shown, from `/PageLayout`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PageLayout {
    /// One page at a time.
    SinglePage,
    /// One column, scrolled.
    OneColumn,
    /// Two columns, odd-numbered pages on the left.
    TwoColumnLeft,
    /// Two columns, odd-numbered pages on the right.
    TwoColumnRight,
    /// Two pages at a time, odd-numbered pages on the left.
    TwoPageLeft,
    /// Two pages at a time, odd-numbered pages on the right.
    TwoPageRight,
}

impl PageLayout {
    /// The name the file gives it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SinglePage => "SinglePage",
            Self::OneColumn => "OneColumn",
            Self::TwoColumnLeft => "TwoColumnLeft",
            Self::TwoColumnRight => "TwoColumnRight",
            Self::TwoPageLeft => "TwoPageLeft",
            Self::TwoPageRight => "TwoPageRight",
        }
    }

    /// The layout with this name, if it is one.
    pub fn from_name(name: &[u8]) -> Option<Self> {
        Some(match name {
            b"SinglePage" => Self::SinglePage,
            b"OneColumn" => Self::OneColumn,
            b"TwoColumnLeft" => Self::TwoColumnLeft,
            b"TwoColumnRight" => Self::TwoColumnRight,
            b"TwoPageLeft" => Self::TwoPageLeft,
            b"TwoPageRight" => Self::TwoPageRight,
            _ => return None,
        })
    }

    const ALL: [Self; 6] = [
        Self::SinglePage,
        Self::OneColumn,
        Self::TwoColumnLeft,
        Self::TwoColumnRight,
        Self::TwoPageLeft,
        Self::TwoPageRight,
    ];

    /// How the render workers' protocol carries it: 1 to 6, in the order of the specification.
    pub fn code(self) -> u8 {
        Self::ALL
            .iter()
            .position(|layout| *layout == self)
            .map_or(0, |at| at as u8 + 1)
    }

    /// The layout a code stands for, with 0 meaning none.
    pub fn from_code(code: u8) -> Option<Self> {
        Self::ALL.get(usize::from(code).checked_sub(1)?).copied()
    }

    /// Whether it shows facing pages.
    pub fn is_two_up(self) -> bool {
        !matches!(self, Self::SinglePage | Self::OneColumn)
    }

    /// Whether odd-numbered pages go on the right, so the first page stands alone, as in a book.
    pub fn first_page_alone(self) -> bool {
        matches!(self, Self::TwoColumnRight | Self::TwoPageRight)
    }
}

/// The order of the content, from `/ViewerPreferences /Direction`. It decides which side facing
/// pages go on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    /// Left to right.
    L2R,
    /// Right to left, including vertical writing such as Japanese.
    R2L,
}

impl Direction {
    /// The name the file gives it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::L2R => "L2R",
            Self::R2L => "R2L",
        }
    }

    /// The direction with this name, if it is one.
    pub fn from_name(name: &[u8]) -> Option<Self> {
        match name {
            b"L2R" => Some(Self::L2R),
            b"R2L" => Some(Self::R2L),
            _ => None,
        }
    }

    /// How the render workers' protocol carries it: 1 for `L2R`, 2 for `R2L`.
    pub fn code(self) -> u8 {
        match self {
            Self::L2R => 1,
            Self::R2L => 2,
        }
    }

    /// The direction a code stands for, with 0 meaning none.
    pub fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::L2R),
            2 => Some(Self::R2L),
            _ => None,
        }
    }
}

/// What the catalog asks for. Either may be missing, and both are when the catalog cannot be found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CatalogHints {
    /// `/PageLayout`.
    pub page_layout: Option<PageLayout>,
    /// `/ViewerPreferences /Direction`.
    pub direction: Option<Direction>,
}

/// Finds the catalog's hints. Never fails: what cannot be read is simply not there.
pub fn catalog_hints(source: &(impl ReadAt + ?Sized)) -> CatalogHints {
    Reader::new(source).hints().unwrap_or_default()
}

/// Why a step did not work. None of these reach the caller.
#[derive(Debug)]
struct Broken;

type Parsed<T> = Result<T, Broken>;

#[derive(Clone, Debug, PartialEq)]
enum Object {
    Null,
    Bool,
    Number,
    Integer(i64),
    Name(Vec<u8>),
    String,
    Array(Vec<Object>),
    Dictionary(Dictionary),
    Reference(u32),
}

type Dictionary = Vec<(Vec<u8>, Object)>;

fn get<'a>(dictionary: &'a Dictionary, key: &[u8]) -> Option<&'a Object> {
    // The last of repeated keys is the one readers use.
    dictionary
        .iter()
        .rev()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value)
}

fn integer(dictionary: &Dictionary, key: &[u8]) -> Option<i64> {
    match get(dictionary, key)? {
        Object::Integer(value) => Some(*value),
        _ => None,
    }
}

fn is_whitespace(byte: u8) -> bool {
    matches!(byte, 0 | b'\t' | b'\n' | 0x0C | b'\r' | b' ')
}

fn is_delimiter(byte: u8) -> bool {
    matches!(
        byte,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

fn is_regular(byte: u8) -> bool {
    !is_whitespace(byte) && !is_delimiter(byte)
}

/// Reads PDF objects out of bytes already in memory.
struct Lexer<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Lexer<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }

    fn skip_space(&mut self) {
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

    /// A run of regular characters, such as a keyword or a number.
    fn token(&mut self) -> &'a [u8] {
        let start = self.position;
        while self.peek().is_some_and(is_regular) {
            self.position += 1;
        }
        &self.bytes[start..self.position]
    }

    fn keyword(&mut self, expected: &[u8]) -> Parsed<()> {
        self.skip_space();
        if self.token() == expected {
            Ok(())
        } else {
            Err(Broken)
        }
    }

    fn unsigned(&mut self) -> Parsed<u64> {
        self.skip_space();
        let token = self.token();
        if token.is_empty() || token.len() > 19 || !token.iter().all(u8::is_ascii_digit) {
            return Err(Broken);
        }
        Ok(token
            .iter()
            .fold(0, |value, digit| value * 10 + u64::from(digit - b'0')))
    }

    /// `number generation obj`, returning the object number.
    fn object_header(&mut self) -> Parsed<u32> {
        let number = u32::try_from(self.unsigned()?).map_err(|_| Broken)?;
        self.unsigned()?;
        self.keyword(b"obj")?;
        Ok(number)
    }

    fn object(&mut self, depth: usize) -> Parsed<Object> {
        if depth > MAX_DEPTH {
            return Err(Broken);
        }
        self.skip_space();
        match self.peek().ok_or(Broken)? {
            b'/' => {
                self.position += 1;
                Ok(Object::Name(self.name()))
            }
            b'<' if self.bytes.get(self.position + 1) == Some(&b'<') => {
                self.position += 2;
                self.dictionary_body(depth).map(Object::Dictionary)
            }
            b'<' => {
                self.position += 1;
                while self.peek().ok_or(Broken)? != b'>' {
                    self.position += 1;
                }
                self.position += 1;
                Ok(Object::String)
            }
            b'(' => {
                self.position += 1;
                self.skip_literal_string()?;
                Ok(Object::String)
            }
            b'[' => {
                self.position += 1;
                let mut items = Vec::new();
                loop {
                    self.skip_space();
                    if self.peek().ok_or(Broken)? == b']' {
                        self.position += 1;
                        return Ok(Object::Array(items));
                    }
                    items.push(self.object(depth + 1)?);
                }
            }
            _ => self.number_or_keyword(),
        }
    }

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

    fn skip_literal_string(&mut self) -> Parsed<()> {
        let mut depth = 1usize;
        while depth > 0 {
            match self.peek().ok_or(Broken)? {
                b'\\' => self.position += 1,
                b'(' => depth += 1,
                b')' => depth -= 1,
                _ => {}
            }
            self.position += 1;
        }
        Ok(())
    }

    fn dictionary_body(&mut self, depth: usize) -> Parsed<Dictionary> {
        let mut entries = Vec::new();
        loop {
            self.skip_space();
            match self.peek().ok_or(Broken)? {
                b'>' if self.bytes.get(self.position + 1) == Some(&b'>') => {
                    self.position += 2;
                    return Ok(entries);
                }
                b'/' => {
                    self.position += 1;
                    let key = self.name();
                    let value = self.object(depth + 1)?;
                    entries.push((key, value));
                }
                _ => return Err(Broken),
            }
        }
    }

    fn number_or_keyword(&mut self) -> Parsed<Object> {
        let token = self.token();
        match token {
            b"true" | b"false" => return Ok(Object::Bool),
            b"null" => return Ok(Object::Null),
            _ => {}
        }
        let text = std::str::from_utf8(token).map_err(|_| Broken)?;
        let Ok(value) = text.parse::<i64>() else {
            return match text.parse::<f64>() {
                Ok(_) => Ok(Object::Number),
                Err(_) => Err(Broken),
            };
        };
        // `number generation R` is a reference.
        let after_number = self.position;
        if let Ok(number) = u32::try_from(value)
            && self.unsigned().is_ok()
        {
            self.skip_space();
            let after_r = self.bytes.get(self.position + 1).copied();
            if self.peek() == Some(b'R') && after_r.is_none_or(|byte| !is_regular(byte)) {
                self.position += 1;
                return Ok(Object::Reference(number));
            }
        }
        self.position = after_number;
        Ok(Object::Integer(value))
    }
}

/// Where a cross-reference section says an object is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Location {
    Offset(u64),
    InStream { stream: u32, index: u32 },
}

/// What one cross-reference section says.
struct Section {
    trailer: Dictionary,
    /// `None` if it does not list the object asked about, `Some(None)` if it lists it as free.
    wanted: Option<Option<Location>>,
}

/// An object as read from the file, with its stream's decoded bytes if it has one.
struct Found {
    number: u32,
    value: Object,
    stream: Option<Vec<u8>>,
}

struct Reader<'a, S: ReadAt + ?Sized> {
    source: &'a S,
    size: u64,
    lookups: usize,
}

impl<'a, S: ReadAt + ?Sized> Reader<'a, S> {
    fn new(source: &'a S) -> Self {
        Self {
            size: source.size().unwrap_or(0),
            source,
            lookups: 0,
        }
    }

    fn hints(&mut self) -> Parsed<CatalogHints> {
        let start = self.startxref()?;
        let root = self.root(start)?;
        let catalog = self.dictionary_of(start, root)?;
        let page_layout = match get(&catalog, b"PageLayout") {
            Some(Object::Name(name)) => PageLayout::from_name(name),
            _ => None,
        };
        let preferences = match get(&catalog, b"ViewerPreferences") {
            Some(Object::Dictionary(dictionary)) => Some(dictionary.clone()),
            Some(Object::Reference(number)) => self.dictionary_of(start, *number).ok(),
            _ => None,
        };
        let direction =
            preferences
                .as_ref()
                .and_then(|preferences| match get(preferences, b"Direction") {
                    Some(Object::Name(name)) => Direction::from_name(name),
                    _ => None,
                });
        Ok(CatalogHints {
            page_layout,
            direction,
        })
    }

    fn read(&self, offset: u64, length: usize) -> Parsed<Vec<u8>> {
        if offset >= self.size {
            return Err(Broken);
        }
        read_up_to(self.source, offset, length).map_err(|_| Broken)
    }

    /// The offset `startxref` names, which is where the newest cross-reference section starts.
    fn startxref(&self) -> Parsed<u64> {
        let from = self.size.saturating_sub(STARTXREF_WITHIN as u64);
        let tail = self.read(from, STARTXREF_WITHIN)?;
        let at = tail
            .windows(9)
            .rposition(|bytes| bytes == b"startxref")
            .ok_or(Broken)?;
        Lexer::new(&tail[at + 9..]).unsigned()
    }

    /// The catalog's object number, from the newest trailer that names one.
    fn root(&mut self, start: u64) -> Parsed<u32> {
        let mut root = None;
        self.walk(start, None, |section| {
            if let Some(Object::Reference(number)) = get(&section.trailer, b"Root") {
                root = Some(*number);
            }
            root.is_some()
        })?;
        root.ok_or(Broken)
    }

    /// Where the newest section that lists object `number` says it is.
    fn locate(&mut self, start: u64, number: u32) -> Parsed<Location> {
        let mut answer = None;
        self.walk(start, Some(number), |section| {
            answer = section.wanted;
            answer.is_some()
        })?;
        answer.flatten().ok_or(Broken)
    }

    /// Shows `visit` each section, newest first, until it returns true.
    fn walk(
        &mut self,
        start: u64,
        wanted: Option<u32>,
        mut visit: impl FnMut(&Section) -> bool,
    ) -> Parsed<()> {
        let mut next = Some(start);
        let mut seen = HashSet::new();
        while let Some(offset) = next {
            if seen.len() >= MAX_SECTIONS || !seen.insert(offset) {
                return Ok(());
            }
            let section = self.section(offset, wanted)?;
            if visit(&section) {
                return Ok(());
            }
            // A hybrid file lists its compressed objects in a stream that the table points to,
            // which comes before the older sections.
            if let Some(stream) = integer(&section.trailer, b"XRefStm")
                && let Ok(stream) = u64::try_from(stream)
                && seen.insert(stream)
                && let Ok(hidden) = self.section(stream, wanted)
                && hidden.wanted.is_some()
                && visit(&Section {
                    trailer: Dictionary::new(),
                    wanted: hidden.wanted,
                })
            {
                return Ok(());
            }
            next = integer(&section.trailer, b"Prev")
                .and_then(|previous| u64::try_from(previous).ok());
        }
        Ok(())
    }

    fn section(&mut self, offset: u64, wanted: Option<u32>) -> Parsed<Section> {
        let head = self.read(offset, 16)?;
        let mut lexer = Lexer::new(&head);
        lexer.skip_space();
        if lexer.token() == b"xref" {
            return self.table(offset, wanted);
        }
        let found = self.object_at(offset)?;
        let Object::Dictionary(dictionary) = found.value else {
            return Err(Broken);
        };
        let data = found.stream.ok_or(Broken)?;
        let wanted = match wanted {
            Some(number) => stream_entry(&dictionary, &data, number)?,
            None => None,
        };
        Ok(Section {
            trailer: dictionary,
            wanted,
        })
    }

    /// A classic table: subsections of fixed 20-byte entries, then `trailer` and a dictionary.
    fn table(&mut self, offset: u64, wanted: Option<u32>) -> Parsed<Section> {
        let mut found = None;
        let header = self.read(offset, 16)?;
        let mut lexer = Lexer::new(&header);
        lexer.keyword(b"xref")?;
        let mut position = offset + lexer.position as u64;
        loop {
            let line = self.read(position, 64)?;
            let mut lexer = Lexer::new(&line);
            lexer.skip_space();
            let at = lexer.position;
            if lexer.token() == b"trailer" {
                position += lexer.position as u64;
                break;
            }
            lexer.position = at;
            let first = lexer.unsigned()?;
            let count = lexer.unsigned()?;
            if count > MAX_TABLE_ENTRIES {
                return Err(Broken);
            }
            // The entries start on the next line.
            while lexer
                .peek()
                .is_some_and(|byte| byte == b' ' || byte == b'\t')
            {
                lexer.position += 1;
            }
            match lexer.peek() {
                Some(b'\r') if line.get(lexer.position + 1) == Some(&b'\n') => lexer.position += 2,
                Some(b'\r' | b'\n') => lexer.position += 1,
                _ => return Err(Broken),
            }
            let entries = position + lexer.position as u64;
            if let Some(number) = wanted.map(u64::from)
                && (first..first + count).contains(&number)
            {
                found = Some(self.table_entry(entries + (number - first) * 20)?);
            }
            position = entries + count * 20;
        }
        let trailer = self.read(position, 64 * 1024)?;
        let Object::Dictionary(trailer) = Lexer::new(&trailer).object(0)? else {
            return Err(Broken);
        };
        Ok(Section {
            trailer,
            wanted: found,
        })
    }

    fn table_entry(&self, at: u64) -> Parsed<Option<Location>> {
        let entry = self.read(at, 20)?;
        let mut lexer = Lexer::new(&entry);
        let offset = lexer.unsigned()?;
        lexer.unsigned()?;
        lexer.skip_space();
        match lexer.peek() {
            Some(b'n') => Ok(Some(Location::Offset(offset))),
            Some(b'f') => Ok(None),
            _ => Err(Broken),
        }
    }

    /// The dictionary of object `number`.
    fn dictionary_of(&mut self, start: u64, number: u32) -> Parsed<Dictionary> {
        self.lookups += 1;
        if self.lookups > MAX_LOOKUPS {
            return Err(Broken);
        }
        let value = match self.locate(start, number)? {
            Location::Offset(offset) => {
                let found = self.object_at(offset)?;
                if found.number != number {
                    return Err(Broken);
                }
                found.value
            }
            Location::InStream { stream, index } => {
                let Location::Offset(offset) = self.locate(start, stream)? else {
                    return Err(Broken);
                };
                let container = self.object_at(offset)?;
                let (Object::Dictionary(dictionary), Some(data)) =
                    (container.value, container.stream)
                else {
                    return Err(Broken);
                };
                if container.number != stream {
                    return Err(Broken);
                }
                object_in_stream(&dictionary, &data, number, index)?
            }
        };
        match value {
            Object::Dictionary(dictionary) => Ok(dictionary),
            _ => Err(Broken),
        }
    }

    /// The object at `offset`, with its stream if it has one.
    fn object_at(&mut self, offset: u64) -> Parsed<Found> {
        let mut window = 4096;
        loop {
            let bytes = self.read(offset, window)?;
            let whole_file = bytes.len() < window;
            let mut lexer = Lexer::new(&bytes);
            let parsed = lexer
                .object_header()
                .and_then(|number| Ok((number, lexer.object(0)?)));
            match parsed {
                Ok((number, value)) => {
                    let stream = self.stream_after(offset, &bytes, lexer.position, &value)?;
                    return Ok(Found {
                        number,
                        value,
                        stream,
                    });
                }
                Err(Broken) if !whole_file && window < MAX_OBJECT_BYTES => window *= 4,
                Err(Broken) => return Err(Broken),
            }
        }
    }

    /// The decoded bytes of the stream that follows a dictionary, if one does.
    fn stream_after(
        &self,
        offset: u64,
        bytes: &[u8],
        end: usize,
        value: &Object,
    ) -> Parsed<Option<Vec<u8>>> {
        let Object::Dictionary(dictionary) = value else {
            return Ok(None);
        };
        let mut lexer = Lexer::new(bytes);
        lexer.position = end;
        lexer.skip_space();
        if !bytes[lexer.position..].starts_with(b"stream") {
            return Ok(None);
        }
        let mut start = lexer.position + b"stream".len();
        match bytes.get(start) {
            Some(b'\r') if bytes.get(start + 1) == Some(&b'\n') => start += 2,
            Some(b'\r' | b'\n') => start += 1,
            _ => {}
        }
        let data = offset + start as u64;
        let length = match integer(dictionary, b"Length") {
            Some(length) => usize::try_from(length).map_err(|_| Broken)?,
            // An indirect length is found by where the data ends instead.
            None => self.length_by_endstream(data)?,
        };
        if length > MAX_STREAM_BYTES {
            return Err(Broken);
        }
        decode(dictionary, self.read(data, length)?).map(Some)
    }

    fn length_by_endstream(&self, data: u64) -> Parsed<usize> {
        let mut window = 64 * 1024;
        loop {
            let bytes = self.read(data, window)?;
            if let Some(at) = bytes.windows(9).position(|bytes| bytes == b"endstream") {
                let mut end = at;
                while end > 0 && matches!(bytes[end - 1], b'\r' | b'\n') {
                    end -= 1;
                }
                return Ok(end);
            }
            if bytes.len() < window || window >= MAX_STREAM_BYTES {
                return Err(Broken);
            }
            window *= 4;
        }
    }
}

/// What a cross-reference stream says about object `number`, as `/W` and `/Index` lay it out.
fn stream_entry(
    dictionary: &Dictionary,
    data: &[u8],
    number: u32,
) -> Parsed<Option<Option<Location>>> {
    let widths: Vec<usize> = match get(dictionary, b"W") {
        Some(Object::Array(items)) if items.len() == 3 => items
            .iter()
            .map(|item| match item {
                Object::Integer(width @ 0..=8) => Ok(*width as usize),
                _ => Err(Broken),
            })
            .collect::<Parsed<_>>()?,
        _ => return Err(Broken),
    };
    let row = widths.iter().sum::<usize>();
    if row == 0 {
        return Err(Broken);
    }
    let size = integer(dictionary, b"Size").ok_or(Broken)?;
    let ranges: Vec<(u64, u64)> = match get(dictionary, b"Index") {
        Some(Object::Array(items)) if items.len() % 2 == 0 => items
            .chunks(2)
            .map(|pair| match pair {
                [Object::Integer(first), Object::Integer(count)] => Ok((
                    u64::try_from(*first).map_err(|_| Broken)?,
                    u64::try_from(*count).map_err(|_| Broken)?,
                )),
                _ => Err(Broken),
            })
            .collect::<Parsed<_>>()?,
        None => vec![(0, u64::try_from(size).map_err(|_| Broken)?)],
        Some(_) => return Err(Broken),
    };

    let mut skipped = 0u64;
    for (first, count) in ranges {
        let number = u64::from(number);
        if !(first..first.saturating_add(count)).contains(&number) {
            skipped = skipped.saturating_add(count);
            continue;
        }
        let at = usize::try_from(skipped + (number - first))
            .ok()
            .and_then(|index| index.checked_mul(row))
            .ok_or(Broken)?;
        let Some(bytes) = data.get(at..at + row) else {
            // Listed, but the stream is short: as good as not listed.
            return Ok(None);
        };
        let field = |bytes: &[u8]| {
            bytes
                .iter()
                .fold(0u64, |value, &byte| value << 8 | u64::from(byte))
        };
        let (kind, rest) = bytes.split_at(widths[0]);
        let (second, third) = rest.split_at(widths[1]);
        // A missing type field means type 1.
        let kind = if widths[0] == 0 { 1 } else { field(kind) };
        return Ok(Some(match kind {
            1 => Some(Location::Offset(field(second))),
            2 => Some(Location::InStream {
                stream: u32::try_from(field(second)).map_err(|_| Broken)?,
                index: u32::try_from(field(third)).map_err(|_| Broken)?,
            }),
            _ => None,
        }));
    }
    Ok(None)
}

/// Object `number`, which an object stream holds at position `index`.
fn object_in_stream(
    dictionary: &Dictionary,
    data: &[u8],
    number: u32,
    index: u32,
) -> Parsed<Object> {
    let count = integer(dictionary, b"N").ok_or(Broken)?;
    let first =
        usize::try_from(integer(dictionary, b"First").ok_or(Broken)?).map_err(|_| Broken)?;
    if i64::from(index) >= count {
        return Err(Broken);
    }
    let mut header = Lexer::new(data.get(..first).ok_or(Broken)?);
    for position in 0..=index {
        let object = header.unsigned()?;
        let offset = usize::try_from(header.unsigned()?).map_err(|_| Broken)?;
        if position == index {
            if object != u64::from(number) {
                return Err(Broken);
            }
            let start = first.checked_add(offset).ok_or(Broken)?;
            return Lexer::new(data.get(start..).ok_or(Broken)?).object(0);
        }
    }
    Err(Broken)
}

/// A stream's data after its filters. Only what cross-reference and object streams use is
/// supported: none, or Flate with or without a PNG predictor.
fn decode(dictionary: &Dictionary, raw: Vec<u8>) -> Parsed<Vec<u8>> {
    let filter = match get(dictionary, b"Filter") {
        None => None,
        Some(Object::Name(name)) => Some(name.as_slice()),
        Some(Object::Array(items)) => match items.as_slice() {
            [] => None,
            [Object::Name(name)] => Some(name.as_slice()),
            _ => return Err(Broken),
        },
        Some(_) => return Err(Broken),
    };
    let decoded = match filter {
        None => raw,
        Some(b"FlateDecode" | b"Fl") => inflate(&raw)?,
        Some(_) => return Err(Broken),
    };
    let parameters = match get(dictionary, b"DecodeParms") {
        Some(Object::Dictionary(parameters)) => Some(parameters),
        Some(Object::Array(items)) => match items.first() {
            Some(Object::Dictionary(parameters)) => Some(parameters),
            _ => None,
        },
        _ => None,
    };
    match parameters.and_then(|parameters| integer(parameters, b"Predictor")) {
        None | Some(1) => Ok(decoded),
        Some(10..=15) => {
            let columns = parameters
                .and_then(|parameters| integer(parameters, b"Columns"))
                .unwrap_or(1);
            unpredict_png(&decoded, usize::try_from(columns).map_err(|_| Broken)?)
        }
        Some(_) => Err(Broken),
    }
}

fn inflate(raw: &[u8]) -> Parsed<Vec<u8>> {
    let mut decoded = Vec::new();
    let result = ZlibDecoder::new(raw)
        .take(MAX_STREAM_BYTES as u64 + 1)
        .read_to_end(&mut decoded);
    if decoded.len() > MAX_STREAM_BYTES {
        return Err(Broken);
    }
    // Like the readers people use, take what came out of a stream that ends badly.
    match result {
        Ok(_) => Ok(decoded),
        Err(_) if !decoded.is_empty() => Ok(decoded),
        Err(_) => Err(Broken),
    }
}

/// Undoes the PNG predictors, with one byte per sample, which is how cross-reference streams use them.
fn unpredict_png(data: &[u8], columns: usize) -> Parsed<Vec<u8>> {
    if columns == 0 || columns > MAX_STREAM_BYTES {
        return Err(Broken);
    }
    let mut output = Vec::with_capacity(data.len());
    let mut previous = vec![0u8; columns];
    let mut current = vec![0u8; columns];
    for row in data.chunks(columns + 1) {
        let (&kind, bytes) = row.split_first().ok_or(Broken)?;
        for (at, &byte) in bytes.iter().enumerate() {
            let left = if at > 0 { current[at - 1] } else { 0 };
            let up = previous[at];
            let up_left = if at > 0 { previous[at - 1] } else { 0 };
            current[at] = match kind {
                0 => byte,
                1 => byte.wrapping_add(left),
                2 => byte.wrapping_add(up),
                3 => byte.wrapping_add(((u16::from(left) + u16::from(up)) / 2) as u8),
                4 => byte.wrapping_add(paeth(left, up, up_left)),
                _ => return Err(Broken),
            };
        }
        output.extend_from_slice(&current[..bytes.len()]);
        std::mem::swap(&mut previous, &mut current);
    }
    Ok(output)
}

fn paeth(left: u8, up: u8, up_left: u8) -> u8 {
    let estimate = i16::from(left) + i16::from(up) - i16::from(up_left);
    let (a, b, c) = (
        (estimate - i16::from(left)).abs(),
        (estimate - i16::from(up)).abs(),
        (estimate - i16::from(up_left)).abs(),
    );
    if a <= b && a <= c {
        left
    } else if b <= c {
        up
    } else {
        up_left
    }
}

#[cfg(test)]
mod tests;

//! A PDF's structure as a reader walks it (ISO 32000-1, 7.5): `startxref`, the cross-reference
//! sections from the newest back through `/Prev`, classic tables, streams and hybrid files, objects
//! in the file and in object streams, the trailer, and the page tree with what pages inherit.
//!
//! Only what annotations need is read, every step has a limit, and a file whose structure does not
//! read cleanly is an error: an update written over a structure that was guessed would be a damaged
//! file, so nothing here repairs anything.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;
use std::io::Read;
use std::rc::Rc;

use flate2::read::ZlibDecoder;

use crate::model::Frame;
use crate::syntax::{Dictionary, Lexer, Object, Ref, SyntaxError};

/// Where `startxref` must be, counting back from the end.
const STARTXREF_WITHIN: usize = 64 * 1024;
/// Most cross-reference sections followed through `/Prev`.
const MAX_SECTIONS: usize = 1024;
/// Most bytes a stream this reads may have once decoded.
const MAX_STREAM_BYTES: usize = 256 << 20;
/// Most references followed to reach one value.
const MAX_INDIRECTION: usize = 16;
/// Most pages, and most levels of the page tree.
pub const MAX_PAGES: usize = 1 << 20;
const MAX_TREE_DEPTH: usize = 64;
/// Most entries of one cross-reference subsection.
const MAX_TABLE_ENTRIES: u64 = 8 << 20;

/// Why a file's structure cannot be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PdfError {
    /// Bytes that are not what the structure says is there.
    Syntax(SyntaxError),
    /// A structure that does not hold together.
    Broken(String),
    /// The file is encrypted, and what it holds cannot be read or written without its key.
    Encrypted,
}

impl fmt::Display for PdfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax(error) => write!(f, "{error}"),
            Self::Broken(what) => write!(f, "{what}"),
            Self::Encrypted => write!(f, "the file is encrypted"),
        }
    }
}

impl std::error::Error for PdfError {}

impl From<SyntaxError> for PdfError {
    fn from(error: SyntaxError) -> Self {
        Self::Syntax(error)
    }
}

pub type Result<T> = std::result::Result<T, PdfError>;

pub(crate) fn broken<T>(what: impl Into<String>) -> Result<T> {
    Err(PdfError::Broken(what.into()))
}

/// What a cross-reference section says of an object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XrefEntry {
    Free,
    InFile { offset: u64, generation: u16 },
    InStream { stream: u32, index: u32 },
}

/// How the newest cross-reference section is written, which is how an update must write its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SectionKind {
    /// `xref` … `trailer`, with or without `/XRefStm`.
    Table,
    /// A cross-reference stream.
    Stream,
}

/// The bytes an object was read from: the file, or an object stream's decoded data. The spans of
/// a dictionary read from them count in these bytes.
#[derive(Clone, Debug)]
pub enum Text<'a> {
    File(&'a [u8]),
    Decoded(Rc<Vec<u8>>),
}

impl Text<'_> {
    pub fn bytes(&self) -> &[u8] {
        match self {
            Self::File(bytes) => bytes,
            Self::Decoded(bytes) => bytes,
        }
    }
}

/// An object as read.
#[derive(Clone, Debug)]
pub struct Loaded<'a> {
    pub reference: Ref,
    pub value: Object,
    pub text: Text<'a>,
    /// Where the value is in `text`: its first byte, and just past its last.
    pub span: (usize, usize),
    /// Whether it lives in an object stream rather than in the file.
    pub compressed: bool,
}

impl Loaded<'_> {
    /// The value's bytes, as the file has them.
    pub fn raw(&self) -> &[u8] {
        &self.text.bytes()[self.span.0..self.span.1]
    }
}

/// An object stream, decoded once: its objects' numbers and where each starts.
struct ObjectStream {
    data: Rc<Vec<u8>>,
    offsets: Vec<(u32, usize)>,
}

/// A page, with what it inherits from the tree above it.
#[derive(Clone, Debug)]
pub struct Page {
    pub reference: Ref,
    pub media_box: Option<[f64; 4]>,
    pub crop_box: Option<[f64; 4]>,
    /// `/Rotate` as the file says it.
    pub rotate: i64,
    /// The box shown, and the quarter turns it is shown at, as PDFium works them out.
    pub frame: Frame,
}

/// One cross-reference section: how it is written, its trailer, and its entries.
type Section = (SectionKind, Dictionary, Vec<(u32, XrefEntry)>);

/// A PDF file's structure.
pub struct PdfFile<'a> {
    pub bytes: &'a [u8],
    entries: HashMap<u32, XrefEntry>,
    /// The newest trailer, with what older ones say that it does not.
    pub trailer: Dictionary,
    /// Where the newest cross-reference section starts.
    pub startxref: u64,
    pub newest: SectionKind,
    /// One more than the highest object number in use or declared.
    pub size: u32,
    streams: RefCell<HashMap<u32, Rc<ObjectStream>>>,
}

impl<'a> PdfFile<'a> {
    /// Reads the structure of `bytes`. An encrypted file is refused here: its strings and streams
    /// cannot be read or written without its key.
    pub fn open(bytes: &'a [u8]) -> Result<Self> {
        let file = Self::open_any(bytes)?;
        if file.is_encrypted() {
            return Err(PdfError::Encrypted);
        }
        Ok(file)
    }

    /// Whether the trailer names an encryption dictionary.
    pub fn is_encrypted(&self) -> bool {
        self.trailer.get(b"Encrypt").is_some()
    }

    /// Reads the structure, encrypted or not.
    pub fn open_any(bytes: &'a [u8]) -> Result<Self> {
        let startxref = startxref(bytes)?;
        let mut file = Self {
            bytes,
            entries: HashMap::new(),
            trailer: Dictionary::default(),
            startxref,
            newest: SectionKind::Table,
            size: 0,
            streams: RefCell::new(HashMap::new()),
        };
        let mut next = Some(startxref);
        let mut seen = HashSet::new();
        let mut first = true;
        let mut declared_size = 0u64;
        while let Some(offset) = next {
            if seen.len() >= MAX_SECTIONS {
                return broken("too many cross-reference sections");
            }
            if !seen.insert(offset) {
                return broken("the cross-reference sections go round in a circle");
            }
            let (kind, trailer, entries) = file.section(offset)?;
            if first {
                file.newest = kind;
                first = false;
            }
            next = match trailer.get(b"Prev") {
                None => None,
                Some(prev) => Some(
                    prev.as_number()
                        .filter(|prev| *prev >= 0.0 && prev.fract() == 0.0)
                        .map(|prev| prev as u64)
                        .ok_or_else(|| PdfError::Broken("a /Prev that is no offset".into()))?,
                ),
            };
            // A hybrid file lists its compressed objects in a stream the table points to; they
            // belong to this same section, under the table's own entries.
            let mut hidden = Vec::new();
            if kind == SectionKind::Table
                && let Some(stream) = trailer.integer(b"XRefStm")
            {
                let stream = u64::try_from(stream).or_else(|_| broken("a negative /XRefStm"))?;
                hidden = file.section(stream)?.2;
            }
            let table: HashMap<u32, XrefEntry> = entries.iter().copied().collect();
            for (number, entry) in &entries {
                if *entry != XrefEntry::Free {
                    file.entries.entry(*number).or_insert(*entry);
                }
            }
            for (number, entry) in hidden {
                if !matches!(table.get(&number), Some(XrefEntry::InFile { .. })) {
                    file.entries.entry(number).or_insert(entry);
                }
            }
            for (number, entry) in entries {
                if entry == XrefEntry::Free {
                    file.entries.entry(number).or_insert(entry);
                }
            }
            if let Some(size) = trailer.integer(b"Size") {
                declared_size = declared_size.max(u64::try_from(size).unwrap_or(0));
            }
            for entry in trailer.entries {
                let own = matches!(
                    entry.key.as_slice(),
                    b"Prev"
                        | b"XRefStm"
                        | b"Type"
                        | b"W"
                        | b"Index"
                        | b"Length"
                        | b"Filter"
                        | b"DecodeParms"
                );
                if !own && file.trailer.entry(&entry.key).is_none() {
                    file.trailer.entries.push(entry);
                }
            }
        }
        let highest = file
            .entries
            .keys()
            .max()
            .map_or(0, |&number| u64::from(number) + 1);
        file.size =
            u32::try_from(declared_size.max(highest)).or_else(|_| broken("too many objects"))?;
        if file
            .trailer
            .get(b"Root")
            .and_then(Object::as_reference)
            .is_none()
        {
            return broken("the trailer names no catalog");
        }
        Ok(file)
    }

    /// The section that starts at `offset`.
    fn section(&self, offset: u64) -> Result<Section> {
        let start = usize::try_from(offset)
            .ok()
            .filter(|&start| start < self.bytes.len())
            .ok_or_else(|| PdfError::Broken("a cross-reference section past the end".into()))?;
        let mut lexer = Lexer::at(self.bytes, start);
        lexer.skip_space();
        let at = lexer.position;
        if lexer.token() == b"xref" {
            return self.table(lexer.position);
        }
        lexer.position = at;
        let reference = lexer.object_header()?;
        let dictionary = match lexer.object()? {
            Object::Dictionary(dictionary) => dictionary,
            _ => return broken("a cross-reference stream without a dictionary"),
        };
        if dictionary.name(b"Type") != Some(b"XRef") {
            return broken("startxref points at something that is no cross-reference section");
        }
        let data = self.stream_data(&dictionary, lexer.position, reference)?;
        let entries = stream_entries(&dictionary, &data)?;
        Ok((SectionKind::Stream, dictionary, entries))
    }

    /// A classic table, from just past `xref`.
    fn table(&self, from: usize) -> Result<Section> {
        let mut lexer = Lexer::at(self.bytes, from);
        let mut entries = Vec::new();
        loop {
            lexer.skip_space();
            let at = lexer.position;
            if lexer.token() == b"trailer" {
                break;
            }
            lexer.position = at;
            let first = lexer.unsigned()?;
            let count = lexer.unsigned()?;
            let room = (self.bytes.len() - lexer.position) as u64 / 18;
            if count > MAX_TABLE_ENTRIES || count > room {
                return broken("a cross-reference subsection longer than the file");
            }
            for index in 0..count {
                let offset = lexer.unsigned()?;
                let generation = lexer.unsigned()?;
                lexer.skip_space();
                let kind = lexer.token();
                let number = u32::try_from(first + index)
                    .or_else(|_| broken("an object number too large"))?;
                let generation = u16::try_from(generation).unwrap_or(u16::MAX);
                let entry = match kind {
                    b"n" if offset > 0 => XrefEntry::InFile { offset, generation },
                    // Some writers list an object at offset 0 as in use: it is nowhere.
                    b"n" | b"f" => XrefEntry::Free,
                    _ => return broken("a cross-reference entry neither in use nor free"),
                };
                entries.push((number, entry));
            }
        }
        lexer.skip_space();
        let trailer = match lexer.object()? {
            Object::Dictionary(dictionary) => dictionary,
            _ => return broken("a trailer without a dictionary"),
        };
        Ok((SectionKind::Table, trailer, entries))
    }

    /// Where the cross-reference sections say object `number` is.
    pub fn entry(&self, number: u32) -> Option<XrefEntry> {
        self.entries.get(&number).copied()
    }

    /// Object `number`, as the newest section has it.
    pub fn load(&self, number: u32) -> Result<Loaded<'a>> {
        match self.entries.get(&number) {
            Some(XrefEntry::InFile { offset, generation }) => {
                let start = usize::try_from(*offset)
                    .ok()
                    .filter(|&start| start < self.bytes.len())
                    .ok_or_else(|| PdfError::Broken(format!("object {number} past the end")))?;
                let mut lexer = Lexer::at(self.bytes, start);
                let reference = lexer.object_header()?;
                if reference.number != number {
                    return broken(format!(
                        "the table says object {number} is where object {} is",
                        reference.number
                    ));
                }
                if reference.generation != *generation {
                    return broken(format!("object {number} has another generation"));
                }
                lexer.skip_space();
                let start = lexer.position;
                let value = lexer.object()?;
                Ok(Loaded {
                    reference,
                    value,
                    text: Text::File(self.bytes),
                    span: (start, lexer.position),
                    compressed: false,
                })
            }
            Some(XrefEntry::InStream { stream, index }) => {
                let container = self.object_stream(*stream)?;
                let Some(&(listed, start)) = container.offsets.get(*index as usize) else {
                    return broken(format!("object stream {stream} has no object {index}"));
                };
                if listed != number {
                    return broken(format!(
                        "object stream {stream} holds object {listed}, not {number}"
                    ));
                }
                let mut lexer = Lexer::at(&container.data, start);
                lexer.skip_space();
                let start = lexer.position;
                let value = lexer.object()?;
                Ok(Loaded {
                    reference: Ref {
                        number,
                        generation: 0,
                    },
                    value,
                    text: Text::Decoded(Rc::clone(&container.data)),
                    span: (start, lexer.position),
                    compressed: true,
                })
            }
            Some(XrefEntry::Free) | None => broken(format!("there is no object {number}")),
        }
    }

    /// What `object` is, references followed.
    pub fn resolve(&self, object: &Object) -> Result<Object> {
        let mut current = object.clone();
        for _ in 0..MAX_INDIRECTION {
            match current {
                Object::Reference(reference) => current = self.load(reference.number)?.value,
                other => return Ok(other),
            }
        }
        broken("references that go on and on")
    }

    /// The value of `key` in `dictionary`, references followed; `None` when it is not there or is
    /// a reference to nothing, which counts as null.
    pub fn get(&self, dictionary: &Dictionary, key: &[u8]) -> Result<Option<Object>> {
        match dictionary.get(key) {
            None => Ok(None),
            Some(Object::Reference(reference)) => match self.entries.get(&reference.number) {
                None | Some(XrefEntry::Free) => Ok(None),
                Some(_) => self.resolve(&Object::Reference(*reference)).map(Some),
            },
            Some(value) => Ok(Some(value.clone())),
        }
    }

    /// The decoded data of the stream whose dictionary ends at `after`.
    pub fn stream_data(
        &self,
        dictionary: &Dictionary,
        after: usize,
        owner: Ref,
    ) -> Result<Vec<u8>> {
        let mut lexer = Lexer::at(self.bytes, after);
        lexer.skip_space();
        if !self.bytes[lexer.position..].starts_with(b"stream") {
            return broken(format!("object {} has no stream", owner.number));
        }
        let mut start = lexer.position + b"stream".len();
        match self.bytes.get(start) {
            Some(b'\r') if self.bytes.get(start + 1) == Some(&b'\n') => start += 2,
            Some(b'\r' | b'\n') => start += 1,
            _ => {}
        }
        let declared = match dictionary.get(b"Length") {
            // While the cross-reference sections are still being read, the length's object may
            // not be known yet: then `endstream` says where the data ends.
            Some(Object::Reference(reference)) if reference.number != owner.number => self
                .resolve(&Object::Reference(*reference))
                .ok()
                .and_then(|length| length.as_integer()),
            Some(other) => other.as_integer(),
            None => None,
        };
        let length = declared
            .and_then(|length| usize::try_from(length).ok())
            .filter(|&length| {
                start.checked_add(length).is_some_and(|end| {
                    end <= self.bytes.len() && {
                        let mut after = Lexer::at(self.bytes, end);
                        after.skip_space();
                        self.bytes[after.position..].starts_with(b"endstream")
                    }
                })
            });
        let length = match length {
            Some(length) => length,
            // A length that is wrong or missing: where `endstream` is says it.
            None => {
                let rest = &self.bytes[start..];
                let at = rest
                    .windows(9)
                    .position(|window| window == b"endstream")
                    .ok_or_else(|| {
                        PdfError::Broken(format!("object {}'s stream does not end", owner.number))
                    })?;
                let mut end = at;
                while end > 0 && matches!(rest[end - 1], b'\r' | b'\n') {
                    end -= 1;
                }
                end
            }
        };
        decode(dictionary, &self.bytes[start..start + length])
    }

    fn object_stream(&self, number: u32) -> Result<Rc<ObjectStream>> {
        if let Some(stream) = self.streams.borrow().get(&number) {
            return Ok(Rc::clone(stream));
        }
        let Some(XrefEntry::InFile { offset, .. }) = self.entries.get(&number).copied() else {
            return broken(format!("object stream {number} is not in the file"));
        };
        let start = usize::try_from(offset)
            .ok()
            .filter(|&start| start < self.bytes.len())
            .ok_or_else(|| PdfError::Broken(format!("object stream {number} past the end")))?;
        let mut lexer = Lexer::at(self.bytes, start);
        let reference = lexer.object_header()?;
        if reference.number != number {
            return broken(format!(
                "object stream {number} is not where the table says"
            ));
        }
        let Object::Dictionary(dictionary) = lexer.object()? else {
            return broken(format!("object stream {number} has no dictionary"));
        };
        let data = self.stream_data(&dictionary, lexer.position, reference)?;
        let count = dictionary
            .integer(b"N")
            .and_then(|count| usize::try_from(count).ok())
            .ok_or_else(|| PdfError::Broken(format!("object stream {number} without /N")))?;
        let first = dictionary
            .integer(b"First")
            .and_then(|first| usize::try_from(first).ok())
            .filter(|&first| first <= data.len())
            .ok_or_else(|| PdfError::Broken(format!("object stream {number} without /First")))?;
        if count > data.len() / 2 + 1 {
            return broken(format!("object stream {number} claims too many objects"));
        }
        let mut header = Lexer::new(&data[..first]);
        let mut offsets = Vec::with_capacity(count);
        for _ in 0..count {
            let object = u32::try_from(header.unsigned()?)
                .or_else(|_| broken("an object number too large"))?;
            let at = usize::try_from(header.unsigned()?)
                .ok()
                .and_then(|at| first.checked_add(at))
                .filter(|&at| at <= data.len())
                .ok_or_else(|| {
                    PdfError::Broken(format!("object stream {number} points past its end"))
                })?;
            offsets.push((object, at));
        }
        let stream = Rc::new(ObjectStream {
            data: Rc::new(data),
            offsets,
        });
        self.streams.borrow_mut().insert(number, Rc::clone(&stream));
        Ok(stream)
    }

    /// The catalog.
    pub fn catalog(&self) -> Result<Loaded<'a>> {
        let root = self
            .trailer
            .get(b"Root")
            .and_then(Object::as_reference)
            .ok_or_else(|| PdfError::Broken("the trailer names no catalog".into()))?;
        let loaded = self.load(root.number)?;
        if loaded.value.as_dictionary().is_none() {
            return broken("the catalog is no dictionary");
        }
        Ok(loaded)
    }

    /// The pages in order, each with what it inherits.
    pub fn pages(&self) -> Result<Vec<Page>> {
        let catalog = self.catalog()?;
        let Some(Object::Reference(root)) = catalog
            .value
            .as_dictionary()
            .and_then(|catalog| catalog.get(b"Pages"))
            .cloned()
        else {
            return broken("the catalog names no page tree");
        };
        let mut pages = Vec::new();
        let mut visited = HashSet::new();
        self.walk(root, Inherited::default(), 0, &mut visited, &mut pages)?;
        Ok(pages)
    }

    fn walk(
        &self,
        node: Ref,
        inherited: Inherited,
        depth: usize,
        visited: &mut HashSet<u32>,
        pages: &mut Vec<Page>,
    ) -> Result<()> {
        if depth > MAX_TREE_DEPTH {
            return broken("a page tree too deep");
        }
        if !visited.insert(node.number) {
            return broken("the page tree goes round in a circle");
        }
        let loaded = self.load(node.number)?;
        let Object::Dictionary(dictionary) = loaded.value else {
            return broken(format!("page tree node {} is no dictionary", node.number));
        };
        let own = Inherited {
            media_box: self
                .rectangle(&dictionary, b"MediaBox")?
                .or(inherited.media_box),
            crop_box: self
                .rectangle(&dictionary, b"CropBox")?
                .or(inherited.crop_box),
            rotate: match self.get(&dictionary, b"Rotate")? {
                Some(value) => value.as_number().map(|value| value as i64),
                None => inherited.rotate,
            },
        };
        let kids = self.get(&dictionary, b"Kids")?;
        let is_leaf = match dictionary.name(b"Type") {
            Some(b"Page") => true,
            Some(b"Pages") => false,
            _ => kids.is_none(),
        };
        if is_leaf {
            if pages.len() >= MAX_PAGES {
                return broken("too many pages");
            }
            let rotate = own.rotate.unwrap_or(0);
            pages.push(Page {
                reference: loaded.reference,
                media_box: own.media_box,
                crop_box: own.crop_box,
                rotate,
                frame: Frame::of(own.media_box, own.crop_box, rotate),
            });
            return Ok(());
        }
        let Some(Object::Array(kids)) = kids else {
            return broken(format!("page tree node {} has no kids", node.number));
        };
        for kid in kids {
            let Object::Reference(kid) = kid else {
                return broken("a page tree kid that is no reference");
            };
            self.walk(kid, own, depth + 1, visited, pages)?;
        }
        Ok(())
    }

    /// A rectangle as four numbers, references followed, or `None` when it is not there or is not
    /// one, which is what PDFium makes of it too.
    fn rectangle(&self, dictionary: &Dictionary, key: &[u8]) -> Result<Option<[f64; 4]>> {
        let Some(value) = self.get(dictionary, key)? else {
            return Ok(None);
        };
        let Object::Array(items) = value else {
            return Ok(None);
        };
        let mut numbers = Vec::with_capacity(4);
        for item in &items {
            match self.resolve(item)?.as_number() {
                Some(number) => numbers.push(number),
                None => return Ok(None),
            }
        }
        Ok(<[f64; 4]>::try_from(numbers).ok())
    }

    /// The pages, from 0, whose `/Annots` is an array with something in it.
    pub fn pages_with_annotations(&self) -> Result<BTreeSet<u32>> {
        let mut found = BTreeSet::new();
        for (index, page) in self.pages()?.iter().enumerate() {
            let loaded = self.load(page.reference.number)?;
            if let Some(dictionary) = loaded.value.as_dictionary()
                && let Some(Object::Array(items)) = self.get(dictionary, b"Annots")?
                && !items.is_empty()
            {
                found.insert(index as u32);
            }
        }
        Ok(found)
    }

    /// The trailer's `/ID`, if it has one: the permanent identifier and the changing one.
    pub fn id(&self) -> Option<(Vec<u8>, Vec<u8>)> {
        let ids = self.trailer.get(b"ID")?.as_array()?;
        Some((
            ids.first()?.as_string()?.to_vec(),
            ids.get(1)?.as_string()?.to_vec(),
        ))
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Inherited {
    media_box: Option<[f64; 4]>,
    crop_box: Option<[f64; 4]>,
    rotate: Option<i64>,
}

/// The offset `startxref` names: the last one in the file's tail.
fn startxref(bytes: &[u8]) -> Result<u64> {
    let from = bytes.len().saturating_sub(STARTXREF_WITHIN);
    let tail = &bytes[from..];
    let at = tail
        .windows(9)
        .rposition(|window| window == b"startxref")
        .ok_or_else(|| PdfError::Broken("there is no startxref".into()))?;
    let mut lexer = Lexer::at(tail, at + 9);
    Ok(lexer.unsigned()?)
}

/// What a cross-reference stream lists, as `/W` and `/Index` lay it out.
fn stream_entries(dictionary: &Dictionary, data: &[u8]) -> Result<Vec<(u32, XrefEntry)>> {
    let widths: Vec<usize> = dictionary
        .get(b"W")
        .and_then(Object::as_array)
        .filter(|items| items.len() == 3)
        .and_then(|items| {
            items
                .iter()
                .map(|item| {
                    item.as_integer()
                        .filter(|w| (0..=8).contains(w))
                        .map(|w| w as usize)
                })
                .collect()
        })
        .ok_or_else(|| PdfError::Broken("a cross-reference stream without /W".into()))?;
    let row = widths.iter().sum::<usize>();
    if row == 0 {
        return broken("a cross-reference stream with empty rows");
    }
    let size = dictionary
        .integer(b"Size")
        .and_then(|size| u64::try_from(size).ok())
        .ok_or_else(|| PdfError::Broken("a cross-reference stream without /Size".into()))?;
    let ranges: Vec<(u64, u64)> = match dictionary.get(b"Index") {
        None => vec![(0, size)],
        Some(Object::Array(items)) if items.len() % 2 == 0 => items
            .chunks(2)
            .map(|pair| {
                Some((
                    u64::try_from(pair[0].as_integer()?).ok()?,
                    u64::try_from(pair[1].as_integer()?).ok()?,
                ))
            })
            .collect::<Option<_>>()
            .ok_or_else(|| PdfError::Broken("a cross-reference stream with a bad /Index".into()))?,
        Some(_) => return broken("a cross-reference stream with a bad /Index"),
    };
    let field = |bytes: &[u8]| {
        bytes
            .iter()
            .fold(0u64, |value, &byte| value << 8 | u64::from(byte))
    };
    let mut entries = Vec::new();
    let mut rows = data.chunks_exact(row);
    for (first, count) in ranges {
        if count > MAX_TABLE_ENTRIES {
            return broken("a cross-reference stream too long");
        }
        for index in 0..count {
            // Listed, but the stream is short: as good as not listed.
            let Some(bytes) = rows.next() else {
                return Ok(entries);
            };
            let number =
                u32::try_from(first + index).or_else(|_| broken("an object number too large"))?;
            let (kind, rest) = bytes.split_at(widths[0]);
            let (second, third) = rest.split_at(widths[1]);
            // A missing type field means type 1.
            let kind = if widths[0] == 0 { 1 } else { field(kind) };
            let entry = match kind {
                0 => XrefEntry::Free,
                1 => {
                    let offset = field(second);
                    if offset == 0 {
                        XrefEntry::Free
                    } else {
                        XrefEntry::InFile {
                            offset,
                            generation: u16::try_from(field(third)).unwrap_or(u16::MAX),
                        }
                    }
                }
                2 => XrefEntry::InStream {
                    stream: u32::try_from(field(second))
                        .or_else(|_| broken("a bad object stream number"))?,
                    index: u32::try_from(field(third))
                        .or_else(|_| broken("a bad object stream index"))?,
                },
                // Other types are reserved, and mean null.
                _ => XrefEntry::Free,
            };
            entries.push((number, entry));
        }
    }
    Ok(entries)
}

/// A stream's data after its filters. Only Flate, with or without a PNG predictor, is decoded:
/// what cross-reference streams, object streams and appearance streams use.
pub fn decode(dictionary: &Dictionary, raw: &[u8]) -> Result<Vec<u8>> {
    let filter = match dictionary.get(b"Filter") {
        None => None,
        Some(Object::Name(name)) => Some(name.as_slice()),
        Some(Object::Array(items)) => match items.as_slice() {
            [] => None,
            [Object::Name(name)] => Some(name.as_slice()),
            _ => return broken("a stream with filters this does not decode"),
        },
        Some(_) => return broken("a stream with a filter that is no name"),
    };
    let decoded = match filter {
        None => raw.to_vec(),
        Some(b"FlateDecode" | b"Fl") => inflate(raw)?,
        Some(other) => {
            return broken(format!(
                "a stream with the filter {}",
                String::from_utf8_lossy(other)
            ));
        }
    };
    let parameters = match dictionary.get(b"DecodeParms") {
        Some(Object::Dictionary(parameters)) => Some(parameters),
        Some(Object::Array(items)) => items.first().and_then(Object::as_dictionary),
        _ => None,
    };
    match parameters.and_then(|parameters| parameters.integer(b"Predictor")) {
        None | Some(1) => Ok(decoded),
        Some(10..=15) => {
            let columns = parameters
                .and_then(|parameters| parameters.integer(b"Columns"))
                .unwrap_or(1);
            let columns = usize::try_from(columns).or_else(|_| broken("negative /Columns"))?;
            unpredict_png(&decoded, columns)
        }
        Some(_) => broken("a predictor this does not undo"),
    }
}

fn inflate(raw: &[u8]) -> Result<Vec<u8>> {
    let mut decoded = Vec::new();
    let result = ZlibDecoder::new(raw)
        .take(MAX_STREAM_BYTES as u64 + 1)
        .read_to_end(&mut decoded);
    if decoded.len() > MAX_STREAM_BYTES {
        return broken("a stream too large once decoded");
    }
    // Like the readers people use, take what came out of a stream that ends badly.
    match result {
        Ok(_) => Ok(decoded),
        Err(_) if !decoded.is_empty() => Ok(decoded),
        Err(error) => broken(format!("a stream that does not inflate: {error}")),
    }
}

/// Undoes the PNG predictors with one byte per sample, as cross-reference streams use them.
fn unpredict_png(data: &[u8], columns: usize) -> Result<Vec<u8>> {
    if columns == 0 || columns > MAX_STREAM_BYTES {
        return broken("a predictor with no columns");
    }
    let mut output = Vec::with_capacity(data.len());
    let mut previous = vec![0u8; columns];
    let mut current = vec![0u8; columns];
    for row in data.chunks(columns + 1) {
        let Some((&kind, bytes)) = row.split_first() else {
            break;
        };
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
                _ => return broken("an unknown PNG predictor"),
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

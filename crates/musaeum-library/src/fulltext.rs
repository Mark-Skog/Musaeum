//! The books' text, searchable: the full-text index, as the plan has it, Tantivy with BM25 ranking,
//! by page or by section.
//!
//! The index is kept by the files' bytes, like the thumbnails: every copy of a file shares its
//! entries, a file that changed is read again, and it lives in the cache folder
//! (`cache/fulltext/v<version>`), so it can always be thrown away and made again from the books.
//! `cache.db` records which files are in it (`fulltext_content`), written after each commit of the
//! index; a file indexed twice by a crash between the two has its entries replaced, not doubled.
//!
//! A unit is what a result points to: a page of a fixed-layout book (its text layer, or the lines
//! OCR read on it), or one of a flowing book's own documents (a chapter, as the reader counts
//! them, so a position in it is the reader's). What reads the text is the desktop's: the PDF and
//! DjVu workers, the normalizer, the OCR cache. This module keeps it and searches it.
//!
//! Words are compared as names are elsewhere: lower case and without accents ("memoria" finds
//! "memória"). A query is its words, all of which must be there, and its "quoted phrases", which
//! must be there in that order; nothing a person types is read as a query language.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use rusqlite::params;
use serde::{Deserialize, Serialize};
use tantivy::collector::{Count, TopDocs};
use tantivy::directory::MmapDirectory;
use tantivy::query::{BooleanQuery, Occur, PhraseQuery, Query, TermQuery};
use tantivy::schema::{
    Field, IndexRecordOption, STORED, STRING, Schema, TextFieldIndexing, TextOptions, Value,
};
use tantivy::tokenizer::{
    AsciiFoldingFilter, LowerCaser, RemoveLongFilter, SimpleTokenizer, TextAnalyzer,
};
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument, Term};

use crate::catalog::BookFile;
use crate::error::{Error, Result, io_error};
use crate::fingerprint::Hash;
use crate::library::Library;
use crate::model::{DocumentId, WorkId, hex};
use crate::sql::now_ms;

/// The version of the index's form and of what reads the text into it: another one reads every
/// book again, into a folder of its own.
pub const FULLTEXT_VERSION: u32 = 1;
/// What the one indexing thread may hold in memory before it writes a segment.
const WRITER_MEMORY: usize = 24 << 20;
/// The most units a search ranks before it groups them by book.
const MAX_UNITS: usize = 500;
/// The most results by book, and units by book, a search returns.
pub const MAX_BOOKS_FOUND: usize = 100;
const UNITS_PER_BOOK: usize = 5;
/// The longest a unit's text may be; past it, it is cut.
const MAX_UNIT_CHARS: usize = 4 << 20;
/// The longest a query may be, and the most words in it.
const MAX_QUERY_CHARS: usize = 500;
const MAX_QUERY_TERMS: usize = 32;
/// Characters of context before and after the first match in a snippet.
const BEFORE: usize = 70;
const AFTER: usize = 150;
/// The name the analyzer is registered under.
const ANALYZER: &str = "musaeum_folded";

/// What a unit is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnitKind {
    /// A page of a fixed-layout book, from 0.
    Page,
    /// One of a flowing book's own documents, from 0, as the reader counts them.
    Document,
}

/// Where a unit's text came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextSource {
    TextLayer,
    Ocr,
    Flow,
}

impl UnitKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::Document => "document",
        }
    }
    fn parse(text: &str) -> Option<Self> {
        match text {
            "page" => Some(Self::Page),
            "document" => Some(Self::Document),
            _ => None,
        }
    }
}

impl TextSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::TextLayer => "text_layer",
            Self::Ocr => "ocr",
            Self::Flow => "flow",
        }
    }
    fn parse(text: &str) -> Option<Self> {
        match text {
            "text_layer" => Some(Self::TextLayer),
            "ocr" => Some(Self::Ocr),
            "flow" => Some(Self::Flow),
            _ => None,
        }
    }
}

/// One stretch of a book's text, as the index keeps it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextUnit {
    pub kind: UnitKind,
    pub index: u32,
    /// What the reader calls it, if anything: a chapter's title.
    pub label: Option<String>,
    pub source: TextSource,
    pub text: String,
}

/// A unit a search found, with a stretch of its text around the first match.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnitHit {
    pub kind: UnitKind,
    pub index: u32,
    pub label: Option<String>,
    pub source: TextSource,
    pub score: f32,
    /// The text around the match, with `…` where it was cut.
    pub snippet: String,
    /// What matched in the snippet, as UTF-16 ranges, which is how the page counts.
    pub marks: Vec<[u32; 2]>,
    /// Where the first match starts in the unit's whole text, in UTF-16 code units.
    pub position: u32,
    /// What it matched, as written there.
    pub matched: String,
}

/// A book a search found.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BookHit {
    /// The bytes it was found in, in hex.
    pub blake3: String,
    pub score: f32,
    /// The documents whose file has those bytes now, and their works.
    pub documents: Vec<DocumentId>,
    pub works: Vec<WorkId>,
    pub units: Vec<UnitHit>,
}

/// What a search found.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FullTextResults {
    /// The words and phrases it looked for, as compared.
    pub terms: Vec<String>,
    pub books: Vec<BookHit>,
    /// Units that matched, in all books.
    pub units: usize,
    pub elapsed_ms: f64,
}

/// How far the index is.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FullTextStatus {
    /// Files whose text is in the index.
    pub indexed: u64,
    /// Files that gave no text.
    pub empty: u64,
    pub failed: u64,
    /// Files still to read.
    pub pending: u64,
    pub units: u64,
    pub chars: u64,
}

/// A file to read into the index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingText {
    pub file: BookFile,
    pub blake3: Hash,
    /// The lines OCR read in it so far, to be recorded with it.
    pub ocr_lines: u64,
    /// Whether it was in the index before, with fewer OCR lines: only its pages change.
    pub reindex: bool,
}

struct Fields {
    content: Field,
    kind: Field,
    unit: Field,
    label: Field,
    source: Field,
    text: Field,
}

pub(crate) struct FullText {
    index: Index,
    reader: IndexReader,
    writer: Mutex<Option<IndexWriter>>,
    fields: Fields,
}

fn analyzer() -> TextAnalyzer {
    TextAnalyzer::builder(SimpleTokenizer::default())
        .filter(RemoveLongFilter::limit(60))
        .filter(LowerCaser)
        .filter(AsciiFoldingFilter)
        .build()
}

fn schema() -> (Schema, Fields) {
    let mut builder = Schema::builder();
    let content = builder.add_text_field("content", STRING | STORED);
    let kind = builder.add_text_field("kind", STRING | STORED);
    let unit = builder.add_u64_field("unit", STORED);
    let label = builder.add_text_field("label", STORED);
    let source = builder.add_text_field("source", STRING | STORED);
    let text = builder.add_text_field(
        "text",
        TextOptions::default()
            .set_indexing_options(
                TextFieldIndexing::default()
                    .set_tokenizer(ANALYZER)
                    .set_index_option(IndexRecordOption::WithFreqsAndPositions),
            )
            .set_stored(),
    );
    (
        builder.build(),
        Fields {
            content,
            kind,
            unit,
            label,
            source,
            text,
        },
    )
}

/// Where a library keeps its index.
pub(crate) fn index_dir(cache_dir: &Path) -> PathBuf {
    cache_dir
        .join("fulltext")
        .join(format!("v{FULLTEXT_VERSION}"))
}

/// What a query asks for: its words, and its phrases of more than one word, folded.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Asked {
    words: Vec<String>,
    phrases: Vec<Vec<String>>,
}

impl Asked {
    fn is_empty(&self) -> bool {
        self.words.is_empty() && self.phrases.is_empty()
    }
    fn shown(&self) -> Vec<String> {
        self.words
            .iter()
            .cloned()
            .chain(
                self.phrases
                    .iter()
                    .map(|phrase| format!("\"{}\"", phrase.join(" "))),
            )
            .collect()
    }
}

fn tokens(text: &str) -> Vec<String> {
    let mut analyzer = analyzer();
    let mut stream = analyzer.token_stream(text);
    let mut out = Vec::new();
    while stream.advance() {
        out.push(stream.token().text.clone());
    }
    out
}

/// Reads a query: "quoted" stretches are phrases, the rest words; a word the analyzer splits
/// ("pré-história") is a phrase of its parts.
fn parse_query(query: &str) -> Asked {
    let query: String = query.chars().take(MAX_QUERY_CHARS).collect();
    let mut asked = Asked::default();
    let mut add = |segment: &str, quoted: bool| {
        if quoted {
            let terms = tokens(segment);
            match terms.len() {
                0 => {}
                1 => asked.words.extend(terms),
                _ => asked.phrases.push(terms),
            }
        } else {
            for word in segment.split_whitespace() {
                let terms = tokens(word);
                match terms.len() {
                    0 => {}
                    1 => asked.words.extend(terms),
                    _ => asked.phrases.push(terms),
                }
            }
        }
    };
    for (index, segment) in query.split('"').enumerate() {
        add(segment, index % 2 == 1);
    }
    let mut seen = HashSet::new();
    asked.words.retain(|word| seen.insert(word.clone()));
    let mut budget = MAX_QUERY_TERMS;
    asked.words.truncate(budget);
    budget -= asked.words.len();
    asked.phrases.retain(|phrase| {
        let fits = phrase.len() <= budget;
        if fits {
            budget -= phrase.len();
        }
        fits
    });
    asked
}

/// A token of a text: where it is, in bytes, and what it is, folded.
struct Token {
    from: usize,
    to: usize,
    text: String,
}

fn tokens_of(text: &str) -> Vec<Token> {
    let mut analyzer = analyzer();
    let mut stream = analyzer.token_stream(text);
    let mut out = Vec::new();
    while stream.advance() {
        let token = stream.token();
        out.push(Token {
            from: token.offset_from,
            to: token.offset_to,
            text: token.text.clone(),
        });
    }
    out
}

fn utf16_len(text: &str) -> u32 {
    text.encode_utf16().count() as u32
}

/// The byte at or before `at` that starts a character.
fn floor_char(text: &str, mut at: usize) -> usize {
    at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// A stretch of `text` around its first match of what was asked, the matches in it marked.
fn snippet(text: &str, asked: &Asked) -> Option<(String, Vec<[u32; 2]>, u32, String)> {
    let tokens = tokens_of(text);
    let words: HashSet<&str> = asked.words.iter().map(String::as_str).collect();
    // Which tokens matched: a word anywhere, a phrase where all of it follows in order.
    let mut matched = vec![false; tokens.len()];
    let mut first: Option<usize> = None;
    for (at, token) in tokens.iter().enumerate() {
        if words.contains(token.text.as_str()) {
            matched[at] = true;
            first = first.or(Some(at));
        }
        for phrase in &asked.phrases {
            if tokens.len() - at >= phrase.len()
                && phrase
                    .iter()
                    .enumerate()
                    .all(|(offset, term)| tokens[at + offset].text == *term)
            {
                for offset in 0..phrase.len() {
                    matched[at + offset] = true;
                }
                // A phrase is the better place to show: it is what was asked most precisely.
                if first.is_none_or(|found| {
                    !asked.phrases.is_empty() && !phrase_starts(&tokens, found, asked)
                }) {
                    first = Some(at);
                }
            }
        }
    }
    let first = first?;
    let anchor = &tokens[first];
    let start = floor_char(text, anchor.from.saturating_sub(BEFORE * 2));
    let start = text[start..anchor.from]
        .char_indices()
        .rev()
        .nth(BEFORE)
        .map_or(start, |(at, _)| start + at);
    // From a word's start, not the middle of one.
    let start = if start == 0 {
        0
    } else {
        text[start..anchor.from]
            .char_indices()
            .find(|(_, c)| c.is_whitespace())
            .map_or(start, |(at, c)| start + at + c.len_utf8())
    };
    let mut end = text[anchor.from..]
        .char_indices()
        .nth(AFTER)
        .map_or(text.len(), |(at, _)| anchor.from + at);
    if end < text.len() {
        end = text[..end]
            .rfind(char::is_whitespace)
            .filter(|&at| at > anchor.to)
            .unwrap_or(end);
    }
    let prefix = if start > 0 { "…" } else { "" };
    let suffix = if end < text.len() { "…" } else { "" };
    let body: String = text[start..end]
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let mut marks: Vec<[u32; 2]> = Vec::new();
    let base = utf16_len(prefix);
    for (at, token) in tokens.iter().enumerate() {
        if matched[at] && token.from >= start && token.to <= end {
            let from = base + utf16_len(&text[start..token.from]);
            let to = from + utf16_len(&text[token.from..token.to]);
            // Two marks one space apart are one: a phrase reads as one.
            match marks.last_mut() {
                Some([_, last_to])
                    if from == *last_to + 1
                        && text[..token.from].ends_with(char::is_whitespace) =>
                {
                    *last_to = to
                }
                _ => marks.push([from, to]),
            }
        }
    }
    let position = utf16_len(&text[..anchor.from]);
    let shown = text[anchor.from..anchor.to].to_owned();
    Some((format!("{prefix}{body}{suffix}"), marks, position, shown))
}

fn phrase_starts(tokens: &[Token], at: usize, asked: &Asked) -> bool {
    asked.phrases.iter().any(|phrase| {
        tokens.len() - at >= phrase.len()
            && phrase
                .iter()
                .enumerate()
                .all(|(offset, term)| tokens[at + offset].text == *term)
    })
}

impl FullText {
    pub(crate) fn open(cache_dir: &Path) -> Result<Self> {
        let dir = index_dir(cache_dir);
        fs::create_dir_all(&dir).map_err(io_error("creating the full-text index's folder"))?;
        let (schema, fields) = schema();
        let open = |dir: &Path| -> Result<Index> {
            let directory =
                MmapDirectory::open(dir).map_err(|error| Error::FullText(error.to_string()))?;
            Ok(Index::open_or_create(directory, schema.clone())?)
        };
        // An index this version cannot read is thrown away: it is made again from the books.
        let index = match open(&dir) {
            Ok(index) => index,
            Err(error) => {
                eprintln!("the full-text index is made again: {error}");
                fs::remove_dir_all(&dir)
                    .map_err(io_error("removing an unreadable full-text index"))?;
                fs::create_dir_all(&dir)
                    .map_err(io_error("creating the full-text index's folder"))?;
                open(&dir)?
            }
        };
        index.tokenizers().register(ANALYZER, analyzer());
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()?;
        Ok(Self {
            index,
            reader,
            writer: Mutex::new(None),
            fields,
        })
    }

    fn with_writer<T>(
        &self,
        work: impl FnOnce(&mut IndexWriter, &Fields) -> Result<T>,
    ) -> Result<T> {
        let mut slot = self
            .writer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot.is_none() {
            *slot = Some(self.index.writer_with_num_threads(1, WRITER_MEMORY)?);
        }
        let writer = slot.as_mut().expect("the writer was just made");
        let result = work(writer, &self.fields);
        if result.is_err() {
            // Whatever it added is not committed; the next use starts clean.
            let _ = writer.rollback();
        }
        result
    }

    /// Replaces what the index has for these bytes with these units, and commits.
    fn store(&self, blake3: &Hash, units: &[TextUnit]) -> Result<()> {
        let content = hex(blake3);
        self.with_writer(|writer, fields| {
            writer.delete_term(Term::from_field_text(fields.content, &content));
            for unit in units {
                let mut text = unit.text.as_str();
                if text.len() > MAX_UNIT_CHARS {
                    text = &text[..floor_char(text, MAX_UNIT_CHARS)];
                }
                if text.trim().is_empty() {
                    continue;
                }
                let mut document = TantivyDocument::default();
                document.add_text(fields.content, &content);
                document.add_text(fields.kind, unit.kind.as_str());
                document.add_u64(fields.unit, u64::from(unit.index));
                if let Some(label) = &unit.label {
                    document.add_text(fields.label, label);
                }
                document.add_text(fields.source, unit.source.as_str());
                document.add_text(fields.text, text);
                writer.add_document(document)?;
            }
            writer.commit()?;
            Ok(())
        })?;
        self.reader.reload()?;
        Ok(())
    }

    /// Takes these bytes' entries out of the index.
    fn forget(&self, contents: &[Hash]) -> Result<()> {
        if contents.is_empty() {
            return Ok(());
        }
        self.with_writer(|writer, fields| {
            for blake3 in contents {
                writer.delete_term(Term::from_field_text(fields.content, &hex(blake3)));
            }
            writer.commit()?;
            Ok(())
        })?;
        self.reader.reload()?;
        Ok(())
    }

    fn query(&self, asked: &Asked) -> Box<dyn Query> {
        let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        for word in &asked.words {
            clauses.push((
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_text(self.fields.text, word),
                    IndexRecordOption::WithFreqs,
                )),
            ));
        }
        for phrase in &asked.phrases {
            let terms = phrase
                .iter()
                .map(|term| Term::from_field_text(self.fields.text, term))
                .collect();
            clauses.push((Occur::Must, Box::new(PhraseQuery::new(terms))));
        }
        Box::new(BooleanQuery::new(clauses))
    }

    /// The units that match, best first, at most `limit`, and how many matched in all.
    fn search(&self, asked: &Asked, limit: usize) -> Result<(Vec<(f32, StoredUnit)>, usize)> {
        let searcher = self.reader.searcher();
        let query = self.query(asked);
        let (top, count) = searcher.search(
            &query,
            &(TopDocs::with_limit(limit).order_by_score(), Count),
        )?;
        let mut found = Vec::with_capacity(top.len());
        for (score, address) in top {
            let document: TantivyDocument = searcher.doc(address)?;
            let text = |field: Field| {
                document
                    .get_first(field)
                    .and_then(|value| value.as_str())
                    .map(str::to_owned)
            };
            let (Some(content), Some(kind), Some(unit), Some(source), Some(body)) = (
                text(self.fields.content),
                text(self.fields.kind).as_deref().and_then(UnitKind::parse),
                document
                    .get_first(self.fields.unit)
                    .and_then(|value| value.as_u64()),
                text(self.fields.source)
                    .as_deref()
                    .and_then(TextSource::parse),
                text(self.fields.text),
            ) else {
                continue;
            };
            found.push((
                score,
                StoredUnit {
                    content,
                    kind,
                    index: unit as u32,
                    label: text(self.fields.label),
                    source,
                    text: body,
                },
            ));
        }
        Ok((found, count))
    }

    fn units(&self) -> u64 {
        self.reader.searcher().num_docs()
    }
}

struct StoredUnit {
    content: String,
    kind: UnitKind,
    index: u32,
    label: Option<String>,
    source: TextSource,
    text: String,
}

fn hash_of_hex(text: &str) -> Option<Hash> {
    if text.len() != 64 {
        return None;
    }
    let mut hash = [0u8; 32];
    for (at, byte) in hash.iter_mut().enumerate() {
        *byte = u8::from_str_radix(text.get(at * 2..at * 2 + 2)?, 16).ok()?;
    }
    Some(hash)
}

impl Library {
    fn fulltext_index(&self) -> Result<&FullText> {
        self.fulltext()
            .ok_or_else(|| Error::FullText("there is no full-text index here".to_owned()))
    }

    /// Files whose text the index does not have yet, or has from fewer OCR lines than the cache
    /// holds now: one per set of bytes, at most `limit`.
    pub fn fulltext_pending(&self, limit: usize) -> Result<Vec<PendingText>> {
        let Some(cache) = self.cache() else {
            return Ok(Vec::new());
        };
        let files = self.hashed_book_files()?;
        let known: HashMap<Hash, (u32, String, u64)> = cache.read(|connection| {
            Ok(connection
                .prepare_cached("SELECT blake3, version, state, ocr_lines FROM fulltext_content")?
                .query_map([], |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, u32>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                })?
                .filter_map(|row| {
                    row.ok().and_then(|(hash, version, state, lines)| {
                        Some((
                            <[u8; 32]>::try_from(hash.as_slice()).ok()?,
                            (version, state, lines.max(0) as u64),
                        ))
                    })
                })
                .collect())
        })?;
        let ocr: HashMap<Hash, u64> = cache.read(|connection| {
            Ok(connection
                .prepare_cached("SELECT blake3, count(*) FROM ocr_line GROUP BY blake3")?
                .query_map([], |row| {
                    Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, i64>(1)?))
                })?
                .filter_map(|row| {
                    row.ok().and_then(|(hash, count)| {
                        Some((
                            <[u8; 32]>::try_from(hash.as_slice()).ok()?,
                            count.max(0) as u64,
                        ))
                    })
                })
                .collect())
        })?;
        let mut seen = HashSet::new();
        let mut pending = Vec::new();
        for file in files {
            let Some(blake3) = file.blake3 else { continue };
            if !seen.insert(blake3) {
                continue;
            }
            let lines = ocr.get(&blake3).copied().unwrap_or(0);
            let (needed, reindex) = match known.get(&blake3) {
                None => (true, false),
                Some((version, _, _)) if *version != FULLTEXT_VERSION => (true, false),
                // A failure is tried again only by another version.
                Some((_, state, _)) if state == "failed" => (false, false),
                Some((_, _, before)) => (file.layout == "fixed" && lines > *before, true),
            };
            if needed {
                pending.push(PendingText {
                    file,
                    blake3,
                    ocr_lines: lines,
                    reindex,
                });
                if pending.len() >= limit {
                    break;
                }
            }
        }
        Ok(pending)
    }

    /// Keeps a file's text in the index, replacing what it had for those bytes.
    pub fn fulltext_store(&self, blake3: &Hash, units: &[TextUnit], ocr_lines: u64) -> Result<()> {
        self.fulltext_index()?.store(blake3, units)?;
        let chars: u64 = units
            .iter()
            .map(|unit| unit.text.chars().count() as u64)
            .sum();
        let state = if chars == 0 { "empty" } else { "done" };
        let stored = units
            .iter()
            .filter(|unit| !unit.text.trim().is_empty())
            .count() as i64;
        self.record_fulltext(blake3, state, stored, chars as i64, ocr_lines as i64, None)
    }

    /// The file could not be read into the index; it is not tried again until another version.
    pub fn fulltext_failed(&self, blake3: &Hash, error: &str) -> Result<()> {
        let error: String = error.chars().take(2000).collect();
        self.record_fulltext(blake3, "failed", 0, 0, 0, Some(error))
    }

    fn record_fulltext(
        &self,
        blake3: &Hash,
        state: &'static str,
        units: i64,
        chars: i64,
        ocr_lines: i64,
        error: Option<String>,
    ) -> Result<()> {
        let Some(cache) = self.cache() else {
            return Ok(());
        };
        let blake3 = *blake3;
        cache.write(move |transaction| {
            transaction
                .prepare_cached(
                    "INSERT OR REPLACE INTO fulltext_content (blake3, version, state, units, chars, ocr_lines, error, indexed_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                )?
                .execute(params![blake3.as_slice(), FULLTEXT_VERSION, state, units, chars, ocr_lines, error, now_ms()])?;
            Ok(())
        })
    }

    /// Takes out of the index the bytes no file of the library has any more.
    pub fn fulltext_forget_gone(&self) -> Result<usize> {
        let Some(cache) = self.cache() else {
            return Ok(0);
        };
        let present: HashSet<Hash> = self
            .hashed_book_files()?
            .into_iter()
            .filter_map(|file| file.blake3)
            .collect();
        let recorded: Vec<Hash> = cache.read(|connection| {
            Ok(connection
                .prepare_cached("SELECT blake3 FROM fulltext_content")?
                .query_map([], |row| row.get::<_, Vec<u8>>(0))?
                .filter_map(|row| {
                    row.ok()
                        .and_then(|hash| <[u8; 32]>::try_from(hash.as_slice()).ok())
                })
                .collect())
        })?;
        let gone: Vec<Hash> = recorded
            .into_iter()
            .filter(|hash| !present.contains(hash))
            .collect();
        if gone.is_empty() {
            return Ok(0);
        }
        self.fulltext_index()?.forget(&gone)?;
        let count = gone.len();
        cache.write(move |transaction| {
            for hash in &gone {
                transaction
                    .prepare_cached("DELETE FROM fulltext_content WHERE blake3 = ?1")?
                    .execute([hash.as_slice()])?;
            }
            Ok(())
        })?;
        Ok(count)
    }

    /// How far the index is, with `pending` worked out from the library's files now.
    pub fn fulltext_status(&self) -> Result<FullTextStatus> {
        let Some(cache) = self.cache() else {
            return Ok(FullTextStatus::default());
        };
        let mut status = cache.read(|connection| {
            let mut status = FullTextStatus::default();
            let mut statement = connection.prepare_cached(
                "SELECT state, count(*), sum(chars) FROM fulltext_content WHERE version = ?1 GROUP BY state",
            )?;
            let rows = statement.query_map([FULLTEXT_VERSION], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, Option<i64>>(2)?))
            })?;
            for row in rows {
                let (state, count, chars) = row?;
                let count = count.max(0) as u64;
                match state.as_str() {
                    "done" => status.indexed = count,
                    "empty" => status.empty = count,
                    _ => status.failed = count,
                }
                status.chars += chars.unwrap_or(0).max(0) as u64;
            }
            Ok(status)
        })?;
        status.pending = self.fulltext_pending(usize::MAX)?.len() as u64;
        status.units = self.fulltext().map_or(0, FullText::units);
        Ok(status)
    }

    /// The lines OCR read on a scanned file's pages, each page's as one text, by page.
    pub fn ocr_page_texts(&self, blake3: &Hash) -> Result<BTreeMap<u32, String>> {
        let Some(cache) = self.cache() else {
            return Ok(BTreeMap::new());
        };
        let blake3 = *blake3;
        cache.read(move |connection| {
            let mut pages: BTreeMap<u32, String> = BTreeMap::new();
            let mut statement = connection.prepare_cached(
                "SELECT page, words FROM ocr_line WHERE blake3 = ?1 ORDER BY page, line",
            )?;
            let rows = statement.query_map([blake3.as_slice()], |row| {
                Ok((row.get::<_, u32>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (page, words) = row?;
                let Ok(serde_json::Value::Array(words)) =
                    serde_json::from_str::<serde_json::Value>(&words)
                else {
                    continue;
                };
                let line: Vec<&str> = words
                    .iter()
                    .filter_map(|word| word.get("text").and_then(|text| text.as_str()))
                    .collect();
                if line.is_empty() {
                    continue;
                }
                let text = pages.entry(page).or_default();
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&line.join(" "));
            }
            Ok(pages)
        })
    }

    /// Finds the books whose text has every word and phrase of `query`, best first, each with
    /// the pages or sections where it does.
    pub fn fulltext_search(&self, query: &str, books: usize) -> Result<FullTextResults> {
        let started = Instant::now();
        let asked = parse_query(query);
        if asked.is_empty() {
            return Ok(FullTextResults {
                terms: Vec::new(),
                books: Vec::new(),
                units: 0,
                elapsed_ms: 0.0,
            });
        }
        let index = self.fulltext_index()?;
        let (found, units) = index.search(&asked, MAX_UNITS)?;
        // By book, in the order of each one's best unit.
        let mut order: Vec<String> = Vec::new();
        let mut by_book: HashMap<String, Vec<(f32, StoredUnit)>> = HashMap::new();
        for (score, unit) in found {
            let units = by_book.entry(unit.content.clone()).or_insert_with(|| {
                order.push(unit.content.clone());
                Vec::new()
            });
            if units.len() < UNITS_PER_BOOK {
                units.push((score, unit));
            }
        }
        let books_limit = books.clamp(1, MAX_BOOKS_FOUND);
        let mut hits = Vec::new();
        for content in order {
            if hits.len() >= books_limit {
                break;
            }
            let Some(blake3) = hash_of_hex(&content) else {
                continue;
            };
            // Only bytes a file of the library has now: an entry left by a file since changed is not
            // shown.
            let owners: Vec<(DocumentId, WorkId)> = self.database().read(move |connection| {
                Ok(connection
                    .prepare_cached(
                        "SELECT DISTINCT r.document_id, d.work_id FROM revision r
                         JOIN file_instance i ON i.revision_id = r.id AND i.state <> 'absent'
                         JOIN document d ON d.id = r.document_id
                         WHERE r.blake3 = ?1 ORDER BY r.document_id",
                    )?
                    .query_map([blake3.as_slice()], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<rusqlite::Result<_>>()?)
            })?;
            if owners.is_empty() {
                continue;
            }
            let units = by_book.remove(&content).unwrap_or_default();
            let score = units.first().map_or(0.0, |(score, _)| *score);
            let mut unit_hits: Vec<UnitHit> = units
                .into_iter()
                .filter_map(|(score, unit)| {
                    let (snippet, marks, position, matched) = snippet(&unit.text, &asked)?;
                    Some(UnitHit {
                        kind: unit.kind,
                        index: unit.index,
                        label: unit.label,
                        source: unit.source,
                        score,
                        snippet,
                        marks,
                        position,
                        matched,
                    })
                })
                .collect();
            if unit_hits.is_empty() {
                continue;
            }
            // Within a book, in the book's order: the page or section where it comes first, first.
            unit_hits.sort_by_key(|hit| hit.index);
            let mut works: Vec<WorkId> = owners.iter().map(|(_, work)| *work).collect();
            works.dedup();
            hits.push(BookHit {
                blake3: content,
                score,
                documents: owners.iter().map(|(document, _)| *document).collect(),
                works,
                units: unit_hits,
            });
        }
        Ok(FullTextResults {
            terms: asked.shown(),
            books: hits,
            units,
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        })
    }
}

/// The folder the index of a library at these paths is in, for tools that measure it.
pub fn fulltext_dir(cache_dir: &Path) -> PathBuf {
    index_dir(cache_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_is_words_and_phrases_folded() {
        let asked = parse_query(r#"Memória  "VIDA maravilhosa" pré-história x"#);
        assert_eq!(asked.words, ["memoria", "x"]);
        assert_eq!(
            asked.phrases,
            [vec!["vida", "maravilhosa"], vec!["pre", "historia"]]
        );
        assert!(parse_query("  \"  \" ").is_empty());
        assert_eq!(parse_query("a a A").words, ["a"]);
        let long = "palavra ".repeat(100);
        assert!(parse_query(&long).words.len() <= MAX_QUERY_TERMS);
    }

    #[test]
    fn a_snippet_shows_the_match_and_marks_it() {
        let text = "Antes de tudo, a memória.\nNo meio do caminho tinha uma pedra, tinha uma pedra no meio do caminho.";
        let (shown, marks, position, matched) = snippet(text, &parse_query("pedra")).unwrap();
        assert!(shown.contains("tinha uma pedra"));
        let utf16: Vec<u16> = shown.encode_utf16().collect();
        let first = marks[0];
        assert_eq!(
            String::from_utf16(&utf16[first[0] as usize..first[1] as usize]).unwrap(),
            "pedra"
        );
        assert_eq!(marks.len(), 2, "both times it is there");
        let whole: Vec<u16> = text.encode_utf16().collect();
        assert_eq!(
            String::from_utf16(&whole[position as usize..position as usize + 5]).unwrap(),
            "pedra"
        );
        assert_eq!(matched, "pedra");
        // Without accents, and a phrase marked as one.
        let (shown, marks, _, matched) =
            snippet(text, &parse_query("\"MEIO do caminho\" memoria")).unwrap();
        let utf16: Vec<u16> = shown.encode_utf16().collect();
        let marked: Vec<String> = marks
            .iter()
            .map(|[from, to]| String::from_utf16(&utf16[*from as usize..*to as usize]).unwrap())
            .collect();
        assert!(marked.contains(&"memória".to_owned()), "{marked:?}");
        assert!(marked.contains(&"meio do caminho".to_owned()), "{marked:?}");
        assert_eq!(
            matched, "meio",
            "the phrase is where the snippet starts from"
        );
    }

    #[test]
    fn a_long_text_is_cut_around_the_match() {
        let text = format!("{} agulha {}", "palha ".repeat(200), "feno ".repeat(200));
        let (shown, marks, position, _) = snippet(&text, &parse_query("agulha")).unwrap();
        assert!(shown.starts_with('…') && shown.ends_with('…'));
        assert!(shown.chars().count() < BEFORE + AFTER + 20);
        assert_eq!(marks.len(), 1);
        assert_eq!(position as usize, 200 * 6 + 1);
    }

    #[test]
    fn the_index_finds_words_phrases_and_forgets() {
        let dir = std::env::temp_dir().join(format!(
            "musaeum-fulltext-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let result = std::panic::catch_unwind(|| {
            let index = FullText::open(&dir).unwrap();
            let one = [1u8; 32];
            let two = [2u8; 32];
            let unit = |index: u32, text: &str| TextUnit {
                kind: UnitKind::Page,
                index,
                label: None,
                source: TextSource::TextLayer,
                text: text.to_owned(),
            };
            index
                .store(
                    &one,
                    &[
                        unit(0, "O burgess shale e seus fósseis."),
                        unit(3, "Wiwaxia, Hallucigenia e Anomalocaris."),
                    ],
                )
                .unwrap();
            index
                .store(&two, &[unit(0, "Um livro sobre a Hallucigenia, outro.")])
                .unwrap();
            let (found, count) = index.search(&parse_query("hallucigenia"), 10).unwrap();
            assert_eq!(count, 2);
            assert_eq!(found.len(), 2);
            let (found, _) = index
                .search(&parse_query("\"wiwaxia hallucigenia\""), 10)
                .unwrap();
            assert_eq!(found.len(), 1);
            assert_eq!(found[0].1.index, 3);
            let (found, _) = index.search(&parse_query("fosseis burgess"), 10).unwrap();
            assert_eq!(found.len(), 1, "every word, accents aside");
            // Stored again, the old units are replaced, not doubled.
            index
                .store(&one, &[unit(5, "Só Hallucigenia agora.")])
                .unwrap();
            let (found, _) = index.search(&parse_query("hallucigenia"), 10).unwrap();
            assert_eq!(found.len(), 2);
            index.forget(&[two]).unwrap();
            let (found, _) = index.search(&parse_query("hallucigenia"), 10).unwrap();
            assert_eq!(found.len(), 1);
            assert_eq!(found[0].1.index, 5);
            drop(index);
        });
        // Windows keeps a mapped file busy a moment after it is let go.
        for _ in 0..50 {
            if fs::remove_dir_all(&dir).is_ok() || !dir.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(
            !dir.exists(),
            "the index's folder is left in the temporary folder"
        );
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }
}

//! Annotations out of the library and back in as W3C Web Annotations, in JSON-LD (the Web
//! Annotation Data Model, 2017): one work's, several works', or the whole library's, without losing
//! anything the library keeps of them.
//!
//! The file is an `AnnotationCollection` with one `AnnotationPage`. Each annotation is an
//! `Annotation` with its UUIDv7 as `urn:uuid:…`, its dates, its note as a `TextualBody` in
//! `text/markdown`, and one `SpecificResource` target per segment. A target's source is the exact
//! file, `urn:blake3:…` of the revision its selectors count in, described with what identifies its
//! work (title, people, identifiers). Its selectors are W3C's where they say exactly what the
//! library keeps (`TextQuoteSelector`, `TextPositionSelector` in UTF-16 code units, a
//! `RangeSelector` of two EPUB CFI `FragmentSelector`s), and the library's own otherwise, with the
//! data as it is stored (`musaeum:PageCharRangeSelector`, `musaeum:PageQuadsSelector`,
//! `musaeum:PageRegionSelector`); a page of a fixed layout also gets a PDF `FragmentSelector`
//! (`page=N`), for other readers. What the model has no word for is in the vocabulary
//! `urn:musaeum:annotation:`, a name that is never looked up.
//!
//! Reading one is strict: at most [`MAX_JSONLD_BYTES`], nested at most [`MAX_DEPTH`] deep, every
//! object with only the properties this version writes, each once, `@context` exactly the one it
//! writes (never fetched: there is no network here), and every value checked before anything is
//! written. An annotation the library has already, by its UUID, deleted or not, is left as it is,
//! so the same file brought in twice adds nothing. Each comes back to the document with the same
//! bytes when there is one; otherwise to a document of the same work (an identifier in common, or
//! the same title by the same first author), where it is found again by the anchoring cascade the
//! next time the document is read, exact, approximate or an orphan, and never lost.

use musaeum_i18n::tr;
use std::collections::{BTreeMap, HashMap, HashSet};

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::annotations::{
    self, AnchorState, Annotation, AnnotationKind, NewAnnotation, QuoteSource, Segment, Selector,
    SelectorKind,
};
use crate::error::{Error, Result};
use crate::library::Library;
use crate::model::{DocumentId, RevisionId, WorkId};
use crate::sql::fold;

/// The W3C context, named and never fetched.
pub const W3C_CONTEXT: &str = "http://www.w3.org/ns/anno.jsonld";
/// The library's own words: a name, never fetched either.
pub const VOCABULARY: &str = "urn:musaeum:annotation:";
/// The version of what is written, in `musaeum:formatVersion`.
pub const FORMAT_VERSION: u32 = 1;
/// The largest file read.
pub const MAX_JSONLD_BYTES: u64 = 256 << 20;
/// How deep objects and arrays may nest.
pub const MAX_DEPTH: usize = 16;
/// The most annotations one file may hold.
pub const MAX_EXCHANGED: usize = 500_000;
/// The most selectors one target may have.
const MAX_SELECTORS: usize = 16;
/// The longest title, name or identifier read.
const MAX_LABEL: usize = 4096;

const CFI: &str = "http://www.idpf.org/epub/linking/cfi/epub-cfi.html";
const PDF_FRAGMENT: &str = "http://tools.ietf.org/rfc/rfc3778";

fn refused(message: impl Into<String>) -> Error {
    Error::Refused(message.into())
}

/// The `@context` every file has: W3C's, and the library's words, with what is plain JSON in them.
pub fn context() -> Value {
    json!([
        W3C_CONTEXT,
        {
            "@version": 1.1,
            "musaeum": VOCABULARY,
            "musaeum:value": {"@type": "@json"},
            "musaeum:clip": {"@type": "@json"},
            "musaeum:creators": {"@type": "@json"},
            "musaeum:identifiers": {"@type": "@json"}
        }
    ])
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Collection {
    #[serde(rename = "@context")]
    context: Value,
    id: String,
    #[serde(rename = "type")]
    kind: String,
    label: String,
    total: usize,
    generator: Generator,
    generated: String,
    #[serde(rename = "musaeum:formatVersion")]
    format_version: u32,
    #[serde(rename = "musaeum:scope")]
    scope: String,
    first: Page,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Generator {
    #[serde(rename = "type")]
    kind: String,
    name: String,
    #[serde(rename = "musaeum:version")]
    version: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    #[serde(rename = "type")]
    kind: String,
    #[serde(rename = "startIndex")]
    start_index: usize,
    items: Vec<JsonAnnotation>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JsonAnnotation {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    motivation: String,
    created: String,
    modified: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    body: Option<Body>,
    target: Vec<Target>,
    #[serde(rename = "musaeum:kind")]
    mark: AnnotationKind,
    #[serde(rename = "musaeum:color")]
    color: String,
    #[serde(
        rename = "musaeum:quote",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    quote: Option<String>,
    #[serde(rename = "musaeum:quoteSource")]
    quote_source: QuoteSource,
    #[serde(
        rename = "musaeum:progression",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    progression: Option<f64>,
    #[serde(rename = "musaeum:anchorState")]
    anchor_state: AnchorState,
    #[serde(
        rename = "musaeum:revision",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    revision: Option<String>,
    #[serde(
        rename = "musaeum:resolvedRevision",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    resolved_revision: Option<String>,
    #[serde(
        rename = "musaeum:resolvedVersion",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    resolved_version: Option<u32>,
    #[serde(
        rename = "musaeum:pdfName",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pdf_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    #[serde(rename = "type")]
    kind: String,
    value: String,
    format: String,
    purpose: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    #[serde(rename = "type")]
    kind: String,
    source: Source,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    selector: Vec<JsonSelector>,
    #[serde(
        rename = "musaeum:ordinal",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    ordinal: Option<u32>,
    #[serde(
        rename = "musaeum:page",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    page: Option<u32>,
    #[serde(
        rename = "musaeum:href",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    href: Option<String>,
    #[serde(
        rename = "musaeum:clip",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    clip: Option<[f64; 4]>,
    #[serde(
        rename = "musaeum:anchorState",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    anchor_state: Option<AnchorState>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Person {
    name: String,
    role: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Code {
    scheme: String,
    value: String,
}

/// The file an annotation is on, and its work.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(rename = "type")]
    kind: String,
    format: String,
    #[serde(rename = "dcterms:title")]
    title: String,
    #[serde(
        rename = "dcterms:creator",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    creator_names: Vec<String>,
    #[serde(
        rename = "musaeum:creators",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    creators: Vec<Person>,
    #[serde(rename = "musaeum:layout")]
    layout: String,
    #[serde(rename = "musaeum:format")]
    file_format: String,
    #[serde(
        rename = "musaeum:series",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    series: Option<String>,
    #[serde(
        rename = "musaeum:seriesPosition",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    series_position: Option<f64>,
    #[serde(
        rename = "musaeum:year",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    year: Option<i64>,
    #[serde(
        rename = "musaeum:language",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    language: Option<String>,
    #[serde(
        rename = "musaeum:identifiers",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    identifiers: Vec<Code>,
    #[serde(
        rename = "musaeum:pdfId0",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pdf_id0: Option<String>,
    #[serde(
        rename = "musaeum:dcIdentifier",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    dc_identifier: Option<String>,
    #[serde(
        rename = "musaeum:isbn",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    isbn: Option<String>,
}

/// Every selector there is, as one shape: which properties a type may have is checked after.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JsonSelector {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    exact: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    suffix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    start: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    end: Option<u64>,
    #[serde(
        rename = "startSelector",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    start_selector: Option<Box<JsonSelector>>,
    #[serde(
        rename = "endSelector",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    end_selector: Option<Box<JsonSelector>>,
    #[serde(
        rename = "conformsTo",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    conforms_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    value: Option<String>,
    #[serde(
        rename = "musaeum:unit",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    unit: Option<String>,
    #[serde(
        rename = "musaeum:value",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    data: Option<Value>,
    #[serde(
        rename = "musaeum:priority",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    priority: Option<i64>,
    #[serde(
        rename = "musaeum:version",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    version: Option<u32>,
    #[serde(
        rename = "musaeum:revision",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    revision: Option<String>,
}

/// The library's own selector types, by kind.
fn own_type(kind: SelectorKind) -> &'static str {
    match kind {
        SelectorKind::Cfi => "musaeum:CfiSelector",
        SelectorKind::TextPosition => "musaeum:TextPositionSelector",
        SelectorKind::TextQuote => "musaeum:TextQuoteSelector",
        SelectorKind::PageCharRange => "musaeum:PageCharRangeSelector",
        SelectorKind::PageQuads => "musaeum:PageQuadsSelector",
        SelectorKind::PageRegion => "musaeum:PageRegionSelector",
    }
}

fn kind_of_own(name: &str) -> Option<SelectorKind> {
    [
        SelectorKind::Cfi,
        SelectorKind::TextPosition,
        SelectorKind::TextQuote,
        SelectorKind::PageCharRange,
        SelectorKind::PageQuads,
        SelectorKind::PageRegion,
    ]
    .into_iter()
    .find(|kind| own_type(*kind) == name)
}

/// A stored object with exactly the string keys given (some of them may be missing, `required`
/// may not).
fn strings_of<'a>(
    data: &'a Value,
    keys: &[&str],
    required: &[&str],
) -> Option<BTreeMap<&'a str, &'a str>> {
    let object = data.as_object()?;
    let mut found = BTreeMap::new();
    for (key, value) in object {
        if !keys.contains(&key.as_str()) {
            return None;
        }
        found.insert(key.as_str(), value.as_str()?);
    }
    required
        .iter()
        .all(|key| found.contains_key(key))
        .then_some(found)
}

fn numbers_of(data: &Value) -> Option<(u64, u64)> {
    let object = data.as_object()?;
    if object.len() != 2 {
        return None;
    }
    Some((object.get("start")?.as_u64()?, object.get("end")?.as_u64()?))
}

fn revision_iri(hash: &[u8]) -> String {
    let mut iri = String::from("urn:blake3:");
    for byte in hash {
        iri.push_str(&format!("{byte:02x}"));
    }
    iri
}

fn hash_of_iri(iri: &str) -> Result<[u8; 32]> {
    let hex = iri
        .strip_prefix("urn:blake3:")
        .filter(|hex| {
            hex.len() == 64
                && hex
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
        .ok_or_else(|| refused(tr!(NotABlake3Name, iri = format!("{:?}", iri))))?;
    let mut hash = [0u8; 32];
    for (index, byte) in hash.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).unwrap_or(0);
    }
    Ok(hash)
}

/// A selector as the file writes it.
fn json_selector(selector: &Selector, revisions: &HashMap<RevisionId, String>) -> JsonSelector {
    let mut out = JsonSelector {
        priority: Some(selector.priority),
        version: selector.normalizer_version,
        revision: selector
            .revision_id
            .and_then(|id| revisions.get(&id).cloned()),
        ..JsonSelector::default()
    };
    let data = &selector.data;
    match selector.kind {
        SelectorKind::TextQuote => {
            if let Some(quote) = strings_of(data, &["exact", "prefix", "suffix"], &["exact"]) {
                out.kind = "TextQuoteSelector".to_owned();
                out.exact = quote.get("exact").map(|text| (*text).to_owned());
                out.prefix = quote.get("prefix").map(|text| (*text).to_owned());
                out.suffix = quote.get("suffix").map(|text| (*text).to_owned());
                return out;
            }
        }
        SelectorKind::TextPosition => {
            if let Some((start, end)) = numbers_of(data) {
                out.kind = "TextPositionSelector".to_owned();
                out.start = Some(start);
                out.end = Some(end);
                out.unit = Some("utf-16".to_owned());
                return out;
            }
        }
        SelectorKind::Cfi => {
            if let Some(range) = strings_of(data, &["start", "end"], &["start", "end"]) {
                let fragment = |value: &str| {
                    Box::new(JsonSelector {
                        kind: "FragmentSelector".to_owned(),
                        conforms_to: Some(CFI.to_owned()),
                        value: Some(value.to_owned()),
                        ..JsonSelector::default()
                    })
                };
                out.kind = "RangeSelector".to_owned();
                out.start_selector = Some(fragment(range["start"]));
                out.end_selector = Some(fragment(range["end"]));
                return out;
            }
        }
        _ => {}
    }
    out.kind = own_type(selector.kind).to_owned();
    out.data = Some(data.clone());
    out
}

/// What a selector of the file is, once checked: one of the library's, or the page fragment added
/// for other readers (`Ok(None)`).
fn read_selector(json: &JsonSelector) -> Result<Option<(Selector, Option<String>)>> {
    let bad = |why: String| refused(tr!(SelectorBad, kind = json.kind, why = why));
    let only = |allowed: &[&str]| -> Result<()> {
        let present = [
            ("exact", json.exact.is_some()),
            ("prefix", json.prefix.is_some()),
            ("suffix", json.suffix.is_some()),
            ("start", json.start.is_some()),
            ("end", json.end.is_some()),
            ("startSelector", json.start_selector.is_some()),
            ("endSelector", json.end_selector.is_some()),
            ("conformsTo", json.conforms_to.is_some()),
            ("value", json.value.is_some()),
            ("musaeum:unit", json.unit.is_some()),
            ("musaeum:value", json.data.is_some()),
            ("musaeum:priority", json.priority.is_some()),
            ("musaeum:version", json.version.is_some()),
            ("musaeum:revision", json.revision.is_some()),
        ];
        match present
            .iter()
            .find(|(name, here)| *here && !allowed.contains(name))
        {
            Some((name, _)) => Err(bad(tr!(SelectorTakesNo, name = name))),
            None => Ok(()),
        }
    };
    const PLACE: [&str; 3] = ["musaeum:priority", "musaeum:version", "musaeum:revision"];
    fn with_place(extra: &[&'static str]) -> Vec<&'static str> {
        extra.iter().chain(PLACE.iter()).copied().collect()
    }
    let (kind, data) = match json.kind.as_str() {
        "FragmentSelector" if json.priority.is_none() => {
            only(&["conformsTo", "value"])?;
            if json.conforms_to.as_deref() != Some(PDF_FRAGMENT)
                || !json.value.as_deref().is_some_and(|value| {
                    value.strip_prefix("page=").is_some_and(|n| {
                        !n.is_empty() && n.len() <= 10 && n.bytes().all(|b| b.is_ascii_digit())
                    })
                })
            {
                return Err(bad(tr!(SelectorPdfPageOnly)));
            }
            return Ok(None);
        }
        "TextQuoteSelector" => {
            only(&with_place(&["exact", "prefix", "suffix"]))?;
            let mut data = serde_json::Map::new();
            data.insert(
                "exact".into(),
                Value::String(
                    json.exact
                        .clone()
                        .ok_or_else(|| bad(tr!(SelectorWithout, name = "exact")))?,
                ),
            );
            if let Some(prefix) = &json.prefix {
                data.insert("prefix".into(), Value::String(prefix.clone()));
            }
            if let Some(suffix) = &json.suffix {
                data.insert("suffix".into(), Value::String(suffix.clone()));
            }
            (SelectorKind::TextQuote, Value::Object(data))
        }
        "TextPositionSelector" => {
            only(&with_place(&["start", "end", "musaeum:unit"]))?;
            if json.unit.as_deref() != Some("utf-16") {
                return Err(bad(tr!(SelectorUtf16)));
            }
            let (Some(start), Some(end)) = (json.start, json.end) else {
                return Err(bad(tr!(SelectorNoStartEnd)));
            };
            (
                SelectorKind::TextPosition,
                json!({"start": start, "end": end}),
            )
        }
        "RangeSelector" => {
            only(&with_place(&["startSelector", "endSelector"]))?;
            let point = |part: &Option<Box<JsonSelector>>| -> Result<String> {
                let part = part.as_deref().ok_or_else(|| bad(tr!(SelectorNoEnds)))?;
                let fine = part.kind == "FragmentSelector"
                    && part.conforms_to.as_deref() == Some(CFI)
                    && part.value.is_some()
                    && JsonSelector {
                        kind: part.kind.clone(),
                        conforms_to: part.conforms_to.clone(),
                        value: part.value.clone(),
                        ..JsonSelector::default()
                    } == *part;
                if !fine {
                    return Err(bad(tr!(SelectorNotCfi)));
                }
                Ok(part.value.clone().unwrap_or_default())
            };
            let start = point(&json.start_selector)?;
            let end = point(&json.end_selector)?;
            (SelectorKind::Cfi, json!({"start": start, "end": end}))
        }
        name => {
            let kind = kind_of_own(name)
                .ok_or_else(|| refused(tr!(SelectorUnknownKind, name = format!("{:?}", name))))?;
            only(&with_place(&["musaeum:value"]))?;
            let data = json
                .data
                .clone()
                .ok_or_else(|| bad(tr!(SelectorWithout, name = "musaeum:value")))?;
            if depth_of(&data) > 8 {
                return Err(bad(tr!(SelectorTooDeep)));
            }
            (kind, data)
        }
    };
    let priority = json
        .priority
        .ok_or_else(|| bad(tr!(SelectorWithout, name = "musaeum:priority")))?;
    if !(-1000..=1000).contains(&priority) {
        return Err(bad(tr!(SelectorPriority)));
    }
    Ok(Some((
        Selector {
            kind,
            data,
            priority,
            normalizer_version: json.version,
            revision_id: None,
        },
        json.revision.clone(),
    )))
}

fn depth_of(value: &Value) -> usize {
    match value {
        Value::Array(items) => 1 + items.iter().map(depth_of).max().unwrap_or(0),
        Value::Object(map) => 1 + map.values().map(depth_of).max().unwrap_or(0),
        _ => 0,
    }
}

/// How deep the file nests, read from its bytes before anything is built from them.
fn nesting(bytes: &[u8]) -> usize {
    let mut deepest = 0;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for &byte in bytes {
        if in_string {
            match (escaped, byte) {
                (true, _) => escaped = false,
                (false, b'\\') => escaped = true,
                (false, b'"') => in_string = false,
                _ => {}
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'[' | b'{' => {
                depth += 1;
                deepest = deepest.max(depth);
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    deepest
}

/// `2026-11-23T20:40:05.123Z`.
fn date_text(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    let of_day = ms.rem_euclid(86_400_000);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        of_day / 3_600_000,
        of_day / 60_000 % 60,
        of_day / 1000 % 60,
        of_day % 1000
    )
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let of_era = year - era * 400;
    let of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    era * 146_097 + of_era * 365 + of_era / 4 - of_era / 100 + of_year - 719_468
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let of_era = z - era * 146_097;
    let year_of_era = (of_era - of_era / 1460 + of_era / 36_524 - of_era / 146_096) / 365;
    let of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted = (5 * of_year + 2) / 153;
    let day = of_year - (153 * shifted + 2) / 5 + 1;
    let month = if shifted < 10 {
        shifted + 3
    } else {
        shifted - 9
    };
    (year_of_era + era * 400 + i64::from(month <= 2), month, day)
}

/// An `xsd:dateTime`: `YYYY-MM-DDTHH:MM:SS`, a fraction of a second if any, and `Z` or an offset.
fn parse_date(text: &str) -> Result<i64> {
    let bad = || refused(tr!(NotADate, text = format!("{:?}", text)));
    let bytes = text.as_bytes();
    if bytes.len() < 20 || bytes.len() > 40 {
        return Err(bad());
    }
    let digits = |from: usize, to: usize| -> Result<i64> {
        let part = text.get(from..to).ok_or_else(bad)?;
        if !part.bytes().all(|b| b.is_ascii_digit()) {
            return Err(bad());
        }
        part.parse().map_err(|_| bad())
    };
    if &bytes[4..5] != b"-"
        || &bytes[7..8] != b"-"
        || &bytes[10..11] != b"T"
        || &bytes[13..14] != b":"
        || &bytes[16..17] != b":"
    {
        return Err(bad());
    }
    let (year, month, day) = (digits(0, 4)?, digits(5, 7)?, digits(8, 10)?);
    let (hour, minute, second) = (digits(11, 13)?, digits(14, 16)?, digits(17, 19)?);
    let month_days = [
        31,
        if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) {
            29
        } else {
            28
        },
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
    if !(1..=12).contains(&month)
        || day < 1
        || day > month_days[(month - 1) as usize]
        || hour > 23
        || minute > 59
        || second > 59
    {
        return Err(bad());
    }
    let mut at = 19;
    let mut millis = 0;
    if bytes.get(at) == Some(&b'.') {
        let start = at + 1;
        let mut end = start;
        while bytes.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
        }
        if end == start || end - start > 9 {
            return Err(bad());
        }
        let fraction = &text[start..end];
        millis = format!("{fraction:0<3}")[..3]
            .parse::<i64>()
            .map_err(|_| bad())?;
        at = end;
    }
    let offset = match &text[at..] {
        "Z" => 0,
        zone if zone.len() == 6
            && (zone.starts_with('+') || zone.starts_with('-'))
            && &zone[3..4] == ":" =>
        {
            let hours = digits(at + 1, at + 3)?;
            let minutes = digits(at + 4, at + 6)?;
            if hours > 14 || minutes > 59 {
                return Err(bad());
            }
            let minutes = hours * 60 + minutes;
            if zone.starts_with('-') {
                -minutes
            } else {
                minutes
            }
        }
        _ => return Err(bad()),
    };
    Ok(days_from_civil(year, month, day) * 86_400_000
        + hour * 3_600_000
        + minute * 60_000
        + second * 1000
        + millis
        - offset * 60_000)
}

/// The media type of a file by its extension.
fn media_type(format: &str) -> &'static str {
    match format.to_ascii_lowercase().as_str() {
        "pdf" => "application/pdf",
        "djvu" | "djv" => "image/vnd.djvu",
        "cbz" => "application/vnd.comicbook+zip",
        "cbr" => "application/vnd.comicbook-rar",
        "epub" => "application/epub+zip",
        "mobi" | "prc" => "application/x-mobipocket-ebook",
        "azw3" | "azw" | "kf8" => "application/vnd.amazon.ebook",
        "fb2" => "application/x-fictionbook+xml",
        "txt" => "text/plain",
        "md" | "markdown" => "text/markdown",
        _ => "application/octet-stream",
    }
}

/// What part of the library to write.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "ids")]
pub enum ExchangeScope {
    /// One document's annotations.
    Document(DocumentId),
    /// The annotations of every document of these works.
    Works(Vec<WorkId>),
    /// Every annotated document's.
    Library,
}

impl ExchangeScope {
    fn name(&self) -> &'static str {
        match self {
            ExchangeScope::Document(_) => "document",
            ExchangeScope::Works(_) => "works",
            ExchangeScope::Library => "library",
        }
    }
}

/// What an export wrote.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExchangeExport {
    pub json: String,
    pub annotations: usize,
    pub documents: usize,
    pub works: usize,
}

/// Where an import may put what it brings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "id")]
pub enum ExchangeTarget {
    /// Only into this document.
    Document(DocumentId),
    /// Only into this work's documents.
    Work(WorkId),
    /// Anywhere in the library.
    Library,
}

/// What an import brought into one document.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedInto {
    pub document_id: DocumentId,
    pub title: String,
    pub added: Vec<Annotation>,
    /// Whether they came from other bytes, to be found again on these.
    pub needs_anchoring: bool,
}

/// A work of the file the library does not have.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnmatchedWork {
    pub title: String,
    pub creators: Vec<String>,
    pub annotations: usize,
}

/// What an import did.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExchangeImport {
    pub into: Vec<ImportedInto>,
    /// Already in the library, by their UUID or their PDF name.
    pub already: usize,
    /// Of works the library does not have.
    pub unmatched: Vec<UnmatchedWork>,
    /// Of other books than the one asked for.
    pub elsewhere: usize,
    /// Brought into the book asked for though the file names another work: the person chose it.
    pub other_work: usize,
    pub total: usize,
}

struct DocumentFacts {
    work: WorkId,
    layout: String,
    format: String,
    pdf_id0: Option<String>,
    dc_identifier: Option<String>,
    isbn: Option<String>,
}

fn document_facts(connection: &Connection, document: DocumentId) -> Result<DocumentFacts> {
    connection
        .prepare_cached("SELECT work_id, layout, format, pdf_id0, dc_identifier, isbn FROM document WHERE id = ?1")?
        .query_row([document], |row| {
            Ok(DocumentFacts {
                work: row.get(0)?,
                layout: row.get(1)?,
                format: row.get(2)?,
                pdf_id0: row.get(3)?,
                dc_identifier: row.get(4)?,
                isbn: row.get(5)?,
            })
        })
        .optional()?
        .ok_or_else(|| Error::NotFound(tr!(NoDocument, id = document)))
}

fn source_of(connection: &Connection, facts: &DocumentFacts) -> Result<Source> {
    let work = crate::metadata::work_metadata(connection, facts.work)?;
    Ok(Source {
        id: None,
        kind: "Text".to_owned(),
        format: media_type(&facts.format).to_owned(),
        title: work.title.clone(),
        creator_names: work
            .creators
            .iter()
            .map(|credit| credit.name.clone())
            .collect(),
        creators: work
            .creators
            .iter()
            .map(|credit| Person {
                name: credit.name.clone(),
                role: credit.role.clone(),
            })
            .collect(),
        layout: facts.layout.clone(),
        file_format: facts.format.clone(),
        series: work.series.clone(),
        series_position: work.series_position,
        year: work.year,
        language: work.language.clone(),
        identifiers: work
            .identifiers
            .iter()
            .map(|id| Code {
                scheme: id.scheme.clone(),
                value: id.value.clone(),
            })
            .collect(),
        pdf_id0: facts.pdf_id0.clone(),
        dc_identifier: facts.dc_identifier.clone(),
        isbn: facts.isbn.clone(),
    })
}

fn revisions_of(
    connection: &Connection,
    document: DocumentId,
) -> Result<HashMap<RevisionId, String>> {
    let rows = connection
        .prepare_cached(
            "SELECT id, blake3 FROM revision WHERE document_id = ?1 AND blake3 IS NOT NULL",
        )?
        .query_map([document], |row| {
            Ok((row.get::<_, RevisionId>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows
        .into_iter()
        .map(|(id, hash)| (id, revision_iri(&hash)))
        .collect())
}

fn json_annotation(
    annotation: &Annotation,
    source: &Source,
    revisions: &HashMap<RevisionId, String>,
) -> JsonAnnotation {
    let iri = |id: Option<RevisionId>| id.and_then(|id| revisions.get(&id).cloned());
    let resolved = iri(annotation
        .resolved_revision_id
        .or(Some(annotation.revision_id)));
    let on = Source {
        id: resolved.clone(),
        ..source.clone()
    };
    let target = if annotation.segments.is_empty() {
        vec![Target {
            kind: "SpecificResource".to_owned(),
            source: on,
            selector: Vec::new(),
            ordinal: None,
            page: None,
            href: None,
            clip: None,
            anchor_state: None,
        }]
    } else {
        annotation
            .segments
            .iter()
            .map(|segment| {
                let mut selector: Vec<JsonSelector> = segment
                    .selectors
                    .iter()
                    .map(|selector| json_selector(selector, revisions))
                    .collect();
                if let Some(page) = segment.page {
                    selector.push(JsonSelector {
                        kind: "FragmentSelector".to_owned(),
                        conforms_to: Some(PDF_FRAGMENT.to_owned()),
                        value: Some(format!("page={}", u64::from(page) + 1)),
                        ..JsonSelector::default()
                    });
                }
                Target {
                    kind: "SpecificResource".to_owned(),
                    source: on.clone(),
                    selector,
                    ordinal: Some(segment.ordinal),
                    page: segment.page,
                    href: segment.href.clone(),
                    clip: segment.clip,
                    anchor_state: Some(segment.anchor_state),
                }
            })
            .collect()
    };
    JsonAnnotation {
        id: format!("urn:uuid:{}", annotation.uuid),
        kind: "Annotation".to_owned(),
        motivation: if annotation.kind == AnnotationKind::Note {
            "commenting"
        } else {
            "highlighting"
        }
        .to_owned(),
        created: date_text(annotation.created_at),
        modified: date_text(annotation.updated_at),
        body: (!annotation.note.is_empty()).then(|| Body {
            kind: "TextualBody".to_owned(),
            value: annotation.note.clone(),
            format: "text/markdown".to_owned(),
            purpose: "commenting".to_owned(),
        }),
        target,
        mark: annotation.kind,
        color: annotation.color.clone(),
        quote: annotation.quote.clone(),
        quote_source: annotation.quote_source,
        progression: annotation.progression,
        anchor_state: annotation.anchor_state,
        revision: iri(Some(annotation.revision_id)),
        resolved_revision: annotation
            .resolved_revision_id
            .and_then(|id| revisions.get(&id).cloned()),
        resolved_version: annotation.resolved_version,
        pdf_name: annotation.pdf_nm.clone(),
    }
}

/// An annotation of the file, checked, before it has a place in this library.
struct Incoming {
    uuid: [u8; 16],
    source: Source,
    created: i64,
    updated: i64,
    kind: AnnotationKind,
    color: String,
    note: String,
    quote: Option<String>,
    quote_source: QuoteSource,
    progression: Option<f64>,
    anchor_state: AnchorState,
    revision: Option<[u8; 32]>,
    resolved: Option<[u8; 32]>,
    resolved_version: Option<u32>,
    pdf_nm: Option<String>,
    /// Segments, their selectors' revisions by position.
    segments: Vec<(Segment, Vec<Option<[u8; 32]>>)>,
}

fn label_ok(text: &str) -> bool {
    text.len() <= MAX_LABEL
}

fn check_source(source: &Source) -> Result<()> {
    if source.kind != "Text" {
        return Err(refused(tr!(
            SourceNotText,
            kind = format!("{:?}", source.kind)
        )));
    }
    if !matches!(source.layout.as_str(), "fixed" | "flow") {
        return Err(refused(tr!(
            NotALayout,
            layout = format!("{:?}", source.layout)
        )));
    }
    let texts = [
        Some(&source.title),
        Some(&source.format),
        Some(&source.file_format),
    ]
    .into_iter()
    .flatten()
    .chain(source.creator_names.iter())
    .chain(source.series.iter())
    .chain(source.language.iter())
    .chain(source.pdf_id0.iter())
    .chain(source.dc_identifier.iter())
    .chain(source.isbn.iter());
    for text in texts {
        if !label_ok(text) {
            return Err(refused(tr!(WorkTextTooLong)));
        }
    }
    if source.creators.len() > 256
        || source.identifiers.len() > 256
        || source.creator_names.len() > 256
    {
        return Err(refused(tr!(WorkTooManyPeople)));
    }
    for person in &source.creators {
        if !label_ok(&person.name) || !label_ok(&person.role) {
            return Err(refused(tr!(PlainNameTooLong)));
        }
    }
    for code in &source.identifiers {
        if !label_ok(&code.scheme) || !label_ok(&code.value) {
            return Err(refused(tr!(IdentifierTooLong)));
        }
    }
    if let Some(id) = &source.id {
        hash_of_iri(id)?;
    }
    Ok(())
}

fn incoming_of(json: &JsonAnnotation) -> Result<Incoming> {
    if json.kind != "Annotation" {
        return Err(refused(tr!(
            ItemNotAnnotation,
            kind = format!("{:?}", json.kind)
        )));
    }
    let text = json
        .id
        .strip_prefix("urn:uuid:")
        .ok_or_else(|| refused(tr!(NotUrnUuid, id = format!("{:?}", json.id))))?;
    let uuid = annotations::uuid_bytes(text)?;
    let uuid_text = uuid::Uuid::from_bytes(uuid).hyphenated().to_string();
    let motivation = if json.mark == AnnotationKind::Note {
        "commenting"
    } else {
        "highlighting"
    };
    if json.motivation != motivation {
        return Err(refused(tr!(
            MotivationMismatch,
            uuid = uuid_text,
            said = format!("{:?}", json.motivation),
            mark = format!("{:?}", json.mark),
            motivation = motivation
        )));
    }
    let note = match &json.body {
        Some(body) => {
            if body.kind != "TextualBody"
                || body.format != "text/markdown"
                || body.purpose != "commenting"
                || body.value.is_empty()
            {
                return Err(refused(tr!(NoteNotMarkdown, uuid = uuid_text)));
            }
            body.value.clone()
        }
        None => String::new(),
    };
    if json.target.is_empty() || json.target.len() > annotations::MAX_SEGMENTS {
        return Err(refused(tr!(TargetCount, uuid = uuid_text)));
    }
    let first = &json.target[0].source;
    let whole = json.target.len() == 1 && json.target[0].ordinal.is_none();
    let mut segments = Vec::new();
    for target in &json.target {
        if target.kind != "SpecificResource" {
            return Err(refused(tr!(
                TargetKind,
                kind = format!("{:?}", target.kind)
            )));
        }
        check_source(&target.source)?;
        let same_work = Source {
            id: None,
            ..target.source.clone()
        } == Source {
            id: None,
            ..first.clone()
        };
        if !same_work {
            return Err(refused(tr!(TwoWorks, uuid = uuid_text)));
        }
        if whole {
            if !target.selector.is_empty()
                || target.page.is_some()
                || target.href.is_some()
                || target.clip.is_some()
                || target.anchor_state.is_some()
            {
                return Err(refused(tr!(WholeBookHasPlace, uuid = uuid_text)));
            }
            continue;
        }
        let ordinal = target
            .ordinal
            .ok_or_else(|| refused(tr!(SegmentNoOrdinal, uuid = uuid_text)))?;
        if target.selector.len() > MAX_SELECTORS {
            return Err(refused(tr!(SegmentTooManySelectors)));
        }
        let mut selectors = Vec::new();
        let mut revisions = Vec::new();
        let mut page_fragment = None;
        for json_selector in &target.selector {
            match read_selector(json_selector)? {
                Some((selector, revision)) => {
                    revisions.push(revision.as_deref().map(hash_of_iri).transpose()?);
                    selectors.push(selector);
                }
                None => page_fragment = json_selector.value.clone(),
            }
        }
        if let Some(fragment) = page_fragment
            && Some(fragment)
                != target
                    .page
                    .map(|page| format!("page={}", u64::from(page) + 1))
        {
            return Err(refused(tr!(SegmentTwoPages, uuid = uuid_text)));
        }
        let segment = Segment {
            ordinal,
            page: target.page,
            href: target.href.clone(),
            clip: target.clip,
            anchor_state: target
                .anchor_state
                .ok_or_else(|| refused(tr!(SegmentNoState, uuid = uuid_text)))?,
            selectors,
        };
        segments.push((segment, revisions));
    }
    let hash = |iri: &Option<String>| iri.as_deref().map(hash_of_iri).transpose();
    if json.color.len() > 32 || json.pdf_name.as_ref().is_some_and(|name| name.len() > 256) {
        return Err(refused(tr!(ColorOrPdfName, uuid = uuid_text)));
    }
    if json.progression.is_some_and(|value| !value.is_finite()) {
        return Err(refused(tr!(PositionNotNumber)));
    }
    Ok(Incoming {
        uuid,
        source: first.clone(),
        created: parse_date(&json.created)?,
        updated: parse_date(&json.modified)?,
        kind: json.mark,
        color: json.color.clone(),
        note,
        quote: json.quote.clone(),
        quote_source: json.quote_source,
        progression: json.progression,
        anchor_state: json.anchor_state,
        revision: hash(&json.revision)?,
        resolved: hash(&json.resolved_revision)?,
        resolved_version: json.resolved_version,
        pdf_nm: json.pdf_name.clone(),
        segments,
    })
}

/// Reads a file of annotations, strictly, and checks every one of them.
fn read_file(bytes: &[u8]) -> Result<(Collection, Vec<Incoming>)> {
    if bytes.len() as u64 > MAX_JSONLD_BYTES {
        return Err(refused(tr!(FileTooLarge)));
    }
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    if nesting(bytes) > MAX_DEPTH {
        return Err(refused(tr!(FileTooDeep, max = MAX_DEPTH)));
    }
    let mut collection: Collection = serde_json::from_slice(bytes)
        .map_err(|error| refused(tr!(NotMusaeumJsonLd, error = error)))?;
    if collection.context != context() {
        return Err(refused(tr!(ContextNotOurs)));
    }
    if collection.kind != "AnnotationCollection"
        || collection.first.kind != "AnnotationPage"
        || collection.first.start_index != 0
    {
        return Err(refused(tr!(NotACollection)));
    }
    if collection.format_version != FORMAT_VERSION {
        return Err(refused(tr!(
            FormatVersion,
            found = collection.format_version,
            known = FORMAT_VERSION
        )));
    }
    if !matches!(collection.scope.as_str(), "document" | "works" | "library")
        || collection.generator.kind != "Software"
    {
        return Err(refused(tr!(FileSaysNothing)));
    }
    if collection.first.items.len() > MAX_EXCHANGED
        || collection.total != collection.first.items.len()
    {
        return Err(refused(tr!(CountMismatch)));
    }
    let items = std::mem::take(&mut collection.first.items);
    let incoming = items.iter().map(incoming_of).collect::<Result<Vec<_>>>()?;
    Ok((collection, incoming))
}

/// Where an annotation of the file goes, or why it does not.
#[derive(Clone, Debug)]
enum Placement {
    Into {
        document: DocumentId,
        other_work: bool,
    },
    Elsewhere,
    Unmatched,
}

fn revision_of_hash(
    connection: &Connection,
    hash: &[u8; 32],
) -> Result<Option<(RevisionId, DocumentId)>> {
    Ok(connection
        .prepare_cached("SELECT id, document_id FROM revision WHERE blake3 = ?1")?
        .query_row([hash.as_slice()], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?)
}

/// The works the library has that a source describes: by an identifier in common, or by the same
/// title and first author.
fn works_like(connection: &Connection, source: &Source) -> Result<Vec<WorkId>> {
    let mut works: Vec<WorkId> = Vec::new();
    let push = |work: WorkId, works: &mut Vec<WorkId>| {
        if !works.contains(&work) {
            works.push(work);
        }
    };
    for code in source
        .identifiers
        .iter()
        .filter(|code| code.scheme != "other")
    {
        let found = connection
            .prepare_cached("SELECT work_id FROM work_identifier WHERE scheme = ?1 AND value = ?2 ORDER BY work_id")?
            .query_map(params![code.scheme, code.value], |row| row.get::<_, WorkId>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for work in found {
            push(work, &mut works);
        }
    }
    for (column, value) in [
        ("isbn", &source.isbn),
        ("pdf_id0", &source.pdf_id0),
        ("dc_identifier", &source.dc_identifier),
    ] {
        let Some(value) = value.as_deref().filter(|value| !value.trim().is_empty()) else {
            continue;
        };
        let found = connection
            .prepare_cached(&format!(
                "SELECT DISTINCT work_id FROM document WHERE {column} = ?1 ORDER BY work_id"
            ))?
            .query_map([value], |row| row.get::<_, WorkId>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for work in found {
            push(work, &mut works);
        }
    }
    let title = fold(&source.title);
    let author = source
        .creators
        .first()
        .map(|person| fold(&person.name))
        .or_else(|| source.creator_names.first().map(|name| fold(name)));
    if let Some(author) = author.filter(|author| !author.is_empty() && !title.is_empty()) {
        let candidates = connection
            .prepare_cached(
                "SELECT w.id, w.title, (SELECT c.name FROM work_creator x JOIN creator c ON c.id = x.creator_id
                                        WHERE x.work_id = w.id ORDER BY x.position LIMIT 1)
                 FROM work w ORDER BY w.id",
            )?
            .query_map([], |row| Ok((row.get::<_, WorkId>(0)?, row.get::<_, String>(1)?, row.get::<_, Option<String>>(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (work, work_title, first) in candidates {
            if fold(&work_title) == title
                && first.as_deref().map(fold).as_deref() == Some(author.as_str())
            {
                push(work, &mut works);
            }
        }
    }
    Ok(works)
}

/// The document of a work that takes annotations made on `source`: one of the same layout, of the
/// same format if it has one, the newest.
fn document_in(
    connection: &Connection,
    work: WorkId,
    source: &Source,
) -> Result<Option<DocumentId>> {
    let documents = connection
        .prepare_cached("SELECT id, layout, format, pdf_id0, dc_identifier FROM document WHERE work_id = ?1 ORDER BY id DESC")?
        .query_map([work], |row| {
            Ok((row.get::<_, DocumentId>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, Option<String>>(3)?, row.get::<_, Option<String>>(4)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let fitting: Vec<_> = documents
        .into_iter()
        .filter(|(_, layout, ..)| *layout == source.layout)
        .collect();
    let score = |(_, _, format, pdf_id0, dc): &(
        DocumentId,
        String,
        String,
        Option<String>,
        Option<String>,
    )| {
        u8::from(format.eq_ignore_ascii_case(&source.file_format)) * 2
            + u8::from(pdf_id0.is_some() && *pdf_id0 == source.pdf_id0)
            + u8::from(dc.is_some() && *dc == source.dc_identifier)
    };
    Ok(fitting
        .iter()
        .max_by_key(|document| (score(document), document.0))
        .map(|document| document.0))
}

fn place(
    connection: &Connection,
    incoming: &Incoming,
    target: ExchangeTarget,
    single_work: bool,
) -> Result<Placement> {
    let target_work = match target {
        ExchangeTarget::Document(document) => Some(document_facts(connection, document)?.work),
        ExchangeTarget::Work(work) => Some(work),
        ExchangeTarget::Library => None,
    };
    let fits = |document: DocumentId| -> Result<bool> {
        Ok(match target {
            ExchangeTarget::Document(asked) => document == asked,
            ExchangeTarget::Work(work) => document_facts(connection, document)?.work == work,
            ExchangeTarget::Library => true,
        })
    };
    // The same bytes.
    let mut hashes: Vec<[u8; 32]> = Vec::new();
    hashes.extend(incoming.resolved);
    hashes.extend(incoming.revision);
    if let Some(id) = &incoming.source.id {
        hashes.push(hash_of_iri(id)?);
    }
    for hash in &hashes {
        if let Some((_, document)) = revision_of_hash(connection, hash)? {
            return Ok(if fits(document)? {
                Placement::Into {
                    document,
                    other_work: false,
                }
            } else {
                Placement::Elsewhere
            });
        }
    }
    // The same work.
    let works = works_like(connection, &incoming.source)?;
    let layout_fits = |document: DocumentId| -> Result<bool> {
        Ok(document_facts(connection, document)?.layout == incoming.source.layout)
    };
    match target {
        ExchangeTarget::Document(asked) => {
            if works.contains(&target_work.unwrap_or_default()) {
                return Ok(if layout_fits(asked)? {
                    Placement::Into {
                        document: asked,
                        other_work: false,
                    }
                } else {
                    Placement::Elsewhere
                });
            }
            if single_work && layout_fits(asked)? {
                return Ok(Placement::Into {
                    document: asked,
                    other_work: true,
                });
            }
            Ok(if works.is_empty() {
                Placement::Unmatched
            } else {
                Placement::Elsewhere
            })
        }
        ExchangeTarget::Work(work) => {
            let matched = works.contains(&work);
            if matched || single_work {
                return Ok(match document_in(connection, work, &incoming.source)? {
                    Some(document) => Placement::Into {
                        document,
                        other_work: !matched,
                    },
                    None => Placement::Elsewhere,
                });
            }
            Ok(if works.is_empty() {
                Placement::Unmatched
            } else {
                Placement::Elsewhere
            })
        }
        ExchangeTarget::Library => {
            for work in works {
                if let Some(document) = document_in(connection, work, &incoming.source)? {
                    return Ok(Placement::Into {
                        document,
                        other_work: false,
                    });
                }
            }
            Ok(Placement::Unmatched)
        }
    }
}

fn local_revision(
    connection: &Connection,
    hash: &Option<[u8; 32]>,
    document: DocumentId,
) -> Result<Option<RevisionId>> {
    let Some(hash) = hash else {
        return Ok(None);
    };
    Ok(connection
        .prepare_cached("SELECT id FROM revision WHERE blake3 = ?1 AND document_id = ?2")?
        .query_row(params![hash.as_slice(), document], |row| row.get(0))
        .optional()?)
}

fn newest_revision(connection: &Connection, document: DocumentId) -> Result<RevisionId> {
    connection
        .prepare_cached("SELECT max(id) FROM revision WHERE document_id = ?1")?
        .query_row([document], |row| row.get::<_, Option<RevisionId>>(0))?
        .ok_or_else(|| Error::NotFound(format!("document {document} has no revision")))
}

fn insert(
    transaction: &Transaction<'_>,
    incoming: &Incoming,
    document: DocumentId,
) -> Result<bool> {
    let creation = local_revision(transaction, &incoming.revision, document)?;
    let resolved = local_revision(
        transaction,
        &incoming.resolved.or(incoming.revision),
        document,
    )?;
    let revision = match creation.or(resolved) {
        Some(revision) => revision,
        None => newest_revision(transaction, document)?,
    };
    let mut segments = Vec::with_capacity(incoming.segments.len());
    for (segment, revisions) in &incoming.segments {
        let mut segment = segment.clone();
        for (selector, hash) in segment.selectors.iter_mut().zip(revisions) {
            selector.revision_id = local_revision(transaction, hash, document)?;
        }
        segments.push(segment);
    }
    let new = NewAnnotation {
        document_id: document,
        revision_id: revision,
        kind: incoming.kind,
        color: incoming.color.clone(),
        note: incoming.note.clone(),
        quote: incoming.quote.clone(),
        quote_source: incoming.quote_source,
        progression: incoming.progression,
        anchor_state: incoming.anchor_state,
        resolved_version: resolved.and(incoming.resolved_version),
        pdf_nm: incoming.pdf_nm.clone(),
        segments,
    };
    annotations::check_new(&new)?;
    transaction
        .prepare_cached(
            "INSERT INTO annotation (uuid, document_id, revision_id, kind, color, note, quote,
                 quote_source, progression, anchor_state, resolved_revision_id, resolved_version,
                 pdf_nm, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        )?
        .execute(params![
            incoming.uuid.as_slice(),
            document,
            revision,
            new.kind.as_str(),
            new.color,
            new.note,
            new.quote,
            new.quote_source.as_str(),
            new.progression,
            new.anchor_state.as_str(),
            resolved,
            new.resolved_version,
            new.pdf_nm,
            incoming.created,
            incoming.updated,
        ])?;
    let id = transaction.last_insert_rowid();
    for segment in &new.segments {
        annotations::insert_segment(transaction, id, segment)?;
    }
    Ok(resolved.is_none())
}

impl Library {
    /// Writes the annotations of a document, of works, or of the whole library, as a JSON-LD
    /// `AnnotationCollection`. Deleted annotations stay out.
    pub fn export_jsonld(
        &self,
        scope: ExchangeScope,
        generated_at: i64,
        version: &str,
    ) -> Result<ExchangeExport> {
        let version = version.to_owned();
        self.database().read(move |connection| {
            let documents: Vec<DocumentId> = match &scope {
                ExchangeScope::Document(document) => vec![*document],
                ExchangeScope::Works(works) => {
                    let mut documents = Vec::new();
                    for work in works {
                        documents.extend(
                            connection
                                .prepare_cached("SELECT id FROM document WHERE work_id = ?1 ORDER BY id")?
                                .query_map([work], |row| row.get::<_, DocumentId>(0))?
                                .collect::<rusqlite::Result<Vec<_>>>()?,
                        );
                    }
                    documents
                }
                ExchangeScope::Library => connection
                    .prepare_cached("SELECT DISTINCT document_id FROM annotation WHERE deleted_at IS NULL ORDER BY document_id")?
                    .query_map([], |row| row.get::<_, DocumentId>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?,
            };
            let mut items = Vec::new();
            let mut documents_with = 0;
            let mut works = HashSet::new();
            for document in documents {
                let facts = document_facts(connection, document)?;
                let annotations = annotations::load_where(connection, "a.document_id = ?1 AND a.deleted_at IS NULL", &document)?;
                if annotations.is_empty() {
                    continue;
                }
                documents_with += 1;
                works.insert(facts.work);
                let source = source_of(connection, &facts)?;
                let revisions = revisions_of(connection, document)?;
                for annotation in &annotations {
                    items.push(json_annotation(annotation, &source, &revisions));
                }
            }
            let count = items.len();
            let collection = Collection {
                context: context(),
                id: format!("urn:uuid:{}", annotations::new_uuid()),
                kind: "AnnotationCollection".to_owned(),
                label: tr!(CollectionLabel),
                total: count,
                generator: Generator {
                    kind: "Software".to_owned(),
                    name: "Musaeum".to_owned(),
                    version,
                },
                generated: date_text(generated_at),
                format_version: FORMAT_VERSION,
                scope: scope.name().to_owned(),
                first: Page {
                    kind: "AnnotationPage".to_owned(),
                    start_index: 0,
                    items,
                },
            };
            let mut json = serde_json::to_string_pretty(&collection)?;
            json.push('\n');
            Ok(ExchangeExport {
                json,
                annotations: count,
                documents: documents_with,
                works: works.len(),
            })
        })
    }

    /// Brings the annotations of a JSON-LD file into the library: into one document, one work's,
    /// or wherever their works are. The file is checked whole before anything is written, and
    /// everything is written in one transaction.
    pub fn import_jsonld(&self, bytes: &[u8], target: ExchangeTarget) -> Result<ExchangeImport> {
        let (_, incoming) = read_file(bytes)?;
        let total = incoming.len();
        let works: HashSet<String> = incoming
            .iter()
            .map(|item| {
                serde_json::to_string(&Source {
                    id: None,
                    layout: String::new(),
                    file_format: String::new(),
                    format: String::new(),
                    ..item.source.clone()
                })
                .unwrap_or_default()
            })
            .collect();
        let single_work = works.len() == 1;
        self.database().write(move |transaction| {
            let mut into: BTreeMap<DocumentId, ImportedInto> = BTreeMap::new();
            let mut unmatched: Vec<UnmatchedWork> = Vec::new();
            let mut already = 0;
            let mut elsewhere = 0;
            let mut other_work = 0;
            let mut seen = HashSet::new();
            for item in &incoming {
                let known: bool = transaction
                    .prepare_cached("SELECT EXISTS (SELECT 1 FROM annotation WHERE uuid = ?1)")?
                    .query_row([item.uuid.as_slice()], |row| row.get(0))?;
                if known || !seen.insert(item.uuid) {
                    already += 1;
                    continue;
                }
                match place(transaction, item, target, single_work)? {
                    Placement::Into { document, other_work: other } => {
                        if let Some(name) = &item.pdf_nm {
                            let named: bool = transaction
                                .prepare_cached("SELECT EXISTS (SELECT 1 FROM annotation WHERE document_id = ?1 AND pdf_nm = ?2)")?
                                .query_row(params![document, name], |row| row.get(0))?;
                            if named {
                                already += 1;
                                continue;
                            }
                        }
                        let needs = insert(transaction, item, document)?;
                        other_work += usize::from(other);
                        let entry = match into.entry(document) {
                            std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
                            std::collections::btree_map::Entry::Vacant(entry) => {
                                let facts = document_facts(transaction, document)?;
                                let title = crate::metadata::work_metadata(transaction, facts.work)?.title;
                                entry.insert(ImportedInto { document_id: document, title, added: Vec::new(), needs_anchoring: false })
                            }
                        };
                        entry.needs_anchoring |= needs;
                        entry.added.extend(annotations::load_where(transaction, "a.uuid = ?1", &item.uuid.as_slice())?);
                    }
                    Placement::Elsewhere => elsewhere += 1,
                    Placement::Unmatched => {
                        let creators: Vec<String> = if item.source.creators.is_empty() {
                            item.source.creator_names.clone()
                        } else {
                            item.source.creators.iter().map(|person| person.name.clone()).collect()
                        };
                        match unmatched.iter_mut().find(|work| work.title == item.source.title && work.creators == creators) {
                            Some(work) => work.annotations += 1,
                            None => unmatched.push(UnmatchedWork { title: item.source.title.clone(), creators, annotations: 1 }),
                        }
                    }
                }
            }
            Ok(ExchangeImport {
                into: into.into_values().collect(),
                already,
                unmatched,
                elsewhere,
                other_work,
                total,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_xsd_and_lossless() {
        for ms in [0, 1_795_466_405_123, 951_782_400_000, 1_700_000_000_001] {
            assert_eq!(parse_date(&date_text(ms)).unwrap(), ms, "{}", date_text(ms));
        }
        assert_eq!(date_text(1_795_466_405_123), "2026-11-23T20:40:05.123Z");
        assert_eq!(
            parse_date("2026-11-23T17:40:05-03:00").unwrap(),
            1_795_466_405_000
        );
        assert_eq!(
            parse_date("2026-11-23T20:40:05.1234567Z").unwrap(),
            1_795_466_405_123
        );
        for bad in [
            "2026-02-30T00:00:00Z",
            "2026-11-23 20:40:05Z",
            "2026-11-23T20:40:05",
            "2026-11-23T24:00:00Z",
            "x",
        ] {
            assert!(parse_date(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn nesting_is_counted_outside_strings() {
        assert_eq!(nesting(br#"{"a": [1, {"b": "[[[{{{"}]}"#), 3);
        assert_eq!(nesting(br#""\"[""#), 0);
        assert_eq!(nesting("[".repeat(40).as_bytes()), 40);
    }

    fn selector(kind: SelectorKind, data: Value, priority: i64) -> Selector {
        Selector {
            kind,
            data,
            priority,
            normalizer_version: Some(3),
            revision_id: Some(7),
        }
    }

    #[test]
    fn selectors_go_as_w3c_s_when_they_say_the_same_and_come_back_as_they_were() {
        let revisions = HashMap::from([(7, revision_iri(&[0xab; 32]))]);
        let cases = [
            (
                selector(
                    SelectorKind::TextQuote,
                    json!({"exact": "ação", "prefix": "a ", "suffix": " b"}),
                    2,
                ),
                "TextQuoteSelector",
            ),
            (
                selector(SelectorKind::TextQuote, json!({"exact": "só isso"}), 2),
                "TextQuoteSelector",
            ),
            (
                selector(
                    SelectorKind::TextPosition,
                    json!({"start": 10, "end": 42}),
                    1,
                ),
                "TextPositionSelector",
            ),
            (
                selector(
                    SelectorKind::Cfi,
                    json!({"start": "/6/4!/4/2/1:0", "end": "/6/4!/4/2/1:10"}),
                    0,
                ),
                "RangeSelector",
            ),
            (
                selector(
                    SelectorKind::PageCharRange,
                    json!({"page": 3, "start": 5, "end": 9}),
                    0,
                ),
                "musaeum:PageCharRangeSelector",
            ),
            (
                selector(
                    SelectorKind::PageQuads,
                    json!({"page": 3, "quads": [[0.1, 0.2, 0.3, 0.2, 0.3, 0.25, 0.1, 0.25]]}),
                    1,
                ),
                "musaeum:PageQuadsSelector",
            ),
            (
                selector(
                    SelectorKind::PageRegion,
                    json!({"page": 1, "tool": "free", "quads": [[0.1, 0.1, 0.2, 0.1, 0.2, 0.2, 0.1, 0.2]], "clip": [0.1, 0.1, 0.2, 0.2], "lines": [], "path": [0.1, 0.1, 0.2, 0.2], "pageHash": "00ff00ff00ff00ff", "regionHash": null}),
                    0,
                ),
                "musaeum:PageRegionSelector",
            ),
            // A quote with something more in it is kept as the library's, whole.
            (
                selector(
                    SelectorKind::TextQuote,
                    json!({"exact": "x", "extra": 1}),
                    2,
                ),
                "musaeum:TextQuoteSelector",
            ),
            (
                selector(
                    SelectorKind::TextPosition,
                    json!({"start": -1, "end": 2}),
                    1,
                ),
                "musaeum:TextPositionSelector",
            ),
        ];
        for (original, kind) in cases {
            let written = json_selector(&original, &revisions);
            assert_eq!(written.kind, kind);
            let text = serde_json::to_string(&written).unwrap();
            let read: JsonSelector = serde_json::from_str(&text).unwrap();
            let (back, revision) = read_selector(&read).unwrap().unwrap();
            assert_eq!(back.kind, original.kind);
            assert_eq!(back.data, original.data, "{text}");
            assert_eq!(back.priority, original.priority);
            assert_eq!(back.normalizer_version, original.normalizer_version);
            assert_eq!(revision.as_deref(), Some(revisions[&7].as_str()));
        }
    }

    #[test]
    fn hostile_selectors_are_refused() {
        let read = |text: &str| -> Result<Option<(Selector, Option<String>)>> {
            let json: JsonSelector =
                serde_json::from_str(text).map_err(|error| refused(error.to_string()))?;
            read_selector(&json)
        };
        assert!(
            read(
                r#"{"type": "TextQuoteSelector", "exact": "a", "musaeum:priority": 1, "extra": 1}"#
            )
            .is_err(),
            "an unknown property"
        );
        assert!(read(r#"{"type": "TextQuoteSelector", "exact": "a", "exact": "b", "musaeum:priority": 1}"#).is_err(), "a property twice");
        assert!(
            read(
                r#"{"type": "TextQuoteSelector", "exact": "a", "start": 1, "musaeum:priority": 1}"#
            )
            .is_err(),
            "a property of another type"
        );
        assert!(
            read(r#"{"type": "XPathSelector", "value": "/a", "musaeum:priority": 1}"#).is_err()
        );
        assert!(
            read(
                r#"{"type": "TextPositionSelector", "start": 1, "end": 2, "musaeum:priority": 1}"#
            )
            .is_err(),
            "code points are not what is kept"
        );
        assert!(read(r#"{"type": "FragmentSelector", "conformsTo": "http://www.w3.org/TR/media-frags/", "value": "t=1"}"#).is_err());
        assert!(read(r#"{"type": "FragmentSelector", "conformsTo": "http://tools.ietf.org/rfc/rfc3778", "value": "page=3"}"#).unwrap().is_none());
        assert!(read(r#"{"type": "RangeSelector", "startSelector": {"type": "XPathSelector", "value": "/"}, "endSelector": {"type": "XPathSelector", "value": "/"}, "musaeum:priority": 0}"#).is_err());
        assert!(read(r#"{"type": "musaeum:PageRegionSelector", "musaeum:value": [[[[[[[[[[1]]]]]]]]]], "musaeum:priority": 0}"#).is_err(), "nested too deep");
        assert!(hash_of_iri("urn:blake3:ABC").is_err());
        assert!(
            hash_of_iri(&format!("urn:blake3:{}", "A".repeat(64))).is_err(),
            "lower case only"
        );
    }
}

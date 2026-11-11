//! One position for every flowing book: a [`Locator`], in the shape of Readium's.
//!
//! A Locator names its document by `href`, says how far into it and into the book it is
//! (`progression`, `totalProgression`, from 0 to 1), points at the exact place with a CFI, and
//! quotes the text there with what comes before and after. The CFI is canonical: it counts in the
//! book's own documents, not in the parts they were cut into, so it does not change when the cut
//! does, and for an EPUB it is the CFI any other reader would write for the same place. The quote
//! finds the place again when the CFI cannot, after a new normalizer version.
//!
//! Positions are always worked out on the whole document, put back together from its parts by
//! [`LogicalDocument::assemble`], so a quote can run across the place where a document was cut.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::NORMALIZER_VERSION;
use crate::cfi::{self, Cfi, CfiError, Point, Step};
use crate::dom::{Dom, NodeId};
use crate::package::Manifest;
use crate::split;

/// Characters quoted on each side of a position, and at it.
pub const QUOTE_CHARS: usize = 32;

/// Where a Locator points, within its document and the book.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Locations {
    /// The canonical CFI.
    pub cfi: String,
    /// How far into the document, by its text, from 0 to 1.
    pub progression: f64,
    /// How far into the book, by its text, from 0 to 1.
    pub total_progression: f64,
}

/// The text at a position, with some before and after it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextQuote {
    pub before: String,
    pub highlight: String,
    pub after: String,
}

/// A position in a flowing book.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Locator {
    /// The document, by its path in the source.
    pub href: String,
    #[serde(rename = "type")]
    pub media_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub locations: Locations,
    pub text: TextQuote,
    /// The normalizer version the CFI and the progressions were computed against.
    pub normalizer: u32,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LocatorError {
    #[error(transparent)]
    Cfi(#[from] CfiError),
    #[error("the parts of the document do not fit together")]
    Parts,
    #[error("nothing in the document matches")]
    NotFound,
    #[error("the position is not in the document's body")]
    OutsideBody,
}

/// A run of text in a document's body, in order.
#[derive(Clone, Copy, Debug)]
struct Run {
    node: NodeId,
    /// Where it starts in the body's text, in UTF-16 code units and in bytes.
    units: u64,
    bytes: usize,
    length: u64,
}

/// One whole document of the book, with an index of its text.
#[derive(Clone, Debug)]
pub struct LogicalDocument {
    pub index: usize,
    pub dom: Dom,
    pub body: Option<NodeId>,
    /// The text under `<body>`, all of it, in order.
    pub text: String,
    runs: Vec<Run>,
    run_of: std::collections::HashMap<NodeId, usize>,
    /// For every node under the body, the text before it and the text up to its end.
    before: Vec<u64>,
    through: Vec<u64>,
    pub text_length: u64,
}

impl LogicalDocument {
    pub fn new(index: usize, dom: Dom) -> Self {
        let body = dom.body().or_else(|| dom.root_element());
        let mut text = String::new();
        let mut runs = Vec::new();
        let mut run_of = std::collections::HashMap::new();
        let mut units = 0u64;
        let mut before = vec![u64::MAX; dom.len()];
        let mut through = vec![u64::MAX; dom.len()];
        if let Some(body) = body {
            let mut stack = vec![(body, false)];
            while let Some((node, done)) = stack.pop() {
                if done {
                    through[node] = units;
                    continue;
                }
                before[node] = units;
                if let Some(value) = dom.text(node) {
                    let length = u64::from(cfi::utf16_len(value));
                    run_of.insert(node, runs.len());
                    runs.push(Run {
                        node,
                        units,
                        bytes: text.len(),
                        length,
                    });
                    text.push_str(value);
                    units += length;
                    through[node] = units;
                    continue;
                }
                stack.push((node, true));
                stack.extend(dom.children(node).iter().rev().map(|&child| (child, false)));
            }
        }
        Self {
            index,
            dom,
            body,
            text,
            runs,
            run_of,
            before,
            through,
            text_length: units,
        }
    }

    /// The document put back together from its parts, each with the start its manifest records.
    pub fn assemble(index: usize, parts: Vec<(Vec<u32>, Dom)>) -> Result<Self, LocatorError> {
        let mut parts = parts.into_iter();
        let (_, mut whole) = parts.next().ok_or(LocatorError::Parts)?;
        let body = whole.body().ok_or(LocatorError::Parts)?;
        for (start, part) in parts {
            let part_body = part.body().ok_or(LocatorError::Parts)?;
            merge(&mut whole, body, &part, part_body, &start)?;
        }
        whole.normalize_text(Dom::DOCUMENT);
        Ok(Self::new(index, whole))
    }

    /// Where `point` is in the body's text, in UTF-16 code units.
    pub fn text_offset(&self, point: Point) -> Result<u64, LocatorError> {
        match point {
            Point::Text { node, offset } => {
                let run = self.run_of.get(&node).ok_or(LocatorError::OutsideBody)?;
                Ok(self.runs[*run].units + u64::from(offset))
            }
            Point::Element(node) => match self.before.get(node) {
                Some(&units) if units != u64::MAX => Ok(units),
                _ => Err(LocatorError::OutsideBody),
            },
            Point::Gap { parent, chunk } => {
                // Before the element after the gap, or at the end of the parent.
                let next = self.dom.element_children(parent).nth((chunk / 2) as usize);
                let units = match next {
                    Some(next) => self.before.get(next),
                    None => self.through.get(parent),
                };
                match units {
                    Some(&units) if units != u64::MAX => Ok(units),
                    _ => Err(LocatorError::OutsideBody),
                }
            }
        }
    }

    /// The point at a text offset: in the run that holds it, or at the end of the last.
    pub fn point_at(&self, units: u64) -> Option<Point> {
        let index = self
            .runs
            .partition_point(|run| run.units + run.length <= units);
        if let Some(run) = self.runs.get(index) {
            return Some(Point::Text {
                node: run.node,
                offset: (units - run.units) as u32,
            });
        }
        let last = self.runs.last()?;
        (units == last.units + last.length).then_some(Point::Text {
            node: last.node,
            offset: last.length as u32,
        })
    }

    /// The point at a text offset as the end of a range: in the run the text before it ends in, so a
    /// range that ends with a run does not reach into the next one, which may be in another part.
    pub fn end_point_at(&self, units: u64) -> Option<Point> {
        if units == 0 {
            return self.point_at(0);
        }
        let index = self
            .runs
            .partition_point(|run| run.units + run.length < units);
        let run = self.runs.get(index)?;
        (units > run.units).then_some(Point::Text {
            node: run.node,
            offset: (units - run.units) as u32,
        })
    }

    /// The byte index in [`Self::text`] of a UTF-16 offset, if it falls on a character.
    pub fn byte_of(&self, units: u64) -> Option<usize> {
        if units >= self.text_length {
            return (units == self.text_length).then_some(self.text.len());
        }
        self.byte_at(units)
    }

    /// The UTF-16 offset of a byte index in [`Self::text`] that falls on a character.
    pub fn units_of(&self, byte: usize) -> u64 {
        let index = self
            .runs
            .partition_point(|run| run.bytes <= byte)
            .saturating_sub(1);
        match self.runs.get(index) {
            Some(run) if byte >= run.bytes => {
                run.units
                    + u64::from(cfi::utf16_len(
                        &self.text[run.bytes..byte.min(self.text.len())],
                    ))
            }
            _ => 0,
        }
    }

    /// The byte index in [`Self::text`] of a UTF-16 offset that falls on a character.
    fn byte_at(&self, units: u64) -> Option<usize> {
        let index = self
            .runs
            .partition_point(|run| run.units + run.length < units);
        let run = self.runs.get(index).or(self.runs.last())?;
        let value = self.dom.text(run.node)?;
        let inner = cfi::utf16_to_byte(value, u32::try_from(units.checked_sub(run.units)?).ok()?)?;
        Some(run.bytes + inner)
    }

    /// The quote at a text offset.
    pub fn quote(&self, units: u64) -> TextQuote {
        let at = self.byte_at(units).unwrap_or(self.text.len());
        let before: String = {
            let chars: Vec<char> = self.text[..at].chars().rev().take(QUOTE_CHARS).collect();
            chars.into_iter().rev().collect()
        };
        let mut rest = self.text[at..].chars();
        let highlight: String = rest.by_ref().take(QUOTE_CHARS).collect();
        let after: String = rest.take(QUOTE_CHARS).collect();
        TextQuote {
            before,
            highlight,
            after,
        }
    }

    /// The canonical steps of a point inside this document.
    pub fn path(&self, point: Point) -> (Vec<Step>, Option<u32>) {
        cfi::point_to_path(&self.dom, point)
    }
}

/// Merges `part`'s content into `whole`, at the level where `start` says the part begins.
fn merge(
    whole: &mut Dom,
    into: NodeId,
    part: &Dom,
    from: NodeId,
    start: &[u32],
) -> Result<(), LocatorError> {
    let children = part.children(from).to_vec();
    if start.len() <= 1 {
        // The part's content at this level is all new.
        for child in children {
            let copy = part.copy_into(child, whole);
            whole.append(into, copy);
        }
        return Ok(());
    }
    // The first element is the rest of an element the parts before began: merge into it.
    let mut rest = children.into_iter();
    let first = rest.next().ok_or(LocatorError::Parts)?;
    if !part.is_element(first) {
        return Err(LocatorError::Parts);
    }
    let existing = whole
        .element_children(into)
        .last()
        .ok_or(LocatorError::Parts)?;
    if whole.name(existing).map(|name| &name.local) != part.name(first).map(|name| &name.local) {
        return Err(LocatorError::Parts);
    }
    merge(whole, existing, part, first, &start[1..])?;
    for child in rest {
        let copy = part.copy_into(child, whole);
        whole.append(into, copy);
    }
    Ok(())
}

/// The canonical CFI of a point in a document.
pub fn canonical_cfi(manifest: &Manifest, document: &LogicalDocument, point: Point) -> String {
    let (path, offset) = document.path(point);
    Cfi {
        package: vec![
            Step::new(6),
            Step {
                index: 2 * (document.index as u32 + 1),
                id: manifest.documents[document.index].id.clone(),
            },
        ],
        path,
        offset,
    }
    .to_string()
}

/// Which document a canonical CFI points into.
pub fn document_of(manifest: &Manifest, cfi: &Cfi) -> Result<usize, LocatorError> {
    let step = cfi.package.get(1).ok_or(CfiError::NotFound)?;
    if cfi.package[0].index != 6 || step.index % 2 != 0 || step.index == 0 {
        return Err(CfiError::NoDocument(step.index).into());
    }
    let index = (step.index / 2 - 1) as usize;
    if index < manifest.documents.len() {
        return Ok(index);
    }
    // The spine step disagrees with the book; the itemref's id may still name a document.
    if let Some(id) = &step.id
        && let Some(found) = manifest
            .documents
            .iter()
            .position(|d| d.id.as_ref() == Some(id))
    {
        return Ok(found);
    }
    Err(CfiError::NoDocument(step.index).into())
}

/// The Locator of a point.
pub fn locate(
    manifest: &Manifest,
    document: &LogicalDocument,
    point: Point,
) -> Result<Locator, LocatorError> {
    let info = &manifest.documents[document.index];
    let offset = document.text_offset(point)?;
    let total = manifest.total_text();
    let before = manifest.text_before(document.index);
    let ratio = |part: u64, whole: u64| {
        if whole == 0 {
            0.0
        } else {
            part as f64 / whole as f64
        }
    };
    Ok(Locator {
        href: info.href.clone(),
        media_type: info.media_type.clone(),
        title: info.title.clone(),
        locations: Locations {
            cfi: canonical_cfi(manifest, document, point),
            progression: ratio(offset, document.text_length),
            total_progression: ratio(before + offset, total),
        },
        text: document.quote(offset),
        normalizer: NORMALIZER_VERSION,
    })
}

/// How a Locator was found again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Found {
    /// By its CFI, exactly.
    Cfi,
    /// By its CFI, after an `id` assertion corrected it.
    CorrectedCfi,
    /// By its quoted text, the CFI being from another normalizer version or broken.
    Quote,
    /// By its progression, nothing else matching.
    Progression,
}

/// Finds a Locator's position again in its document, which the caller assembles.
pub fn resolve(
    locator: &Locator,
    document: &LogicalDocument,
) -> Result<(Point, Found), LocatorError> {
    if locator.normalizer == NORMALIZER_VERSION
        && let Ok(cfi) = cfi::parse(&locator.locations.cfi)
        && let Ok((point, corrected)) = cfi::resolve(&document.dom, &cfi.path, cfi.offset)
    {
        return Ok((
            point,
            if corrected {
                Found::CorrectedCfi
            } else {
                Found::Cfi
            },
        ));
    }
    if let Some(units) = find_quote(document, &locator.text, locator.locations.progression)
        && let Some(point) = document.point_at(units)
    {
        return Ok((point, Found::Quote));
    }
    let units = (locator.locations.progression.clamp(0.0, 1.0) * document.text_length as f64)
        .round() as u64;
    document
        .point_at(units.min(document.text_length))
        .map(|point| (point, Found::Progression))
        .ok_or(LocatorError::NotFound)
}

/// The text offset where a quote matches, the match nearest `progression` when there are several.
pub fn find_quote(document: &LogicalDocument, quote: &TextQuote, progression: f64) -> Option<u64> {
    let full = format!("{}{}{}", quote.before, quote.highlight, quote.after);
    if full.is_empty() {
        return None;
    }
    let expected = progression.clamp(0.0, 1.0) * document.text_length as f64;
    let mut best: Option<(f64, u64)> = None;
    // Every match, overlapping ones too: in code and tables the same characters repeat, and the
    // right match can start inside another.
    let mut from = 0;
    let mut counted = (0usize, 0u64);
    while let Some(found) = document.text[from..].find(&full) {
        let at = from + found;
        let start = at + quote.before.len();
        // UTF-16 units up to `start`, counted on from the last match.
        counted.1 += u64::from(cfi::utf16_len(&document.text[counted.0..start]));
        counted.0 = start;
        let units = counted.1;
        let distance = (units as f64 - expected).abs();
        if best.is_none_or(|(d, _)| distance < d) {
            best = Some((distance, units));
        }
        from = at + document.text[at..].chars().next().map_or(1, char::len_utf8);
    }
    best.map(|(_, units)| units)
}

/// A CFI from the reader, which counts in the normalized book's parts, as the canonical CFI.
pub fn reader_to_canonical(manifest: &Manifest, reader: &str) -> Result<String, LocatorError> {
    let cfi = cfi::parse(reader)?;
    let item = cfi.package.get(1).ok_or(CfiError::NotFound)?;
    let global = (item.index / 2).checked_sub(1).ok_or(CfiError::NotFound)? as usize;
    let (document, part) = manifest
        .spine()
        .nth(global)
        .ok_or(CfiError::NoDocument(item.index))?;
    let info = &manifest.documents[document];
    let start = &info.parts[part].start;
    let path = translate(&cfi.path, info.body_step, |inner| {
        split::from_part_path(start, inner)
    });
    Ok(Cfi {
        package: vec![
            Step::new(6),
            Step {
                index: 2 * (document as u32 + 1),
                id: info.id.clone(),
            },
        ],
        path,
        offset: cfi.offset,
    }
    .to_string())
}

/// A canonical CFI as the reader's, in the part that holds the position.
pub fn canonical_to_reader(manifest: &Manifest, canonical: &str) -> Result<String, LocatorError> {
    let cfi = cfi::parse(canonical)?;
    let document = document_of(manifest, &cfi)?;
    let info = &manifest.documents[document];
    let starts: Vec<Vec<u32>> = info.parts.iter().map(|part| part.start.clone()).collect();
    let below_body: Vec<u32> = match (info.body_step, cfi.path.first()) {
        (Some(body), Some(first)) if first.index == body => {
            cfi.path[1..].iter().map(|step| step.index).collect()
        }
        _ => Vec::new(),
    };
    let part = if below_body.is_empty() {
        0
    } else {
        split::part_of(&starts, &below_body)
    };
    let start = &starts[part];
    let path = translate(&cfi.path, info.body_step, |inner| {
        split::to_part_path(start, inner)
    });
    let global = manifest
        .spine()
        .position(|(d, p)| d == document && p == part)
        .ok_or(LocatorError::Parts)?;
    Ok(Cfi {
        package: vec![Step::new(6), Step::new(2 * (global as u32 + 1))],
        path,
        offset: cfi.offset,
    }
    .to_string())
}

/// Applies `map` to the steps below `<body>`, keeping the assertions.
fn translate(path: &[Step], body_step: Option<u32>, map: impl Fn(&[u32]) -> Vec<u32>) -> Vec<Step> {
    match (body_step, path.first()) {
        (Some(body), Some(first)) if first.index == body => {
            let inner: Vec<u32> = path[1..].iter().map(|step| step.index).collect();
            let mapped = map(&inner);
            let mut out = vec![first.clone()];
            out.extend(
                mapped
                    .into_iter()
                    .zip(&path[1..])
                    .map(|(index, step)| Step {
                        index,
                        id: step.id.clone(),
                    }),
            );
            out
        }
        _ => path.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{Document, Metadata, PartInfo, Source, SourceFormat, Stats};
    use crate::split::{PART_BUDGET, split};
    use crate::xml::{Limits, parse};

    fn manifest_for(parts: &[split::Part], text_length: u64) -> Manifest {
        Manifest {
            normalizer: NORMALIZER_VERSION,
            source: Source {
                format: SourceFormat::Epub,
                version: None,
                fixed_layout: false,
                direction: None,
            },
            metadata: Metadata::default(),
            documents: vec![
                Document {
                    href: "intro.xhtml".to_owned(),
                    id: Some("intro".to_owned()),
                    media_type: "application/xhtml+xml".to_owned(),
                    linear: true,
                    parsed: "xml".to_owned(),
                    body_step: Some(4),
                    text_length: 0,
                    parts: vec![PartInfo {
                        href: "text/0000.xhtml".to_owned(),
                        ..PartInfo::default()
                    }],
                    ..Document::default()
                },
                Document {
                    href: "OEBPS/long.xhtml".to_owned(),
                    id: Some("long".to_owned()),
                    media_type: "application/xhtml+xml".to_owned(),
                    linear: true,
                    parsed: "xml".to_owned(),
                    body_step: Some(4),
                    text_length,
                    parts: parts
                        .iter()
                        .enumerate()
                        .map(|(k, part)| PartInfo {
                            href: format!("text/0001-{k}.xhtml"),
                            start: part.start.clone(),
                            ..PartInfo::default()
                        })
                        .collect(),
                    ..Document::default()
                },
            ],
            resources: Vec::new(),
            toc: Vec::new(),
            page_list: Vec::new(),
            landmarks: Vec::new(),
            stats: Stats::default(),
            warnings: Vec::new(),
        }
    }

    fn long_document() -> Dom {
        let paragraphs: String = (0..2500)
            .map(|n| format!("<p id='p{n}'>Paragraph {n} says <em>something {n}</em> 😀 and more words here. </p>\n"))
            .collect();
        parse(
            &format!(
                "<html xmlns='http://www.w3.org/1999/xhtml'><head><title>t</title></head><body>\
                 <h1>Long</h1>\n<div class='chapter'>{paragraphs}</div>\n<p>The end.</p></body></html>"
            ),
            &Limits::default(),
        )
        .unwrap()
    }

    #[test]
    fn parts_assemble_into_the_document_they_came_from() {
        let dom = long_document();
        let parts = split(&dom, PART_BUDGET);
        assert!(parts.len() > 1);
        let assembled = LogicalDocument::assemble(
            1,
            parts
                .iter()
                .map(|part| (part.start.clone(), part.dom.clone()))
                .collect(),
        )
        .unwrap();
        assert_eq!(
            crate::serialize::to_xhtml(&assembled.dom),
            crate::serialize::to_xhtml(&dom)
        );
    }

    #[test]
    fn every_position_goes_to_a_locator_and_back_exactly() {
        let dom = long_document();
        let parts = split(&dom, PART_BUDGET);
        let document = LogicalDocument::new(1, dom);
        let manifest = manifest_for(&parts, document.text_length);
        let mut checked = 0;
        let nodes: Vec<NodeId> = document.dom.descendants(document.body.unwrap()).collect();
        for node in nodes.into_iter().step_by(7) {
            let points = match document.dom.text(node) {
                Some(text) => {
                    let length = cfi::utf16_len(text);
                    vec![
                        Point::Text { node, offset: 0 },
                        Point::Text {
                            node,
                            offset: length / 2,
                        },
                        Point::Text {
                            node,
                            offset: length,
                        },
                    ]
                }
                None => vec![Point::Element(node)],
            };
            for point in points {
                // A point that splits a surrogate pair is not a place in the text.
                if let Point::Text { node, offset } = point
                    && cfi::utf16_to_byte(document.dom.text(node).unwrap(), offset).is_none()
                {
                    continue;
                }
                let locator = locate(&manifest, &document, point).unwrap();
                let json = serde_json::to_string(&locator).unwrap();
                let read: Locator = serde_json::from_str(&json).unwrap();
                let (found, how) = resolve(&read, &document).unwrap();
                assert_eq!(how, Found::Cfi);
                assert_eq!(found, point, "{}", locator.locations.cfi);
                assert_eq!(locate(&manifest, &document, found).unwrap(), locator);
                // The reader's CFI, in the part, and back.
                let reader = canonical_to_reader(&manifest, &locator.locations.cfi).unwrap();
                assert_eq!(
                    reader_to_canonical(&manifest, &reader).unwrap(),
                    locator.locations.cfi
                );
                checked += 1;
            }
        }
        assert!(checked > 1000, "{checked}");
    }

    #[test]
    fn a_locator_from_another_version_is_found_by_its_quote() {
        let dom = long_document();
        let parts = split(&dom, PART_BUDGET);
        let document = LogicalDocument::new(1, dom);
        let manifest = manifest_for(&parts, document.text_length);
        let target = document
            .dom
            .descendants(document.body.unwrap())
            .find(|&node| {
                document
                    .dom
                    .text(node)
                    .is_some_and(|t| t.starts_with("Paragraph 1234 "))
            })
            .unwrap();
        let point = Point::Text {
            node: target,
            offset: 5,
        };
        let mut locator = locate(&manifest, &document, point).unwrap();
        locator.normalizer = NORMALIZER_VERSION + 1;
        locator.locations.cfi = "epubcfi(/6/4!/4/99/99/1:0)".to_owned();
        let (found, how) = resolve(&locator, &document).unwrap();
        assert_eq!(how, Found::Quote);
        assert_eq!(
            document.text_offset(found).unwrap(),
            document.text_offset(point).unwrap()
        );
    }

    #[test]
    fn a_quote_inside_repeated_characters_is_found_where_it_was() {
        let dom = parse(
            &format!(
                "<html xmlns='http://www.w3.org/1999/xhtml'><body><pre>{}</pre></body></html>",
                " ".repeat(300)
            ),
            &Limits::default(),
        )
        .unwrap();
        let document = LogicalDocument::new(0, dom);
        // Every match of 96 spaces overlaps the next; the old search only saw every 96th.
        let target = 100u64;
        let quote = document.quote(target);
        assert_eq!(
            find_quote(
                &document,
                &quote,
                target as f64 / document.text_length as f64
            ),
            Some(target)
        );
    }

    #[test]
    fn the_reader_cfi_points_into_the_right_part() {
        let dom = long_document();
        let parts = split(&dom, PART_BUDGET);
        let document = LogicalDocument::new(1, dom);
        let manifest = manifest_for(&parts, document.text_length);
        let last = document
            .dom
            .descendants(document.body.unwrap())
            .filter(|&node| {
                document
                    .dom
                    .text(node)
                    .is_some_and(|t| t.starts_with("Paragraph 2499 "))
            })
            .last()
            .unwrap();
        let canonical = canonical_cfi(
            &manifest,
            &document,
            Point::Text {
                node: last,
                offset: 3,
            },
        );
        let reader = canonical_to_reader(&manifest, &canonical).unwrap();
        let reader_cfi = cfi::parse(&reader).unwrap();
        let global = (reader_cfi.package[1].index / 2 - 1) as usize;
        assert_eq!(global, parts.len(), "the last part of the second document");
        let part = &parts[parts.len() - 1];
        let (point, _) = cfi::resolve(&part.dom, &reader_cfi.path, reader_cfi.offset).unwrap();
        assert!(
            matches!(point, Point::Text { node, offset: 3 } if part.dom.text(node).unwrap().starts_with("Paragraph 2499 "))
        );
    }
}

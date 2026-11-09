//! EPUB Canonical Fragment Identifiers: written for a node, and found again from one.
//!
//! A CFI walks from a document's root element down: an even step is an element, counted among
//! elements only (`/4` is the second), an odd step the run of text between two elements (`/3` lies
//! between the first and the second), and `:n` an offset into that text. Offsets count UTF-16 code
//! units, as JavaScript strings do, so that a CFI written here means the same character in the
//! reader. An element's `id` goes along as an assertion, `/4[body01]`, and wins when the counting
//! disagrees, which is how the specification recovers from an edited document.
//!
//! Only what positions need is read: steps, indirection, assertions and a character offset. A range
//! (`,`), a spatial (`@`) or temporal (`~`) offset is an error.

use std::fmt;

use thiserror::Error;

use crate::dom::{Dom, NodeId};

/// One step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub index: u32,
    /// The element's `id`, asserted.
    pub id: Option<String>,
}

impl Step {
    pub fn new(index: u32) -> Self {
        Self { index, id: None }
    }
}

/// A CFI that points at one place: steps through the package to a document, then through it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cfi {
    /// Before `!`: `/6/N[id]`, the spine and an item in it.
    pub package: Vec<Step>,
    /// After `!`: from the document's root element.
    pub path: Vec<Step>,
    /// Into the text the last step names, in UTF-16 code units.
    pub offset: Option<u32>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CfiError {
    #[error("not a CFI: {0}")]
    Syntax(String),
    #[error("CFI ranges, spatial and temporal offsets are not positions")]
    Unsupported,
    #[error("the CFI leads outside the document")]
    NotFound,
    #[error("the spine step {0} names no document")]
    NoDocument(u32),
}

fn escape(text: &str, out: &mut String) {
    for c in text.chars() {
        if matches!(c, '^' | '[' | ']' | '(' | ')' | ',' | ';' | '=') {
            out.push('^');
        }
        out.push(c);
    }
}

fn write_steps(steps: &[Step], out: &mut String) {
    for step in steps {
        out.push('/');
        out.push_str(&step.index.to_string());
        if let Some(id) = &step.id {
            out.push('[');
            escape(id, out);
            out.push(']');
        }
    }
}

impl fmt::Display for Cfi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut out = String::from("epubcfi(");
        write_steps(&self.package, &mut out);
        if !self.package.is_empty() {
            out.push('!');
        }
        write_steps(&self.path, &mut out);
        if let Some(offset) = self.offset {
            out.push(':');
            out.push_str(&offset.to_string());
        }
        out.push(')');
        f.write_str(&out)
    }
}

impl std::str::FromStr for Cfi {
    type Err = CfiError;

    fn from_str(text: &str) -> Result<Self, CfiError> {
        parse(text)
    }
}

/// Reads an assertion up to its closing bracket, unescaping `^`. Returns its text and the rest.
fn read_assertion(rest: &str) -> Result<(String, &str), CfiError> {
    let mut value = String::new();
    let mut chars = rest.char_indices();
    while let Some((at, c)) = chars.next() {
        match c {
            '^' => match chars.next() {
                Some((_, escaped)) => value.push(escaped),
                None => break,
            },
            ']' => return Ok((value, &rest[at + 1..])),
            c => value.push(c),
        }
    }
    Err(CfiError::Syntax("an assertion is not closed".to_owned()))
}

fn read_number(rest: &str) -> Result<(u32, &str), CfiError> {
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    let number = rest[..end]
        .parse()
        .map_err(|_| CfiError::Syntax(format!("a number was expected at {rest:?}")))?;
    Ok((number, &rest[end..]))
}

/// Parses `epubcfi(...)`, or the same without the wrapper.
pub fn parse(text: &str) -> Result<Cfi, CfiError> {
    let text = text.trim();
    let inner = text
        .strip_prefix("epubcfi(")
        .and_then(|rest| rest.strip_suffix(')'))
        .unwrap_or(text);
    let mut cfi = Cfi {
        package: Vec::new(),
        path: Vec::new(),
        offset: None,
    };
    let mut indirected = false;
    let mut rest = inner;
    while !rest.is_empty() {
        let mut chars = rest.chars();
        match chars.next() {
            Some('/') => {
                if cfi.offset.is_some() {
                    return Err(CfiError::Syntax("a step after an offset".to_owned()));
                }
                let (index, after) = read_number(&rest[1..])?;
                rest = after;
                let mut step = Step::new(index);
                if let Some(after_bracket) = rest.strip_prefix('[') {
                    let (assertion, after) = read_assertion(after_bracket)?;
                    rest = after;
                    // Parameters such as `;s=a` come after the id; only the id is kept.
                    let id = assertion.split(';').next().unwrap_or("").to_owned();
                    if !id.is_empty() {
                        step.id = Some(id);
                    }
                }
                if indirected {
                    cfi.path.push(step);
                } else {
                    cfi.package.push(step);
                }
            }
            Some('!') => {
                if indirected {
                    return Err(CfiError::Unsupported);
                }
                indirected = true;
                rest = &rest[1..];
            }
            Some(':') => {
                let (offset, after) = read_number(&rest[1..])?;
                cfi.offset = Some(offset);
                rest = after;
                if let Some(after_bracket) = rest.strip_prefix('[') {
                    // A text assertion or side bias: not needed to find the place.
                    let (_, after) = read_assertion(after_bracket)?;
                    rest = after;
                }
            }
            Some(',' | '@' | '~') => return Err(CfiError::Unsupported),
            Some(other) => return Err(CfiError::Syntax(format!("unexpected {other:?}"))),
            None => break,
        }
    }
    if !indirected {
        // A CFI without `!` is a path inside one document.
        cfi.path = std::mem::take(&mut cfi.package);
    }
    if cfi.path.is_empty() && cfi.package.is_empty() {
        return Err(CfiError::Syntax("no steps".to_owned()));
    }
    Ok(cfi)
}

/// How many UTF-16 code units `text` takes.
pub fn utf16_len(text: &str) -> u32 {
    text.chars().map(|c| c.len_utf16() as u32).sum()
}

/// The byte index of the UTF-16 offset `units` in `text`, or `None` if it falls inside a character
/// or past the end.
pub fn utf16_to_byte(text: &str, units: u32) -> Option<usize> {
    let mut counted = 0u32;
    for (at, c) in text.char_indices() {
        if counted == units {
            return Some(at);
        }
        counted += c.len_utf16() as u32;
        if counted > units {
            return None;
        }
    }
    (counted == units).then_some(text.len())
}

/// A place in a document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Point {
    /// In a run of text, before the UTF-16 code unit `offset`.
    Text { node: NodeId, offset: u32 },
    /// At an element.
    Element(NodeId),
    /// Where a run of text would be, between elements of `parent`, with none there. `chunk` is the
    /// odd CFI index.
    Gap { parent: NodeId, chunk: u32 },
}

/// The step that leads from `parent` to its child `child`.
pub fn step_of(dom: &Dom, parent: NodeId, child: NodeId) -> Step {
    let mut elements = 0u32;
    for &sibling in dom.children(parent) {
        if sibling == child {
            break;
        }
        if dom.is_element(sibling) {
            elements += 1;
        }
    }
    if dom.is_element(child) {
        Step {
            index: 2 * (elements + 1),
            id: dom
                .attr(child, "id")
                .filter(|id| !id.is_empty())
                .map(str::to_owned),
        }
    } else {
        Step::new(2 * elements + 1)
    }
}

/// The steps from the root element to `node`, which must be under it.
pub fn path_to(dom: &Dom, node: NodeId) -> Vec<Step> {
    let root = dom.root_element();
    let mut steps = Vec::new();
    let mut current = node;
    while Some(current) != root {
        let Some(parent) = dom.parent(current) else {
            break;
        };
        steps.push(step_of(dom, parent, current));
        current = parent;
    }
    steps.reverse();
    steps
}

/// A point as CFI steps and an offset.
pub fn point_to_path(dom: &Dom, point: Point) -> (Vec<Step>, Option<u32>) {
    match point {
        Point::Text { node, offset } => (path_to(dom, node), Some(offset)),
        Point::Element(node) => (path_to(dom, node), None),
        Point::Gap { parent, chunk } => {
            let mut steps = path_to(dom, parent);
            steps.push(Step::new(chunk));
            (steps, Some(0))
        }
    }
}

/// The first element with this `id`, in document order.
pub fn element_by_id(dom: &Dom, id: &str) -> Option<NodeId> {
    dom.descendants(Dom::DOCUMENT)
        .find(|&node| dom.is_element(node) && dom.attr(node, "id") == Some(id))
}

/// Where `steps` and `offset` lead, and whether an `id` assertion had to correct the counting.
pub fn resolve(dom: &Dom, steps: &[Step], offset: Option<u32>) -> Result<(Point, bool), CfiError> {
    let mut current = dom.root_element().ok_or(CfiError::NotFound)?;
    let mut corrected = false;
    for (level, step) in steps.iter().enumerate() {
        let last = level + 1 == steps.len();
        if step.index % 2 == 0 {
            let counted = dom
                .element_children(current)
                .nth((step.index / 2).saturating_sub(1) as usize)
                .filter(|_| step.index > 0);
            let element = match (&step.id, counted) {
                (Some(id), Some(found)) if dom.attr(found, "id") == Some(id.as_str()) => found,
                (Some(id), counted) => match element_by_id(dom, id) {
                    Some(by_id) => {
                        corrected = true;
                        by_id
                    }
                    None => counted.ok_or(CfiError::NotFound)?,
                },
                (None, Some(found)) => found,
                (None, None) => return Err(CfiError::NotFound),
            };
            current = element;
            if last {
                return Ok((Point::Element(element), corrected));
            }
        } else {
            if !last {
                return Err(CfiError::NotFound);
            }
            // The run of text after element number index / 2.
            let before = (step.index / 2) as usize;
            let mut elements = 0usize;
            let mut text = None;
            for &child in dom.children(current) {
                if dom.is_element(child) {
                    elements += 1;
                    if elements > before {
                        break;
                    }
                } else if elements == before && dom.is_text(child) {
                    text = Some(child);
                }
            }
            if elements < before {
                return Err(CfiError::NotFound);
            }
            let offset = offset.unwrap_or(0);
            return match text {
                Some(node) => {
                    let length = utf16_len(dom.text(node).unwrap_or(""));
                    if offset > length {
                        return Err(CfiError::NotFound);
                    }
                    Ok((Point::Text { node, offset }, corrected))
                }
                None if offset == 0 => Ok((
                    Point::Gap {
                        parent: current,
                        chunk: step.index,
                    },
                    corrected,
                )),
                None => Err(CfiError::NotFound),
            };
        }
    }
    Ok((Point::Element(current), corrected))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xml::{Limits, parse as parse_xml};

    #[test]
    fn a_cfi_reads_and_writes_back_the_same() {
        for text in [
            "epubcfi(/6/4[chap01ref]!/4[body01]/10[para05]/3:10)",
            "epubcfi(/6/2!/4/2/1:0)",
            "epubcfi(/6/14[x^[1^]^^y]!/4/2)",
        ] {
            let cfi = parse(text).unwrap();
            assert_eq!(cfi.to_string(), text);
        }
        let cfi =
            parse("epubcfi(/6/4[chap01ref]!/4[body01]/10[para05]/3:10[yyy,zzz;s=a])").unwrap();
        assert_eq!(cfi.offset, Some(10));
        assert_eq!(cfi.package[1].id.as_deref(), Some("chap01ref"));
        assert_eq!(parse("/4/2/1:3").unwrap().path.len(), 3);
        assert_eq!(
            parse("epubcfi(/6/4!/4,/2/1:1,/2/1:5)"),
            Err(CfiError::Unsupported)
        );
        assert!(parse("epubcfi(/6/x)").is_err());
        assert!(parse("epubcfi(/6/4[open").is_err());
    }

    #[test]
    fn the_specification_example_resolves() {
        // The example document of the EPUB CFI specification, section 3.
        let dom = parse_xml(
            "<html xmlns='http://www.w3.org/1999/xhtml'><head><title>t</title></head>\
             <body id='body01'><p>…</p><p>…</p><p>…</p><p>…</p>\
             <p id='para05'>xxx<em>yyy</em>0123456789</p><p>…</p><p>…</p>\
             <img id='svgimg' src='foo.svg' alt='…'/><p>…</p><p>…</p></body></html>",
            &Limits::default(),
        )
        .unwrap();
        let cfi = parse("epubcfi(/6/4[chap01ref]!/4[body01]/10[para05]/3:10)").unwrap();
        let (point, corrected) = resolve(&dom, &cfi.path, cfi.offset).unwrap();
        assert!(!corrected);
        let Point::Text { node, offset } = point else {
            panic!("{point:?}")
        };
        assert_eq!(dom.text(node), Some("0123456789"));
        assert_eq!(offset, 10);
        let (steps, back) = point_to_path(&dom, point);
        assert_eq!(steps, cfi.path);
        assert_eq!(back, Some(10));
        let (image, _) = resolve(&dom, &parse("/4/16[svgimg]").unwrap().path, None).unwrap();
        assert!(matches!(image, Point::Element(node) if dom.attr(node, "id") == Some("svgimg")));
    }

    #[test]
    fn an_id_wins_over_a_count_that_changed() {
        let dom = parse_xml(
            "<html xmlns='http://www.w3.org/1999/xhtml'><body><p>new</p><p id='x'>moved</p></body></html>",
            &Limits::default(),
        )
        .unwrap();
        // Written when #x was the first paragraph.
        let (point, corrected) =
            resolve(&dom, &parse("/2/2[x]/1:2").unwrap().path, Some(2)).unwrap();
        assert!(corrected);
        assert!(
            matches!(point, Point::Text { node, offset: 2 } if dom.text(node) == Some("moved"))
        );
    }

    #[test]
    fn offsets_count_utf16_like_javascript() {
        assert_eq!(utf16_len("a😀b"), 4);
        assert_eq!(utf16_to_byte("a😀b", 3), Some(5));
        assert_eq!(utf16_to_byte("a😀b", 2), None, "inside the surrogate pair");
        assert_eq!(utf16_to_byte("a😀b", 4), Some(6));
        assert_eq!(utf16_to_byte("a😀b", 5), None);
    }

    #[test]
    fn empty_runs_of_text_are_gaps() {
        let dom = parse_xml(
            "<html xmlns='http://www.w3.org/1999/xhtml'><body><p>a</p><p>b</p></body></html>",
            &Limits::default(),
        )
        .unwrap();
        let (point, _) = resolve(&dom, &parse("/2/3").unwrap().path, Some(0)).unwrap();
        assert!(matches!(point, Point::Gap { chunk: 3, .. }));
        let (steps, offset) = point_to_path(&dom, point);
        assert_eq!(
            Cfi {
                package: Vec::new(),
                path: steps,
                offset
            }
            .to_string(),
            "epubcfi(/2/3:0)"
        );
        assert!(resolve(&dom, &parse("/2/3").unwrap().path, Some(1)).is_err());
        assert!(resolve(&dom, &parse("/2/9").unwrap().path, Some(0)).is_err());
    }
}

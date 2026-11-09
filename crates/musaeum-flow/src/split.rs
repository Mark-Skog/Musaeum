//! Long documents cut into parts a browser lays out in time.
//!
//! A reader lays out one document at a time, and the cost of changing the font grows with the
//! document: Phase 0 measured up to 65 ms at 200 KiB and about a second at 3 MiB (ADR 0001, D5).
//! So a document whose XHTML is larger than the budget is cut, and only between blocks: a cut never
//! falls inside a paragraph, a heading, a table, a `pre`, a figure, a list item or a run of inline
//! content. When the blocks are inside a container (`<div class="chapter">`), the container is
//! repeated in each part around its share of them, with its attributes, so styles still apply.
//!
//! Each part records where it starts in the whole document, as CFI steps below `<body>`. That is
//! all it takes to move a position between the whole document and a part, in either direction,
//! without opening either: see [`to_part_path`] and [`from_part_path`].

use crate::dom::{Dom, Name, NodeId, Ns};
use crate::serialize;

/// The largest part, in bytes of XHTML, as the plan and ADR 0001 set it.
pub const PART_BUDGET: usize = 200 * 1024;

/// Kept free in each part for what changes after the cut: links get their final targets, and a
/// list that continues gets a `start`.
const MARGIN: usize = 2 * 1024;

/// Elements that start a block, where a cut may fall.
const BLOCKS: [&str; 42] = [
    "address",
    "article",
    "aside",
    "blockquote",
    "center",
    "details",
    "dialog",
    "dd",
    "dir",
    "div",
    "dl",
    "dt",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hgroup",
    "hr",
    "li",
    "listing",
    "main",
    "menu",
    "nav",
    "ol",
    "p",
    "plaintext",
    "pre",
    "section",
    "table",
    "ul",
    "xmp",
    "noscript",
    "search",
    "summary",
];

/// Blocks that may be opened to cut between their children.
const CONTAINERS: [&str; 15] = [
    "article",
    "aside",
    "blockquote",
    "center",
    "dir",
    "div",
    "dl",
    "footer",
    "header",
    "main",
    "menu",
    "nav",
    "ol",
    "section",
    "ul",
];

/// One part of a document.
#[derive(Debug)]
pub struct Part {
    pub dom: Dom,
    /// Where the part starts in the whole document: CFI steps below `<body>`, all even, empty for
    /// the first part.
    pub start: Vec<u32>,
}

/// A run of a container's children that is never cut.
#[derive(Debug)]
struct Unit {
    /// The containers this unit sits in, outermost first, below `<body>`.
    chain: Vec<NodeId>,
    nodes: Vec<NodeId>,
    size: usize,
    /// Where it starts, as in [`Part::start`].
    start: Vec<u32>,
    /// Whether a part may start with it. Content after a container that was opened up sits
    /// between that container's last unit and whatever follows, and cannot begin a part.
    cuttable: bool,
}

fn is_block(dom: &Dom, node: NodeId, in_dl: bool) -> bool {
    let Some(name) = dom.name(node) else {
        return false;
    };
    if name.ns != Ns::Html {
        return false;
    }
    if in_dl {
        // A term and its definitions stay together.
        return name.local == "dt";
    }
    BLOCKS.contains(&name.local.as_str())
}

fn is_container(dom: &Dom, node: NodeId) -> bool {
    dom.name(node).is_some_and(|name| {
        name.ns == Ns::Html
            && CONTAINERS.contains(&name.local.as_str())
            // A reversed list counts down from its length, which a part cannot know.
            && !(name.local == "ol" && dom.attr(node, "reversed").is_some())
    })
}

/// The CFI step of child `index` of `parent`: even for elements, counted among elements only.
fn element_step(dom: &Dom, parent: NodeId, child: NodeId) -> u32 {
    let position = dom
        .children(parent)
        .iter()
        .filter(|&&node| dom.is_element(node))
        .position(|&node| node == child)
        .expect("a child of its parent");
    2 * (position as u32 + 1)
}

struct Units<'a> {
    dom: &'a Dom,
    sizes: &'a [usize],
    /// A unit larger than this is opened up if it can be.
    fits: usize,
    units: Vec<Unit>,
}

impl Units<'_> {
    /// Splits `container`'s children into units, opening those too large to fit.
    fn collect(&mut self, container: NodeId, chain: &[NodeId], path: &[u32]) {
        let dom = self.dom;
        let in_dl = dom.is_html(container, "dl");
        let children = dom.children(container);
        let mut groups: Vec<Vec<NodeId>> = Vec::new();
        for &child in children {
            if is_block(dom, child, in_dl) || groups.is_empty() {
                groups.push(Vec::new());
            }
            groups.last_mut().expect("a group").push(child);
        }
        for group in groups {
            let size: usize = group.iter().map(|&node| self.sizes[node]).sum();
            let head = group[0];
            let starts_with_block = is_block(dom, head, in_dl);
            let start = if starts_with_block {
                let mut start = path.to_vec();
                start.push(element_step(dom, container, head));
                start
            } else {
                // Inline content at the start of a container starts where the container does.
                path.to_vec()
            };
            if size > self.fits && starts_with_block && is_container(dom, head) {
                let mut inner_chain = chain.to_vec();
                inner_chain.push(head);
                let before = self.units.len();
                self.collect(head, &inner_chain, &start);
                if self.units.len() > before {
                    // Whatever follows the container in its group (white space, inline content)
                    // goes after it, with the unit before.
                    if group.len() > 1 {
                        self.units.push(Unit {
                            chain: chain.to_vec(),
                            nodes: group[1..].to_vec(),
                            size: group[1..].iter().map(|&node| self.sizes[node]).sum(),
                            start: Vec::new(),
                            cuttable: false,
                        });
                    }
                    continue;
                }
            }
            self.units.push(Unit {
                chain: chain.to_vec(),
                nodes: group,
                size,
                start,
                cuttable: true,
            });
        }
    }
}

/// The size of an element's own tags, without its children.
fn tag_size(dom: &Dom, sizes: &[usize], node: NodeId) -> usize {
    let children: usize = dom.children(node).iter().map(|&child| sizes[child]).sum();
    sizes[node].saturating_sub(children)
}

/// Cuts `dom` into parts of at most `budget` bytes, between blocks. A document that fits, or that
/// is not XHTML with a `<body>`, comes back whole, as one part.
pub fn split(dom: &Dom, budget: usize) -> Vec<Part> {
    let whole = || {
        vec![Part {
            dom: dom.clone(),
            start: Vec::new(),
        }]
    };
    let Some(body) = dom.body() else {
        return whole();
    };
    let sizes = serialize::sizes(dom);
    let total = serialize::xhtml_len(dom);
    if total <= budget {
        return whole();
    }
    let content: usize = dom.children(body).iter().map(|&child| sizes[child]).sum();
    let overhead = total - content;
    let room = budget.saturating_sub(overhead + MARGIN).max(1);
    let mut collector = Units {
        dom,
        sizes: &sizes,
        fits: room,
        units: Vec::new(),
    };
    collector.collect(body, &[], &[]);
    let units = collector.units;

    // Balanced: as many parts as the budget needs, each near the same size.
    let count = content.div_ceil(room).max(1);
    let target = content / count;
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut current = 0usize;
    let mut open_chain: Vec<NodeId> = Vec::new();
    for (index, unit) in units.iter().enumerate() {
        // Containers this unit would open in the current part.
        let shared = open_chain
            .iter()
            .zip(&unit.chain)
            .take_while(|(a, b)| a == b)
            .count();
        let wrappers: usize = unit.chain[shared..]
            .iter()
            .map(|&node| tag_size(dom, &sizes, node))
            .sum();
        let exceeds = current + wrappers + unit.size > room;
        if groups.is_empty() || (unit.cuttable && current > 0 && (exceeds || current >= target)) {
            groups.push(Vec::new());
            current = unit
                .chain
                .iter()
                .map(|&node| tag_size(dom, &sizes, node))
                .sum();
            open_chain = unit.chain.clone();
        } else {
            current += wrappers;
            open_chain = unit.chain.clone();
        }
        current += unit.size;
        groups.last_mut().expect("a part").push(index);
    }
    if groups.len() <= 1 {
        return whole();
    }
    groups
        .iter()
        .enumerate()
        .map(|(number, members)| build(dom, body, &units, members, number == 0))
        .collect()
}

/// Builds one part from its units.
fn build(dom: &Dom, body: NodeId, units: &[Unit], members: &[usize], first: bool) -> Part {
    let mut part = Dom::new();
    let root = dom.root_element().expect("a root");
    let html = dom.shallow_copy_into(root, &mut part);
    part.append(Dom::DOCUMENT, html);
    for &child in dom.children(root) {
        if child == body {
            break;
        }
        let copy = dom.copy_into(child, &mut part);
        part.append(html, copy);
    }
    let body_copy = dom.shallow_copy_into(body, &mut part);
    part.append(html, body_copy);
    // What comes after body under html, which is only ever white space.
    let start = if first {
        Vec::new()
    } else {
        units[members[0]].start.clone()
    };

    let mut open: Vec<(NodeId, NodeId)> = Vec::new();
    for &index in members {
        let unit = &units[index];
        let shared = open
            .iter()
            .zip(&unit.chain)
            .take_while(|((original, _), chained)| original == *chained)
            .count();
        open.truncate(shared);
        for &container in &unit.chain[shared..] {
            let parent = open.last().map_or(body_copy, |(_, copy)| *copy);
            let copy = dom.shallow_copy_into(container, &mut part);
            part.append(parent, copy);
            open.push((container, copy));
        }
        let parent = open.last().map_or(body_copy, |(_, copy)| *copy);
        for &node in &unit.nodes {
            let copy = dom.copy_into(node, &mut part);
            part.append(parent, copy);
        }
    }
    continue_lists(dom, &mut part, body, body_copy, &start);
    part.normalize_text(Dom::DOCUMENT);
    Part { dom: part, start }
}

/// A list that this part enters in the middle keeps counting where the part before left off.
fn continue_lists(dom: &Dom, part: &mut Dom, body: NodeId, body_copy: NodeId, start: &[u32]) {
    let mut original = body;
    let mut copy = body_copy;
    for &step in start {
        let index = (step / 2) as usize - 1;
        let Some(child) = dom.element_children(original).nth(index) else {
            return;
        };
        if dom.is_html(original, "ol") {
            let before = dom
                .element_children(original)
                .take(index)
                .filter(|&item| dom.is_html(item, "li"))
                .count() as i64;
            let first: i64 = dom
                .attr(original, "start")
                .and_then(|value| value.trim().parse().ok())
                .unwrap_or(1);
            part.set_attr(copy, Name::plain("start"), (first + before).to_string());
        }
        let Some(next) = part.element_children(copy).next() else {
            return;
        };
        original = child;
        copy = next;
    }
}

/// A path in the whole document (CFI steps below `<body>`) as a path in the part that starts at
/// `start`. The path must be in that part.
pub fn to_part_path(start: &[u32], path: &[u32]) -> Vec<u32> {
    let mut translated = Vec::with_capacity(path.len());
    let mut on_start = true;
    for (level, &step) in path.iter().enumerate() {
        match start.get(level) {
            Some(&first) if on_start => {
                let skipped = first - 2;
                translated.push(step - skipped);
                on_start = step == first;
            }
            _ => {
                on_start = false;
                translated.push(step);
            }
        }
    }
    translated
}

/// The inverse of [`to_part_path`].
pub fn from_part_path(start: &[u32], path: &[u32]) -> Vec<u32> {
    let mut translated = Vec::with_capacity(path.len());
    let mut on_start = true;
    for (level, &step) in path.iter().enumerate() {
        match start.get(level) {
            Some(&first) if on_start => {
                let skipped = first - 2;
                translated.push(step + skipped);
                on_start = step == 2;
            }
            _ => {
                on_start = false;
                translated.push(step);
            }
        }
    }
    translated
}

/// Which of the parts, given by where each starts, holds the node at `path` (CFI steps below
/// `<body>`): the last that starts at or before it in document order.
pub fn part_of(starts: &[Vec<u32>], path: &[u32]) -> usize {
    let mut found = 0;
    for (index, start) in starts.iter().enumerate() {
        if index == 0 || !precedes(path, start) {
            found = index;
        } else {
            break;
        }
    }
    found
}

/// Whether `a` comes strictly before `b` in document order. An ancestor comes before what it holds.
fn precedes(a: &[u32], b: &[u32]) -> bool {
    for (x, y) in a.iter().zip(b) {
        if x != y {
            return x < y;
        }
    }
    a.len() < b.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serialize::to_xhtml;
    use crate::xml::{Limits, parse};

    fn paragraphs(count: usize, words: usize) -> String {
        (0..count)
            .map(|n| format!("<p id='p{n}'>{}</p>\n", format!("word{n} ").repeat(words)))
            .collect()
    }

    fn doc(body: &str) -> Dom {
        parse(
            &format!(
                "<html xmlns='http://www.w3.org/1999/xhtml'><head><title>t</title></head><body class='b'>{body}</body></html>"
            ),
            &Limits::default(),
        )
        .unwrap()
    }

    #[test]
    fn a_small_document_stays_whole() {
        let dom = doc(&paragraphs(10, 10));
        let parts = split(&dom, PART_BUDGET);
        assert_eq!(parts.len(), 1);
        assert!(parts[0].start.is_empty());
    }

    #[test]
    fn a_large_document_is_cut_between_paragraphs_into_balanced_parts() {
        let dom = doc(&format!(
            "<div class='chapter' id='c'>{}</div>",
            paragraphs(3000, 60)
        ));
        let total = to_xhtml(&dom).len();
        assert!(total > 1 << 20, "{total}");
        let parts = split(&dom, PART_BUDGET);
        assert!(
            parts.len() >= total.div_ceil(PART_BUDGET),
            "{}",
            parts.len()
        );
        let mut paragraphs_seen = 0;
        for part in &parts {
            let text = to_xhtml(&part.dom);
            assert!(text.len() <= PART_BUDGET, "{}", text.len());
            assert!(
                text.contains("<div class=\"chapter\" id=\"c\">"),
                "the container repeats"
            );
            assert!(text.contains("<body class=\"b\">"));
            let body = part.dom.body().unwrap();
            let chapter = part.dom.element_children(body).next().unwrap();
            paragraphs_seen += part.dom.element_children(chapter).count();
        }
        assert_eq!(paragraphs_seen, 3000, "nothing lost, nothing repeated");
        let sizes: Vec<usize> = parts.iter().map(|part| to_xhtml(&part.dom).len()).collect();
        let smallest = *sizes.iter().min().unwrap();
        let largest = *sizes.iter().max().unwrap();
        assert!(smallest * 2 > largest, "balanced: {sizes:?}");
    }

    #[test]
    fn paths_move_between_the_document_and_its_parts_exactly() {
        let dom = doc(&format!(
            "<h1>Title</h1>\n<div class='chapter'>intro {}</div>\n<p>after</p>",
            paragraphs(1500, 60)
        ));
        let parts = split(&dom, PART_BUDGET);
        assert!(parts.len() > 2);
        let starts: Vec<Vec<u32>> = parts.iter().map(|part| part.start.clone()).collect();
        let body = dom.body().unwrap();
        // Every element of the whole document, found in its part by path, is the same element.
        let mut checked = 0;
        for node in dom.descendants(body) {
            if node == body || !dom.is_element(node) {
                continue;
            }
            let mut path = Vec::new();
            let mut current = node;
            while current != body {
                let parent = dom.parent(current).unwrap();
                path.push(element_step(&dom, parent, current));
                current = parent;
            }
            path.reverse();
            let index = part_of(&starts, &path);
            let part = &parts[index];
            let inner = to_part_path(&part.start, &path);
            assert_eq!(from_part_path(&part.start, &inner), path);
            let mut found = part.dom.body().unwrap();
            for step in &inner {
                found = part
                    .dom
                    .element_children(found)
                    .nth((*step / 2 - 1) as usize)
                    .unwrap_or_else(|| panic!("{path:?} -> {inner:?} in part {index}"));
            }
            assert_eq!(part.dom.name(found), dom.name(node), "{path:?}");
            assert_eq!(part.dom.attr(found, "id"), dom.attr(node, "id"), "{path:?}");
            // A repeated container holds only its share of the text in each part; a paragraph is whole.
            if dom.is_html(node, "p") {
                assert!(
                    part.dom.text_content(found) == dom.text_content(node),
                    "{path:?}"
                );
            }
            checked += 1;
        }
        assert_eq!(checked, 1503);
    }

    #[test]
    fn a_numbered_list_keeps_counting_and_a_table_is_never_cut() {
        let items: String = (0..4000)
            .map(|n| format!("<li>item {n} {}</li>", "x".repeat(60)))
            .collect();
        let rows: String = (0..3000)
            .map(|n| format!("<tr><td>{n} {}</td></tr>", "y".repeat(80)))
            .collect();
        let dom = doc(&format!("<ol start='5'>{items}</ol><table>{rows}</table>"));
        let parts = split(&dom, PART_BUDGET);
        let mut next_number = 5;
        let mut tables = 0;
        for part in &parts {
            let body = part.dom.body().unwrap();
            for child in part.dom.element_children(body) {
                if part.dom.is_html(child, "ol") {
                    let start: usize = part.dom.attr(child, "start").unwrap().parse().unwrap();
                    assert_eq!(start, next_number);
                    next_number += part.dom.element_children(child).count();
                }
                if part.dom.is_html(child, "table") {
                    tables += 1;
                }
            }
        }
        assert_eq!(next_number, 4005);
        assert_eq!(tables, 1, "the table is in one part, too large as it is");
    }
}

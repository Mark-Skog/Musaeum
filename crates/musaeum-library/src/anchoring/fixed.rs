//! Anchoring on fixed layouts with a text layer: PDF, and DjVu with OCR.
//!
//! A segment is a stretch of characters on one page. It keeps three selectors, tried in this order:
//!
//! 1. the range of characters in the page's character layer (`page_char_range`);
//! 2. the quads those characters covered, in the unrotated crop box (`page_quads`), which find the
//!    same glyphs when the text layer counts its characters differently, at any zoom and rotation;
//! 3. the quote with what comes before and after it (`text_quote`), on its page first, then on the
//!    pages around it, then anywhere in the document.
//!
//! Each candidate is checked against the quote: a range that now holds other text is not the
//! annotation's. What none of them finds is an orphan.

use std::sync::Arc;

use musaeum_shm::chars::{chars_in_quads, line_quads};
use musaeum_shm::{CHARS_VERSION, CharLayer};
use serde_json::{Value, json};

use super::text::{self, CONTEXT_CHARS, Folded, MIN_SIMILARITY, Quality, Quote, collapse, fold};
use crate::annotations::{AnchorState, Segment, Selector, SelectorKind};
use crate::model::RevisionId;

/// The pages of the document as it is now.
pub trait Pages {
    /// How many pages there are.
    fn count(&self) -> u32;
    /// A page's characters, or `None` if they cannot be read.
    fn layer(&mut self, page: u32) -> Option<Arc<CharLayer>>;
}

/// Characters `start..end` of a page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageRange {
    pub page: u32,
    pub start: usize,
    pub end: usize,
}

/// Where a segment is now.
#[derive(Clone, Debug, PartialEq)]
pub struct FixedPlace {
    pub range: PageRange,
    /// What a mark is drawn with.
    pub quads: Vec<[f32; 8]>,
    pub state: AnchorState,
}

/// The byte in the page's text of each character, and the text's length at the end.
fn char_bytes(text: &str) -> Vec<usize> {
    let mut bytes: Vec<usize> = text.char_indices().map(|(at, _)| at).collect();
    bytes.push(text.len());
    bytes
}

/// The characters `start..end` of a layer as text.
fn text_of(layer: &CharLayer, start: usize, end: usize) -> String {
    layer.chars[start.min(layer.chars.len())..end.min(layer.chars.len())]
        .iter()
        .map(|glyph| glyph.ch)
        .collect()
}

/// What characters `start..end` of a page quote, with what comes before and after them.
pub fn quote_at(layer: &CharLayer, start: usize, end: usize) -> Quote {
    let before = collapse(&text_of(
        layer,
        start.saturating_sub(CONTEXT_CHARS * 4),
        start,
    ));
    let after = collapse(&text_of(layer, end, end + CONTEXT_CHARS * 4));
    Quote {
        exact: collapse(&text_of(layer, start, end)),
        prefix: text::tail_chars(&before, CONTEXT_CHARS),
        suffix: text::head_chars(&after, CONTEXT_CHARS),
    }
}

/// The quote selector of a quote.
pub fn quote_selector(quote: &Quote) -> Selector {
    Selector {
        kind: SelectorKind::TextQuote,
        data: json!({"exact": quote.exact, "prefix": quote.prefix, "suffix": quote.suffix}),
        priority: 2,
        normalizer_version: None,
        revision_id: None,
    }
}

/// The selectors that say where a range is on this revision, the quote aside.
pub fn place_selectors(
    range: PageRange,
    quads: &[[f32; 8]],
    revision: RevisionId,
) -> Vec<Selector> {
    vec![
        Selector {
            kind: SelectorKind::PageCharRange,
            data: json!({"page": range.page, "start": range.start, "end": range.end}),
            priority: 0,
            normalizer_version: Some(CHARS_VERSION),
            revision_id: Some(revision),
        },
        Selector {
            kind: SelectorKind::PageQuads,
            data: json!({"page": range.page, "quads": quads}),
            priority: 1,
            normalizer_version: Some(CHARS_VERSION),
            revision_id: Some(revision),
        },
    ]
}

/// A new segment for characters the reader selected, with the quote it makes.
pub fn segment_for(
    ordinal: u32,
    range: PageRange,
    layer: &CharLayer,
    revision: RevisionId,
) -> (Segment, Quote) {
    let quote = quote_at(layer, range.start, range.end);
    let quads = line_quads(&layer.chars, range.start, range.end);
    let mut selectors = place_selectors(range, &quads, revision);
    selectors.push(quote_selector(&quote));
    (
        Segment {
            ordinal,
            page: Some(range.page),
            href: None,
            clip: None,
            anchor_state: AnchorState::Exact,
            selectors,
        },
        quote,
    )
}

fn selector(segment: &Segment, kind: SelectorKind) -> Option<&Selector> {
    segment
        .selectors
        .iter()
        .find(|selector| selector.kind == kind)
}

fn number(data: &Value, key: &str) -> Option<u64> {
    data.get(key)?.as_u64()
}

/// The quote a segment keeps, if it has one.
pub fn quote_of(segment: &Segment) -> Option<Quote> {
    let data = &selector(segment, SelectorKind::TextQuote)?.data;
    let field = |key: &str| {
        data.get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned()
    };
    let quote = Quote {
        exact: field("exact"),
        prefix: field("prefix"),
        suffix: field("suffix"),
    };
    (!quote.exact.is_empty()).then_some(quote)
}

fn quads_of(data: &Value) -> Vec<[f32; 8]> {
    data.get("quads")
        .and_then(Value::as_array)
        .map(|quads| {
            quads
                .iter()
                .filter_map(|quad| {
                    let values: Vec<f32> = quad
                        .as_array()?
                        .iter()
                        .filter_map(|value| value.as_f64().map(|v| v as f32))
                        .collect();
                    values.try_into().ok()
                })
                .collect()
        })
        .unwrap_or_default()
}

fn place(layer: &CharLayer, page: u32, start: usize, end: usize, state: AnchorState) -> FixedPlace {
    FixedPlace {
        range: PageRange { page, start, end },
        quads: line_quads(&layer.chars, start, end),
        state,
    }
}

/// Whether characters `start..end` of a layer say what the quote says. With no quote to compare,
/// only a range on the same bytes and the same text layer version is trusted.
fn holds(
    layer: &CharLayer,
    start: usize,
    end: usize,
    quote: Option<&Quote>,
    trusted: bool,
) -> bool {
    if start >= end || end > layer.chars.len() {
        return false;
    }
    match quote {
        Some(quote) => fold(&text_of(layer, start, end)) == fold(&quote.exact),
        None => trusted,
    }
}

/// Finds a quote on one page, the edit search only if asked.
fn find_on(
    layer: &CharLayer,
    quote: &Quote,
    hint: Option<usize>,
    edits: bool,
) -> Option<(usize, usize, Quality)> {
    let text = layer.text();
    let bytes = char_bytes(&text);
    let folded = Folded::new(&text);
    let hint = hint.and_then(|index| bytes.get(index).copied());
    let (from, to, quality) = folded.find(quote, hint)?;
    if !edits && fold(&text[from..to]) != fold(&quote.exact) {
        return None;
    }
    let start = bytes.partition_point(|&byte| byte < from);
    let end = bytes.partition_point(|&byte| byte < to);
    Some((start, end, quality))
}

/// Where a segment is on the document as it is now, if anywhere. `revision` is the revision the
/// document is now; selectors worked out on it, with this text layer, are taken at their word.
pub fn resolve(
    segment: &Segment,
    pages: &mut dyn Pages,
    revision: RevisionId,
) -> Option<FixedPlace> {
    let quote = quote_of(segment);
    let count = pages.count();
    let current = |selector: &Selector| {
        selector.revision_id == Some(revision) && selector.normalizer_version == Some(CHARS_VERSION)
    };

    // 1. The range of characters.
    let ranged = selector(segment, SelectorKind::PageCharRange).and_then(|selector| {
        let page = u32::try_from(number(&selector.data, "page")?).ok()?;
        let start = usize::try_from(number(&selector.data, "start")?).ok()?;
        let end = usize::try_from(number(&selector.data, "end")?).ok()?;
        Some((page, start, end, current(selector)))
    });
    if let Some((page, start, end, trusted)) = ranged
        && page < count
        && let Some(layer) = pages.layer(page)
        && holds(&layer, start, end, quote.as_ref(), trusted)
    {
        return Some(place(&layer, page, start, end, AnchorState::Exact));
    }

    // 2. The glyphs under the quads.
    let quadded = selector(segment, SelectorKind::PageQuads).and_then(|selector| {
        let page = u32::try_from(number(&selector.data, "page")?).ok()?;
        Some((page, quads_of(&selector.data)))
    });
    let mut near: Option<FixedPlace> = None;
    if let Some((page, quads)) = &quadded
        && *page < count
        && !quads.is_empty()
        && let Some(layer) = pages.layer(*page)
        && let Some((start, end)) = chars_in_quads(&layer.chars, quads)
    {
        if holds(&layer, start, end, quote.as_ref(), false) {
            return Some(place(&layer, *page, start, end, AnchorState::Exact));
        }
        let similar = quote.as_ref().is_some_and(|quote| {
            text::similarity(&fold(&text_of(&layer, start, end)), &fold(&quote.exact))
                >= MIN_SIMILARITY
        });
        if similar {
            near = Some(place(&layer, *page, start, end, AnchorState::Approximate));
        }
    }

    // 3. The quote: on its page, then the pages around it, then everywhere.
    let Some(quote) = quote else {
        return near;
    };
    let home = ranged
        .map(|(page, ..)| page)
        .or(quadded.as_ref().map(|(page, _)| *page))
        .or(segment.page)
        .unwrap_or(0)
        .min(count.saturating_sub(1));
    let hint = ranged.map(|(_, start, ..)| start);
    let mut order: Vec<u32> = Vec::with_capacity(count as usize);
    order.push(home);
    for step in 1..count {
        if let Some(after) = home.checked_add(step).filter(|page| *page < count) {
            order.push(after);
        }
        if let Some(before) = home.checked_sub(step) {
            order.push(before);
        }
        if home + step >= count && home < step {
            break;
        }
    }
    // As it was, anywhere, nearest first; only then edited, and only near where it was.
    for &page in &order {
        let Some(layer) = pages.layer(page) else {
            continue;
        };
        let page_hint = (page == home).then_some(hint).flatten();
        if let Some((start, end, quality)) = find_on(&layer, &quote, page_hint, false) {
            let state = match quality {
                Quality::Exact => AnchorState::Exact,
                Quality::Approximate => AnchorState::Approximate,
            };
            return Some(place(&layer, page, start, end, state));
        }
    }
    if let Some(found) = near {
        return Some(found);
    }
    for &page in order.iter().take(3) {
        let Some(layer) = pages.layer(page) else {
            continue;
        };
        let page_hint = (page == home).then_some(hint).flatten();
        if let Some((start, end, _)) = find_on(&layer, &quote, page_hint, true) {
            return Some(place(&layer, page, start, end, AnchorState::Approximate));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use musaeum_shm::Glyph;
    use musaeum_shm::chars::{GENERATED, LINE_END, box_quad};

    use super::*;

    /// A page of lines, each character 1/100 of the width and each line 1/20 of the height.
    fn page(lines: &[&str]) -> CharLayer {
        let mut chars = Vec::new();
        for (row, line) in lines.iter().enumerate() {
            let top = 0.05 + row as f32 * 0.05;
            for (column, ch) in line.chars().enumerate() {
                let left = 0.05 + column as f32 * 0.01;
                chars.push(Glyph {
                    ch,
                    quad: if ch == ' ' {
                        [0.0; 8]
                    } else {
                        box_quad([left, top, left + 0.01, top + 0.03])
                    },
                    flags: if ch == ' ' { GENERATED } else { 0 },
                });
            }
            chars.push(Glyph {
                ch: '\n',
                quad: [0.0; 8],
                flags: GENERATED | LINE_END,
            });
        }
        CharLayer { rotation: 0, chars }
    }

    struct Book(HashMap<u32, Arc<CharLayer>>, u32, usize);

    impl Pages for Book {
        fn count(&self) -> u32 {
            self.1
        }
        fn layer(&mut self, page: u32) -> Option<Arc<CharLayer>> {
            self.2 += 1;
            self.0.get(&page).cloned()
        }
    }

    fn book(pages: impl IntoIterator<Item = CharLayer>) -> Book {
        let pages: Vec<CharLayer> = pages.into_iter().collect();
        let count = pages.len() as u32;
        Book(
            pages
                .into_iter()
                .enumerate()
                .map(|(i, p)| (i as u32, Arc::new(p)))
                .collect(),
            count,
            0,
        )
    }

    const FIRST: [&str; 3] = [
        "Era uma vez um leitor que marcava tudo",
        "o que lia, para nunca mais perder",
        "o fio de cada pensamento.",
    ];

    fn marked(layer: &CharLayer, needle: &str) -> Segment {
        let text = layer.text();
        let byte = text.find(needle).unwrap();
        let start = text[..byte].chars().count();
        let end = start + needle.chars().count();
        segment_for(
            0,
            PageRange {
                page: 1,
                start,
                end,
            },
            layer,
            7,
        )
        .0
    }

    #[test]
    fn on_the_same_revision_the_range_is_enough() {
        let pages = [page(&["capa"]), page(&FIRST)];
        let segment = marked(&pages[1], "nunca mais perder");
        let mut book = book(pages);
        let found = resolve(&segment, &mut book, 7).unwrap();
        assert_eq!(found.state, AnchorState::Exact);
        assert_eq!(
            text_of(&book.0[&1], found.range.start, found.range.end),
            "nunca mais perder"
        );
        assert_eq!(found.quads.len(), 1);
        assert_eq!(book.2, 1, "only its page was read");
    }

    #[test]
    fn a_quote_across_a_line_is_one_segment_with_a_quad_per_line() {
        let pages = [page(&["capa"]), page(&FIRST)];
        let segment = marked(&pages[1], "tudo\no que lia");
        let found = resolve(&segment, &mut book(pages), 7).unwrap();
        assert_eq!(found.quads.len(), 2);
        assert_eq!(quote_of(&segment).unwrap().exact, "tudo o que lia");
    }

    #[test]
    fn characters_counted_differently_are_found_by_their_quads() {
        let pages = [page(&["capa"]), page(&FIRST)];
        let segment = marked(&pages[1], "nunca mais perder");
        // The new revision's text layer has no made-up spaces: every index after them moves.
        let mut squeezed = page(&FIRST);
        squeezed
            .chars
            .retain(|glyph| !glyph.is_generated() || glyph.ends_line());
        let found = resolve(
            &segment,
            &mut book(vec![page(&["capa"]), squeezed.clone()]),
            8,
        );
        // The glyphs are found, but without spaces their text is not the quote's; the quote search
        // on the page cannot find "nunca mais perder" either, so it is the quads' near match.
        let found = found.unwrap();
        assert_eq!(found.state, AnchorState::Approximate);
        assert_eq!(
            text_of(&squeezed, found.range.start, found.range.end),
            "nuncamaisperder"
        );
    }

    #[test]
    fn a_page_inserted_before_moves_the_mark_to_the_next_page() {
        let pages = [page(&["capa"]), page(&FIRST)];
        let segment = marked(&pages[1], "nunca mais perder");
        let newer = vec![
            page(&["capa"]),
            page(&["Prefácio novo", "em duas linhas"]),
            page(&FIRST),
        ];
        let found = resolve(&segment, &mut book(newer), 8).unwrap();
        assert_eq!(found.range.page, 2);
        assert_eq!(found.state, AnchorState::Exact);
    }

    #[test]
    fn a_light_edit_near_the_page_is_approximate() {
        let pages = [page(&["capa"]), page(&FIRST)];
        let segment = marked(&pages[1], "para nunca mais perder");
        let edited = vec![
            page(&["capa"]),
            page(&[
                "Era uma vez um leitor que marcava tudo",
                "o que lia, para jamais perder",
                "o fio.",
            ]),
        ];
        let mut edited_book = book(edited);
        let found = resolve(&segment, &mut edited_book, 8).unwrap();
        assert_eq!(found.state, AnchorState::Approximate);
        assert_eq!(
            text_of(&edited_book.0[&1], found.range.start, found.range.end),
            "para jamais perder"
        );
    }

    #[test]
    fn a_removed_page_leaves_an_orphan() {
        let pages = [page(&["capa"]), page(&FIRST)];
        let segment = marked(&pages[1], "nunca mais perder");
        let without = vec![
            page(&["capa"]),
            page(&["Outra coisa inteiramente diferente aqui."]),
        ];
        assert_eq!(resolve(&segment, &mut book(without), 8), None);
        assert_eq!(resolve(&segment, &mut book(vec![page(&["capa"])]), 8), None);
    }
}

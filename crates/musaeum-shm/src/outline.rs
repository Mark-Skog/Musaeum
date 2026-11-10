//! A document's outline (a PDF's bookmarks, its table of contents) on its way from a worker to the
//! host.
//!
//! It travels the way a text layer does, written into a slot, because an outline can be far larger
//! than a control message: as the text layer of a page that does not exist, [`OUTLINE_PAGE`]. Each
//! entry is one word, whose text is its level, its page (`-` for an entry that leads nowhere) and
//! its title, separated by single spaces. The host decodes it with every check a text layer gets,
//! then checks the levels and the pages against the document.

use crate::text::{TextLayer, Word};

/// The page whose text layer is the outline.
pub const OUTLINE_PAGE: u32 = u32::MAX;
/// Most entries an outline keeps.
pub const MAX_OUTLINE_ENTRIES: usize = 10_000;
/// Deepest nesting kept; deeper entries are listed at this level.
pub const MAX_OUTLINE_DEPTH: u16 = 16;
/// Longest title kept, in bytes of UTF-8.
pub const MAX_TITLE_BYTES: usize = 300;
/// Most bytes the encoded outline may take, well inside one slot.
const MAX_ENCODED: usize = 768 << 10;

/// One entry of an outline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutlineEntry {
    /// 0 for the top level.
    pub level: u16,
    /// The zero-based page it leads to, if it leads to one.
    pub page: Option<u32>,
    pub title: String,
}

/// A title as it is kept: white space folded, control characters gone, at most
/// [`MAX_TITLE_BYTES`], never empty.
pub fn clean_title(raw: &str) -> String {
    let mut title = String::new();
    for word in raw
        .split(|character: char| character.is_whitespace() || character.is_control())
        .filter(|word| !word.is_empty())
    {
        if !title.is_empty() {
            title.push(' ');
        }
        title.push_str(word);
        if title.len() > MAX_TITLE_BYTES {
            break;
        }
    }
    if title.len() > MAX_TITLE_BYTES {
        let mut end = MAX_TITLE_BYTES;
        while !title.is_char_boundary(end) {
            end -= 1;
        }
        title.truncate(end);
        title.push('…');
    }
    if title.is_empty() {
        title.push('—');
    }
    title
}

/// The outline as a text layer, cut where it would no longer fit a slot.
pub fn to_text_layer(entries: &[OutlineEntry]) -> TextLayer {
    let mut words = Vec::new();
    let mut size = 16;
    for entry in entries.iter().take(MAX_OUTLINE_ENTRIES) {
        let page = entry
            .page
            .map_or_else(|| "-".to_owned(), |page| page.to_string());
        let text = format!(
            "{} {page} {}",
            entry.level.min(MAX_OUTLINE_DEPTH),
            clean_title(&entry.title)
        );
        size += 24 + text.len();
        if size > MAX_ENCODED {
            break;
        }
        words.push(Word {
            text,
            rect: [0.0; 4],
            line_end: false,
        });
    }
    TextLayer { words }
}

/// The outline a decoded text layer carries, checked against a document of `page_count` pages: a
/// level out of range or a page the document does not have is dropped, never trusted.
pub fn from_text_layer(layer: &TextLayer, page_count: u32) -> Vec<OutlineEntry> {
    layer
        .words
        .iter()
        .take(MAX_OUTLINE_ENTRIES)
        .map(|word| {
            let mut fields = word.text.splitn(3, ' ');
            let level = fields
                .next()
                .and_then(|level| level.parse::<u16>().ok())
                .filter(|level| *level <= MAX_OUTLINE_DEPTH)
                .unwrap_or(0);
            let page = fields
                .next()
                .and_then(|page| page.parse::<u32>().ok())
                .filter(|page| *page < page_count);
            OutlineEntry {
                level,
                page,
                title: clean_title(fields.next().unwrap_or_default()),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(level: u16, page: Option<u32>, title: &str) -> OutlineEntry {
        OutlineEntry {
            level,
            page,
            title: title.to_owned(),
        }
    }

    #[test]
    fn an_outline_survives_the_trip_through_a_slot() {
        let outline = vec![
            entry(0, Some(0), "Capa"),
            entry(0, Some(4), "Capítulo 1 — Começo"),
            entry(1, Some(5), "1.1 Seção"),
            entry(1, None, "Sem destino"),
            entry(2, Some(16_000_000), "Página longe"),
        ];
        let layer = to_text_layer(&outline);
        let mut slot = vec![0u8; 1 << 20];
        let len = layer.encode_into(&mut slot).unwrap();
        let decoded = TextLayer::decode(&slot[..len]).unwrap();
        assert_eq!(from_text_layer(&decoded, 16_000_001), outline);
        // A page the document does not have leads nowhere.
        assert_eq!(from_text_layer(&decoded, 10)[4].page, None);
    }

    #[test]
    fn titles_are_cleaned_and_kept_short() {
        assert_eq!(clean_title("  Um\n\tdois\u{0}três  "), "Um dois três");
        assert_eq!(clean_title(""), "—");
        let long = clean_title(&"á".repeat(400));
        assert!(long.len() <= MAX_TITLE_BYTES + '…'.len_utf8());
        assert!(long.ends_with('…'));
    }

    #[test]
    fn what_a_worker_cannot_mean_is_dropped() {
        let word = |text: &str| Word {
            text: text.to_owned(),
            rect: [0.0; 4],
            line_end: false,
        };
        let layer = TextLayer {
            words: vec![
                word("99 2 nível"),
                word("-3 -7 negativo"),
                word("1 500 longe"),
                word("sem campos"),
            ],
        };
        let outline = from_text_layer(&layer, 100);
        assert_eq!(outline[0], entry(0, Some(2), "nível"));
        assert_eq!(outline[1], entry(0, None, "negativo"));
        assert_eq!(outline[2], entry(1, None, "longe"));
        assert_eq!(outline[3], entry(0, None, "—"));
    }

    #[test]
    fn a_huge_outline_is_cut_to_fit_a_slot() {
        let outline: Vec<OutlineEntry> = (0..50_000)
            .map(|index| entry(0, Some(index), &"x".repeat(200)))
            .collect();
        let layer = to_text_layer(&outline);
        assert!(layer.words.len() < outline.len());
        assert!(layer.encoded_len() <= MAX_ENCODED);
        let mut slot = vec![0u8; 1 << 20];
        assert!(layer.encode_into(&mut slot).is_some());
    }
}

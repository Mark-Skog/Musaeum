//! Study reports: what a person marked and wrote in their books, in the order the books are read,
//! grouped by the chapters of their tables of contents, as Markdown ([`markdown`]) and as a PDF made
//! to be printed and read ([`pdf`]).
//!
//! The report is made from a [`Report`], which says everything it shows: the works, their
//! annotations and where each one is, and when it was made. Nothing here reads a clock, a file or
//! the library, so the same report gives the same bytes every time, in both formats.
//!
//! A note is Markdown, read as the reader reads it ([`notes`]): what the reader shows as formatting
//! is formatting in the report too, and what it shows as text is text, escaped so it stays text.

pub mod font;
pub mod markdown;
pub mod notes;
pub mod order;
pub mod pdf;
pub mod text;
pub mod time;

use serde::{Deserialize, Serialize};

/// What a report holds: the works, in the order given, and when and by what it was made.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    /// When it was made, in Unix milliseconds.
    pub generated_at: i64,
    /// The offset of the person's clock from UTC then, in minutes, as the dates are shown.
    pub utc_offset_minutes: i32,
    /// What made it, as `Musaeum 0.0.1`.
    pub generator: String,
    pub works: Vec<WorkReport>,
}

/// Someone who made a work, and how.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Creator {
    pub name: String,
    /// `author`, `editor`, `translator`…
    pub role: String,
}

/// An identifier of a work, by its scheme: `isbn`, `doi`, `issn`…
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identifier {
    pub scheme: String,
    pub value: String,
}

/// An entry of a work's table of contents, where it starts in reading order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Heading {
    pub label: String,
    /// 0 for the top level.
    pub depth: u32,
    /// Where it starts, in the units of [`Item::at`].
    pub at: f64,
}

/// One work of a report and its annotations.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkReport {
    pub title: String,
    pub creators: Vec<Creator>,
    pub series: Option<String>,
    pub series_position: Option<f64>,
    pub year: Option<i64>,
    pub language: Option<String>,
    pub identifiers: Vec<Identifier>,
    /// The file's format, as its extension names it: `pdf`, `epub`…
    pub format: String,
    /// Whether its pages are fixed (PDF, DjVu, comics) or its text flows (EPUB and the others).
    pub layout: Layout,
    /// How many pages it has, for a fixed layout.
    pub pages: Option<u32>,
    /// Its table of contents, in its own order, every level flattened.
    pub contents: Vec<Heading>,
    /// Its annotations, in any order.
    pub items: Vec<Item>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    Fixed,
    Flow,
}

/// What kind of mark an annotation is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Highlight,
    Underline,
    Note,
}

/// How a mark was drawn: on the text, or on the picture of the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mark {
    /// Over characters of the text.
    Text,
    /// Along lines found on a scan.
    Lines,
    /// A box.
    Box,
    /// A stroke drawn by hand.
    Stroke,
    /// A note pinned at a point.
    Point,
    /// Over quads given from outside, with no text under them.
    Quads,
}

/// Whether an annotation is where it was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Found {
    Exact,
    Approximate,
    Orphan,
}

/// One annotation, as the report shows it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub uuid: String,
    pub kind: Kind,
    pub mark: Mark,
    /// A name of the reader's palette (`yellow`…) or `#rrggbb`, `#rrggbbaa`.
    pub color: String,
    /// The text it covers, as it was when it was made.
    pub quote: Option<String>,
    /// What the person wrote, in Markdown.
    pub note: String,
    /// Where it starts in reading order: for a fixed layout the page, from 0, plus how far down it
    /// the mark starts; for a flowing book the position of its first character in the book's whole
    /// text. `None` for a note about the whole book.
    pub at: Option<f64>,
    /// The page it is on, from 0, for a fixed layout.
    pub page: Option<u32>,
    /// How far into the book it starts, from 0 to 1.
    pub progression: Option<f64>,
    pub found: Found,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Kind {
    /// As the reader names it.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Highlight => "Realce",
            Kind::Underline => "Sublinhado",
            Kind::Note => "Nota",
        }
    }
}

impl Item {
    /// What the report calls it: its kind, or the shape it was drawn as.
    pub fn label(&self) -> &'static str {
        match self.mark {
            Mark::Box => "Caixa",
            Mark::Stroke => "Traço",
            Mark::Point => "Nota fixada",
            Mark::Text | Mark::Lines | Mark::Quads => self.kind.label(),
        }
    }

    /// Where it is, as a person reads it: the page of a fixed layout, how far into a flowing book.
    pub fn place(&self, layout: Layout) -> Option<String> {
        match (layout, self.page, self.progression) {
            (Layout::Fixed, Some(page), _) => Some(format!("p. {}", u64::from(page) + 1)),
            (_, _, Some(progression)) => Some(format!(
                "{} %",
                (progression.clamp(0.0, 1.0) * 100.0).round()
            )),
            _ => None,
        }
    }
}

/// The reader's palette: a name, its name in Portuguese, the color of its highlight and the color
/// its underline is drawn with, as 0–255 channels.
pub const PALETTE: [(&str, &str, [u8; 3], [u8; 3]); 5] = [
    ("yellow", "amarelo", [0xff, 0xe3, 0x6b], [217, 153, 0]),
    ("green", "verde", [0xa8, 0xe6, 0xa1], [33, 153, 51]),
    ("blue", "azul", [0x99, 0xc9, 0xfc], [26, 102, 217]),
    ("pink", "rosa", [0xfa, 0xad, 0xd6], [217, 51, 128]),
    ("purple", "roxo", [0xc7, 0xb5, 0xfc], [115, 64, 217]),
];

/// A color's name as a person reads it: the palette's in Portuguese, or the color as it is written.
pub fn color_name(color: &str) -> String {
    PALETTE
        .iter()
        .find(|(name, ..)| *name == color)
        .map_or_else(|| color.to_owned(), |(_, label, ..)| (*label).to_owned())
}

/// The color a mark is shown with on white paper: the palette's highlight, or its underline's ink,
/// or a color given as `#rrggbb[aa]` laid over white as much as its opacity says. Yellow for
/// anything else, as the reader does.
pub fn paint(color: &str, underline: bool) -> [u8; 3] {
    if let Some((_, _, fill, ink)) = PALETTE.iter().find(|(name, ..)| *name == color) {
        return if underline { *ink } else { *fill };
    }
    let hex = color.strip_prefix('#').filter(|digits| {
        (digits.len() == 6 || digits.len() == 8) && digits.bytes().all(|b| b.is_ascii_hexdigit())
    });
    let Some(hex) = hex else {
        return if underline {
            PALETTE[0].3
        } else {
            PALETTE[0].2
        };
    };
    let byte = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).unwrap_or(0);
    let alpha = if hex.len() == 8 {
        f64::from(byte(6)) / 255.0
    } else {
        1.0
    };
    let over_white = |channel: u8| (255.0 - alpha * (255.0 - f64::from(channel))).round() as u8;
    [
        over_white(byte(0)),
        over_white(byte(2)),
        over_white(byte(4)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_are_named_and_painted_as_the_reader_paints_them() {
        assert_eq!(color_name("yellow"), "amarelo");
        assert_eq!(color_name("#12ab34"), "#12ab34");
        assert_eq!(paint("blue", false), [0x99, 0xc9, 0xfc]);
        assert_eq!(paint("blue", true), [26, 102, 217]);
        assert_eq!(paint("#ff0000", false), [255, 0, 0]);
        assert_eq!(paint("#ff000080", false), [255, 127, 127]);
        assert_eq!(paint("nonsense", false), PALETTE[0].2);
    }

    #[test]
    fn places_are_pages_or_how_far_in() {
        let item = Item {
            uuid: String::new(),
            kind: Kind::Highlight,
            mark: Mark::Text,
            color: "yellow".into(),
            quote: None,
            note: String::new(),
            at: Some(11.2),
            page: Some(11),
            progression: Some(0.4249),
            found: Found::Exact,
            created_at: 0,
            updated_at: 0,
        };
        assert_eq!(item.place(Layout::Fixed).as_deref(), Some("p. 12"));
        assert_eq!(item.place(Layout::Flow).as_deref(), Some("42 %"));
    }
}

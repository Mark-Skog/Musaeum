//! A report as a PDF: A4 pages to print or read, set in the fonts built into the app, with real
//! text a reader can select, search and copy, in any language those fonts write.
//!
//! Each annotation is a block with a band down its left side in the color of its mark: the meta
//! line (what it is, its color, its page, when it was made), what the book says in a serif face,
//! and the note in a sans face with its Markdown set as the reader shows it. Headings stay with what
//! follows them; a short annotation is never cut across pages, and a long one leaves at least three
//! lines on each side. Every page has the work's title at the top and its number at the bottom;
//! the works and chapters are the PDF's bookmarks.
//!
//! The file is written here, object by object: fonts as `Type0` fonts over `CIDFontType2`
//! subsets with `Identity-H` and a `ToUnicode` map, so what is selected is the text that was set;
//! a character no font draws gets a blank glyph of its own, still mapped to itself. Nothing in it
//! depends on the clock or on where it is made: the same report gives the same bytes.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::Write as _;

use flate2::Compression;
use flate2::write::ZlibEncoder;

use crate::font::{Face, glyph_for};
use crate::markdown::{byline, counts, file_line, graphic, identifiers, meta};
use crate::notes::{self, Block as NoteBlock, Inline};
use crate::order::{self, Ordered};
use crate::text::{clean, fold_spaces};
use crate::{Found, Heading, Item, Kind, Report, WorkReport, paint, time};

const PAGE_W: f32 = 595.276;
const PAGE_H: f32 = 841.89;
const LEFT: f32 = 64.0;
const RIGHT: f32 = 64.0;
const TOP: f32 = 80.0;
const BOTTOM: f32 = 74.0;
const WIDTH: f32 = PAGE_W - LEFT - RIGHT;
/// The band beside an annotation, and how far its text is from the margin.
const BAND: f32 = 3.6;
const INDENT: f32 = 14.0;

const INK: [u8; 3] = [0x1c, 0x1c, 0x1c];
const SOFT: [u8; 3] = [0x5e, 0x5e, 0x5e];
const FAINT: [u8; 3] = [0x8a, 0x8a, 0x8a];
const RULE: [u8; 3] = [0xd6, 0xd4, 0xce];
const CODE_GROUND: [u8; 3] = [0xf2, 0xf1, 0xed];

#[derive(Clone, Copy, Debug, PartialEq)]
struct Style {
    face: Face,
    size: f32,
    color: [u8; 3],
}

const fn style(face: Face, size: f32, color: [u8; 3]) -> Style {
    Style { face, size, color }
}

/// A glyph set on a line: from which face, which glyph (`None`: a blank for a character no font
/// draws), the character it stands for, and how wide it is at its size.
#[derive(Clone, Debug)]
struct Glyph {
    face: Face,
    id: Option<u16>,
    c: char,
    size: f32,
    color: [u8; 3],
    width: f32,
}

fn shape(text: &str, style: Style) -> Vec<Glyph> {
    text.chars()
        .filter(|&c| c != '\n')
        .map(|c| {
            let c = if c == '\t' { ' ' } else { c };
            let (face, id) = glyph_for(style.face, c);
            let units = face.font().width(id.unwrap_or(0));
            Glyph {
                face,
                id,
                c,
                size: style.size,
                color: style.color,
                width: units as f32 * style.size / 1000.0,
            }
        })
        .collect()
}

fn width_of(glyphs: &[Glyph]) -> f32 {
    glyphs.iter().map(|glyph| glyph.width).sum()
}

/// Lines of at most `width`, broken at spaces; a word longer than a line is cut where it fills it.
fn wrap(glyphs: Vec<Glyph>, width: f32) -> Vec<Vec<Glyph>> {
    let mut lines = Vec::new();
    let mut line: Vec<Glyph> = Vec::new();
    let mut used = 0.0;
    let mut word: Vec<Glyph> = Vec::new();
    let flush_trailing = |line: &mut Vec<Glyph>| {
        while line.last().is_some_and(|glyph| glyph.c == ' ') {
            line.pop();
        }
    };
    let place_word = |word: &mut Vec<Glyph>,
                      line: &mut Vec<Glyph>,
                      used: &mut f32,
                      lines: &mut Vec<Vec<Glyph>>| {
        if word.is_empty() {
            return;
        }
        let needed = width_of(word);
        if !line.is_empty() && *used + needed > width + 0.01 {
            flush_trailing(line);
            lines.push(std::mem::take(line));
            *used = 0.0;
        }
        if line.is_empty() && needed > width {
            for glyph in word.drain(..) {
                if !line.is_empty() && *used + glyph.width > width + 0.01 {
                    lines.push(std::mem::take(line));
                    *used = 0.0;
                }
                *used += glyph.width;
                line.push(glyph);
            }
        } else {
            *used += needed;
            line.append(word);
        }
    };
    for glyph in glyphs {
        if glyph.c == ' ' {
            place_word(&mut word, &mut line, &mut used, &mut lines);
            if !line.is_empty() {
                used += glyph.width;
                line.push(glyph);
            }
        } else {
            word.push(glyph);
        }
    }
    place_word(&mut word, &mut line, &mut used, &mut lines);
    flush_trailing(&mut line);
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

/// Lines cut wherever they fill the width, spaces kept: code.
fn wrap_hard(glyphs: Vec<Glyph>, width: f32) -> Vec<Vec<Glyph>> {
    let mut lines = vec![Vec::new()];
    let mut used = 0.0;
    for glyph in glyphs {
        if used + glyph.width > width + 0.01 && lines.last().is_some_and(|line| !line.is_empty()) {
            lines.push(Vec::new());
            used = 0.0;
        }
        used += glyph.width;
        if let Some(line) = lines.last_mut() {
            line.push(glyph);
        }
    }
    lines
}

/// A filled rectangle, from the top of its line.
#[derive(Clone, Debug)]
struct Fill {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: [u8; 3],
}

/// A line of a block: how tall it is, where its baseline is from its top, and what is on it.
#[derive(Clone, Debug, Default)]
struct Line {
    height: f32,
    baseline: f32,
    runs: Vec<(f32, Vec<Glyph>)>,
    fills: Vec<Fill>,
}

impl Line {
    fn text(x: f32, glyphs: Vec<Glyph>, size: f32, leading: f32) -> Self {
        Line {
            height: leading,
            baseline: leading / 2.0 + size * 0.36,
            runs: vec![(x, glyphs)],
            fills: Vec::new(),
        }
    }

    fn space(height: f32) -> Self {
        Line {
            height,
            ..Line::default()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Keep {
    /// Never cut: moved whole to the next page if it does not fit.
    Together,
    /// Cut only leaving at least this many lines on each side.
    Lines(usize),
}

#[derive(Clone, Debug)]
struct Block {
    lines: Vec<Line>,
    /// Room above it, unless it starts a page.
    before: f32,
    keep: Keep,
    /// A heading: on the same page as the start of what follows.
    with_next: bool,
    /// The band down its left side.
    band: Option<[u8; 3]>,
    /// A bookmark to it: its level and title.
    bookmark: Option<(usize, String)>,
    /// Starts a new page.
    new_page: bool,
    /// The work it belongs to, for the title at the top of the page.
    work: Option<usize>,
}

impl Block {
    fn new(lines: Vec<Line>) -> Self {
        Block {
            lines,
            before: 0.0,
            keep: Keep::Together,
            with_next: false,
            band: None,
            bookmark: None,
            new_page: false,
            work: None,
        }
    }

    fn height(&self) -> f32 {
        self.lines.iter().map(|line| line.height).sum()
    }
}

/// Paragraph text set at `x`, wrapped to `width`.
fn paragraph(glyphs: Vec<Glyph>, x: f32, width: f32, size: f32, leading: f32) -> Vec<Line> {
    wrap(glyphs, width)
        .into_iter()
        .map(|line| Line::text(x, line, size, leading))
        .collect()
}

const NOTE: Style = style(Face::Sans, 9.4, [0x26, 0x26, 0x26]);
const NOTE_LEADING: f32 = 13.6;
const CODE: Style = style(Face::Mono, 8.3, [0x33, 0x33, 0x33]);
const CODE_LEADING: f32 = 11.8;

fn face_of(bold: bool, italic: bool) -> Face {
    match (bold, italic) {
        (false, false) => Face::Sans,
        (false, true) => Face::SansItalic,
        (true, false) => Face::SansBold,
        (true, true) => Face::SansBoldItalic,
    }
}

/// A note's inline pieces as glyphs, as the reader shows them: bold, italic, code, and a link as
/// its label with its address after it.
fn inline_glyphs(pieces: &[Inline], base: Style, bold: bool, italic: bool, out: &mut Vec<Glyph>) {
    for piece in pieces {
        match piece {
            Inline::Text(text) => out.extend(shape(
                &clean(text),
                Style {
                    face: face_of(bold, italic),
                    ..base
                },
            )),
            Inline::Code(code) => out.extend(shape(
                &clean(code),
                Style {
                    size: base.size - 0.6,
                    ..CODE
                },
            )),
            Inline::Strong(inner) => inline_glyphs(inner, base, true, italic, out),
            Inline::Emphasis(inner) => inline_glyphs(inner, base, bold, true, out),
            Inline::Link { label, href } => {
                out.extend(shape(
                    &clean(label),
                    Style {
                        face: face_of(bold, italic),
                        ..base
                    },
                ));
                out.extend(shape(
                    &format!(" ({})", clean(href)),
                    Style {
                        face: face_of(false, italic),
                        color: FAINT,
                        ..base
                    },
                ));
            }
        }
    }
}

fn note_text(text: &str, base: Style, bold: bool, italic: bool) -> Vec<Glyph> {
    let mut glyphs = Vec::new();
    inline_glyphs(
        &notes::inline(&clean(text)),
        base,
        bold,
        italic,
        &mut glyphs,
    );
    glyphs
}

/// A note's blocks as lines, from `x`, `width` wide.
fn note_lines(source: &str, x: f32, width: f32) -> Vec<Line> {
    let mut lines = Vec::new();
    for (index, block) in notes::blocks(source).into_iter().enumerate() {
        if index > 0 {
            lines.push(Line::space(4.0));
        }
        match block {
            NoteBlock::Heading { level, text } => {
                let size = if level <= 1 { 10.4 } else { 9.8 };
                let base = Style { size, ..NOTE };
                lines.extend(paragraph(
                    note_text(&text, base, true, false),
                    x,
                    width,
                    size,
                    size * 1.4,
                ));
            }
            NoteBlock::Paragraph(text) => {
                lines.extend(paragraph(
                    note_text(&text, NOTE, false, false),
                    x,
                    width,
                    NOTE.size,
                    NOTE_LEADING,
                ));
            }
            NoteBlock::List { ordered, items } => {
                for (at, item) in items.iter().enumerate() {
                    let marker = if ordered {
                        format!("{}.", at + 1)
                    } else {
                        "•".to_owned()
                    };
                    let marker = shape(&marker, NOTE);
                    let mut set = paragraph(
                        note_text(item, NOTE, false, false),
                        x + 13.0,
                        width - 13.0,
                        NOTE.size,
                        NOTE_LEADING,
                    );
                    if let Some(first) = set.first_mut() {
                        first.runs.insert(0, (x + 1.0, marker));
                    }
                    lines.extend(set);
                }
            }
            NoteBlock::Quote(text) => {
                let base = Style {
                    face: Face::SansItalic,
                    color: SOFT,
                    ..NOTE
                };
                for mut line in paragraph(
                    note_text(&text, base, false, true),
                    x + 10.0,
                    width - 10.0,
                    NOTE.size,
                    NOTE_LEADING,
                ) {
                    line.fills.push(Fill {
                        x: x + 1.0,
                        y: 0.0,
                        w: 1.4,
                        h: line.height,
                        color: RULE,
                    });
                    lines.push(line);
                }
            }
            NoteBlock::Code(code) => {
                let code = clean(&code).replace('\t', "    ");
                for source_line in code.split('\n') {
                    for set in wrap_hard(shape(source_line, CODE), width - 8.0) {
                        let mut line = Line::text(x + 4.0, set, CODE.size, CODE_LEADING);
                        line.fills.push(Fill {
                            x,
                            y: 0.0,
                            w: width,
                            h: CODE_LEADING,
                            color: CODE_GROUND,
                        });
                        lines.push(line);
                    }
                }
            }
        }
    }
    lines
}

const QUOTE: Style = style(Face::Serif, 10.4, [0x1a, 0x1a, 0x1a]);
const QUOTE_LEADING: f32 = 15.0;
const META: Style = style(Face::Sans, 7.9, SOFT);

/// What a book says, its paragraphs apart, each one's lines joined as text flows.
fn quote_lines(quote: &str, x: f32, width: f32) -> Vec<Line> {
    let cleaned = clean(quote);
    let mut lines = Vec::new();
    let mut paragraph_text = Vec::new();
    let mut paragraphs = Vec::new();
    for line in cleaned.split('\n') {
        if line.trim().is_empty() {
            if !paragraph_text.is_empty() {
                paragraphs.push(paragraph_text.join(" "));
                paragraph_text.clear();
            }
        } else {
            paragraph_text.push(line.trim().to_owned());
        }
    }
    if !paragraph_text.is_empty() {
        paragraphs.push(paragraph_text.join(" "));
    }
    for (index, text) in paragraphs.iter().enumerate() {
        if index > 0 {
            lines.push(Line::space(4.0));
        }
        lines.extend(paragraph(
            shape(text, QUOTE),
            x,
            width,
            QUOTE.size,
            QUOTE_LEADING,
        ));
    }
    lines
}

fn item_block(item: &Item, work: &WorkReport, offset: i32, index: usize) -> Block {
    let x = LEFT + INDENT;
    let width = WIDTH - INDENT;
    let mut lines = Vec::new();
    let mut head = shape(
        item.label(),
        Style {
            face: Face::SansBold,
            ..META
        },
    );
    head.extend(shape(
        &format!(" · {}", meta(item, work.layout, offset).join(" · ")),
        META,
    ));
    lines.extend(paragraph(head, x, width, META.size, 11.5));
    match item
        .quote
        .as_deref()
        .filter(|quote| !quote.trim().is_empty())
    {
        Some(quote) => {
            lines.push(Line::space(2.0));
            lines.extend(quote_lines(quote, x, width));
        }
        None if graphic(item) => {
            lines.push(Line::space(1.0));
            lines.extend(paragraph(
                shape(
                    "Sem texto: uma marca desenhada na página.",
                    Style {
                        face: Face::SansItalic,
                        size: 8.8,
                        color: FAINT,
                    },
                ),
                x,
                width,
                8.8,
                12.5,
            ));
        }
        None => {}
    }
    if !item.note.trim().is_empty() {
        lines.push(Line::space(4.5));
        lines.extend(note_lines(&item.note, x, width));
    }
    let count = lines.len();
    Block {
        before: 11.0,
        keep: if count <= 14 {
            Keep::Together
        } else {
            Keep::Lines(3)
        },
        band: Some(paint(&item.color, item.kind == Kind::Underline)),
        work: Some(index),
        ..Block::new(lines)
    }
}

fn heading_block(text: &str, depth: usize, level: usize, index: Option<usize>) -> Block {
    let (size, leading, before) = match depth {
        0 => (14.5, 19.0, 20.0),
        1 => (12.4, 16.5, 15.0),
        _ => (11.2, 15.0, 12.0),
    };
    let label = fold_spaces(text);
    Block {
        before,
        with_next: true,
        bookmark: Some((level, label.clone())),
        work: index,
        ..Block::new(paragraph(
            shape(&label, style(Face::SerifBold, size, INK)),
            LEFT,
            WIDTH,
            size,
            leading,
        ))
    }
}

fn plain_block(
    text: &str,
    style_of: Style,
    leading: f32,
    before: f32,
    index: Option<usize>,
) -> Block {
    Block {
        before,
        work: index,
        ..Block::new(paragraph(
            shape(&clean(text), style_of),
            LEFT,
            WIDTH,
            style_of.size,
            leading,
        ))
    }
}

fn rule_block(before: f32, index: Option<usize>) -> Block {
    let mut line = Line::space(1.0);
    line.fills.push(Fill {
        x: LEFT,
        y: 0.0,
        w: WIDTH,
        h: 0.6,
        color: RULE,
    });
    Block {
        before,
        work: index,
        ..Block::new(vec![line, Line::space(2.0)])
    }
}

/// The blocks of one work: its title and facts, then its annotations under its chapters.
fn work_blocks(
    work: &WorkReport,
    index: usize,
    report: &Report,
    level: usize,
    blocks: &mut Vec<Block>,
) {
    let offset = report.utc_offset_minutes;
    let title = fold_spaces(&work.title);
    blocks.push(Block {
        bookmark: Some((level, title.clone())),
        work: Some(index),
        new_page: level > 0,
        ..Block::new(paragraph(
            shape(&title, style(Face::SerifBold, 20.0, INK)),
            LEFT,
            WIDTH,
            20.0,
            25.5,
        ))
    });
    let by = byline(work);
    if !by.is_empty() {
        blocks.push(plain_block(
            &by,
            style(Face::SansItalic, 10.8, [0x33, 0x33, 0x33]),
            15.0,
            3.0,
            Some(index),
        ));
    }
    let mut facts: Vec<(&str, String)> = Vec::new();
    if let Some(series) = work.series.as_deref().filter(|s| !s.trim().is_empty()) {
        let position = work.series_position.map(|n| {
            if n.fract() == 0.0 {
                format!(", n.º {}", n as i64)
            } else {
                format!(", n.º {n}")
            }
        });
        facts.push((
            "Série",
            format!("{}{}", series.trim(), position.unwrap_or_default()),
        ));
    }
    if let Some(year) = work.year {
        facts.push(("Ano", year.to_string()));
    }
    if let Some(language) = work.language.as_deref().filter(|l| !l.trim().is_empty()) {
        facts.push(("Idioma", language.trim().to_owned()));
    }
    if !work.identifiers.is_empty() {
        facts.push(("Identificadores", identifiers(work)));
    }
    facts.push(("Arquivo", file_line(work)));
    facts.push(("Anotações", counts(work)));
    let fact_style = style(Face::Sans, 8.8, SOFT);
    let mut lines = Vec::new();
    for (label, value) in facts {
        let mut glyphs = shape(
            &format!("{label}: "),
            Style {
                face: Face::SansBold,
                ..fact_style
            },
        );
        glyphs.extend(shape(&clean(&value), fact_style));
        lines.extend(paragraph(glyphs, LEFT, WIDTH, fact_style.size, 12.8));
    }
    blocks.push(Block {
        before: 8.0,
        work: Some(index),
        ..Block::new(lines)
    });
    if level == 0 {
        blocks.push(plain_block(
            &format!(
                "Relatório de estudo gerado em {} ({}) pelo {}.",
                time::date_time(report.generated_at, offset),
                time::offset_label(offset),
                report.generator
            ),
            style(Face::SansItalic, 8.3, FAINT),
            12.0,
            6.0,
            Some(index),
        ));
    }
    blocks.push(rule_block(9.0, Some(index)));

    let Ordered {
        whole,
        sections,
        orphans,
    } = order::order(work);
    if work.items.is_empty() {
        blocks.push(plain_block(
            "Nenhuma anotação nesta obra.",
            style(Face::SansItalic, 9.5, SOFT),
            13.0,
            12.0,
            Some(index),
        ));
    }
    if !whole.is_empty() {
        blocks.push(heading_block(
            "Sobre o livro todo",
            0,
            level + 1,
            Some(index),
        ));
        for item in whole {
            blocks.push(item_block(item, work, offset, index));
        }
    }
    let mut printed: Vec<&Heading> = Vec::new();
    for section in &sections {
        let common = printed
            .iter()
            .zip(&section.path)
            .take_while(|(a, b)| std::ptr::eq(**a, **b))
            .count();
        for (depth, heading) in section.path.iter().enumerate().skip(common) {
            blocks.push(heading_block(
                &heading.label,
                depth,
                level + 1 + depth,
                Some(index),
            ));
        }
        printed = section.path.clone();
        for item in &section.items {
            blocks.push(item_block(item, work, offset, index));
        }
    }
    if !orphans.is_empty() {
        blocks.push(heading_block(
            "Não encontradas no livro",
            0,
            level + 1,
            Some(index),
        ));
        blocks.push(plain_block(
            "O trecho destas anotações não foi encontrado no livro como ele está agora. Elas ficam com a citação, a nota e o lugar em que estavam.",
            style(Face::SansItalic, 8.8, SOFT),
            12.5,
            2.0,
            Some(index),
        ));
        for item in orphans {
            debug_assert_eq!(item.found, Found::Orphan);
            blocks.push(item_block(item, work, offset, index));
        }
    }
}

/// A line placed on a page, from the top of the page.
struct Placed {
    y: f32,
    line: Line,
}

struct Page {
    lines: Vec<Placed>,
    bands: Vec<Fill>,
    work: Option<usize>,
}

struct Bookmark {
    level: usize,
    title: String,
    page: usize,
    y: f32,
}

fn paginate(blocks: Vec<Block>) -> (Vec<Page>, Vec<Bookmark>) {
    let bottom = PAGE_H - BOTTOM;
    let mut pages = vec![Page {
        lines: Vec::new(),
        bands: Vec::new(),
        work: None,
    }];
    let mut bookmarks = Vec::new();
    let mut y = TOP;
    // What a heading must have below it on its page: the start of the next block.
    let next_start: Vec<f32> = (0..blocks.len())
        .map(|index| {
            blocks.get(index + 1).map_or(0.0, |next| {
                let lines = match next.keep {
                    Keep::Together => next.lines.len(),
                    Keep::Lines(minimum) => minimum.min(next.lines.len()),
                };
                next.before
                    + next
                        .lines
                        .iter()
                        .take(lines)
                        .map(|line| line.height)
                        .sum::<f32>()
            })
        })
        .collect();
    let at_top = |y: f32| (y - TOP).abs() < 0.01;
    for (index, block) in blocks.into_iter().enumerate() {
        let turn = |pages: &mut Vec<Page>, y: &mut f32| {
            pages.push(Page {
                lines: Vec::new(),
                bands: Vec::new(),
                work: block.work,
            });
            *y = TOP;
        };
        if block.new_page && !at_top(y) {
            turn(&mut pages, &mut y);
        }
        let height = block.height();
        let before = if at_top(y) { 0.0 } else { block.before };
        let mut needed = before + height;
        if block.with_next {
            needed += next_start[index];
        }
        let minimum = match block.keep {
            Keep::Together => block.lines.len(),
            Keep::Lines(minimum) => minimum,
        };
        let first: f32 = before
            + block
                .lines
                .iter()
                .take(minimum)
                .map(|line| line.height)
                .sum::<f32>();
        // Moved to the next page whole, when it must not be cut or what may stay here is too little.
        if !at_top(y)
            && needed > bottom - y + 0.01
            && (block.with_next || first > bottom - y + 0.01 || block.keep == Keep::Together)
        {
            turn(&mut pages, &mut y);
        } else {
            y += before;
        }
        if let Some(page) = pages.last_mut()
            && page.work.is_none()
        {
            page.work = block.work;
        }
        if let Some((level, title)) = &block.bookmark {
            bookmarks.push(Bookmark {
                level: *level,
                title: title.clone(),
                page: pages.len() - 1,
                y,
            });
        }
        let lines = block.lines;
        let total = lines.len();
        let mut at = 0;
        while at < total {
            // Lines of space do not start a page.
            if at_top(y) && at > 0 {
                while at < total && lines[at].runs.is_empty() && lines[at].fills.is_empty() {
                    at += 1;
                }
                if at == total {
                    break;
                }
            }
            let mut fit = 0;
            let mut used = 0.0;
            while at + fit < total && y + used + lines[at + fit].height <= bottom + 0.01 {
                used += lines[at + fit].height;
                fit += 1;
            }
            let mut count = fit;
            if at + fit < total {
                // A cut: what goes on leaves at least the minimum, if the block may be cut at all.
                if let Keep::Lines(minimum) = block.keep {
                    count = count.min((total - at).saturating_sub(minimum));
                    if count < minimum {
                        count = 0;
                    }
                }
                if count == 0 && at_top(y) {
                    // Taller than a page, or too tight to cut well: cut where the page ends.
                    count = fit.max(1);
                }
            }
            let start = y;
            for line in &lines[at..at + count] {
                let height = line.height;
                if let Some(page) = pages.last_mut() {
                    page.lines.push(Placed {
                        y,
                        line: line.clone(),
                    });
                }
                y += height;
            }
            if let (Some(color), Some(page)) = (block.band, pages.last_mut())
                && count > 0
            {
                page.bands.push(Fill {
                    x: LEFT,
                    y: start,
                    w: BAND,
                    h: y - start,
                    color,
                });
            }
            at += count;
            if at < total {
                turn(&mut pages, &mut y);
            }
        }
    }
    (pages, bookmarks)
}

/// A number as a PDF writes it: no more than three decimals, no trailing zeros.
fn num(value: f32) -> String {
    let rounded = (f64::from(value) * 1000.0).round() / 1000.0;
    let mut text = format!("{rounded:.3}");
    while text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    if text == "-0" { "0".to_owned() } else { text }
}

fn color(rgb: [u8; 3]) -> String {
    let channel = |value: u8| num(f32::from(value) / 255.0);
    format!(
        "{} {} {}",
        channel(rgb[0]),
        channel(rgb[1]),
        channel(rgb[2])
    )
}

/// A text string as UTF-16BE behind its byte order mark, in hex.
fn text_string(text: &str) -> String {
    let mut out = String::from("<FEFF");
    for unit in text.encode_utf16() {
        let _ = write!(out, "{unit:04X}");
    }
    out.push('>');
    out
}

fn deflate(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::new(6));
    let _ = encoder.write_all(bytes);
    encoder.finish().unwrap_or_default()
}

/// Each glyph of a face, by its number in the font (or none: a blank) and the character it stands
/// for, and its number in the subset.
type Numbers = BTreeMap<(Option<u16>, char), u16>;

/// The glyphs of one face a document uses: the font's own, and blanks for characters none draws.
#[derive(Default)]
struct FaceUse {
    glyphs: BTreeMap<u16, char>,
    blanks: BTreeSet<char>,
}

struct Objects {
    bodies: Vec<Vec<u8>>,
}

impl Objects {
    fn reserve(&mut self) -> usize {
        self.bodies.push(Vec::new());
        self.bodies.len()
    }

    fn set(&mut self, id: usize, body: impl Into<Vec<u8>>) {
        self.bodies[id - 1] = body.into();
    }

    fn add(&mut self, body: impl Into<Vec<u8>>) -> usize {
        let id = self.reserve();
        self.set(id, body);
        id
    }

    fn stream(&mut self, dictionary: &str, data: &[u8]) -> usize {
        let packed = deflate(data);
        let mut body = format!(
            "<< {dictionary} /Filter /FlateDecode /Length {} >>\nstream\n",
            packed.len()
        )
        .into_bytes();
        body.extend_from_slice(&packed);
        body.extend_from_slice(b"\nendstream");
        self.add(body)
    }
}

/// Six capitals that name a subset, from what is in it, as PDF wants before a subset's name.
fn subset_tag(name: &str, font: &[u8]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(name.as_bytes());
    hasher.update(font);
    hasher.finalize().as_bytes()[..6]
        .iter()
        .map(|byte| char::from(b'A' + byte % 26))
        .collect()
}

fn to_unicode(map: &BTreeMap<u16, char>) -> Vec<u8> {
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n\
         /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
         /CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n\
         1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
    );
    let entries: Vec<(&u16, &char)> = map.iter().collect();
    for chunk in entries.chunks(100) {
        let _ = writeln!(cmap, "{} beginbfchar", chunk.len());
        for (glyph, c) in chunk {
            let mut units = [0u16; 2];
            let hex: String = c
                .encode_utf16(&mut units)
                .iter()
                .map(|unit| format!("{unit:04X}"))
                .collect();
            let _ = writeln!(cmap, "<{glyph:04X}> <{hex}>");
        }
        cmap.push_str("endbfchar\n");
    }
    cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    cmap.into_bytes()
}

/// Writes one face's subset and its font objects; returns the font dictionary and each glyph's
/// new number, blanks included.
fn embed_face(objects: &mut Objects, face: Face, used: &FaceUse) -> (usize, Numbers) {
    let font = face.font();
    let real: Vec<u16> = used.glyphs.keys().copied().collect();
    let (subset, renumbered) = font.subset(&real, used.blanks.len());
    let first_blank = renumbered.len() as u16;
    let mut numbers: BTreeMap<(Option<u16>, char), u16> = BTreeMap::new();
    let mut unicode: BTreeMap<u16, char> = BTreeMap::new();
    for (&old, &c) in &used.glyphs {
        let new = renumbered.get(&old).copied().unwrap_or(0);
        numbers.insert((Some(old), c), new);
        unicode.entry(new).or_insert(c);
    }
    for (index, &c) in used.blanks.iter().enumerate() {
        let new = first_blank + index as u16;
        numbers.insert((None, c), new);
        unicode.insert(new, c);
    }
    let count = usize::from(first_blank) + used.blanks.len();
    let mut order: Vec<u16> = vec![0; count];
    for (&old, &new) in &renumbered {
        order[usize::from(new)] = old;
    }
    let widths: Vec<String> = order
        .iter()
        .map(|&old| font.width(old).to_string())
        .collect();
    let scale = |units: i16| i32::from(units) * 1000 / i32::from(font.units_per_em);
    let tag = subset_tag(&font.name, &subset);
    let name = format!("{tag}+{}", font.name);
    let mut flags = 0u32;
    if font.fixed_pitch || face == Face::Mono {
        flags |= 1;
    }
    if face.serif() {
        flags |= 2;
    }
    flags |= if matches!(face, Face::Math | Face::Symbols) {
        4
    } else {
        32
    };
    if font.italic_angle != 0.0 {
        flags |= 64;
    }
    let font_file = objects.stream(&format!("/Length1 {}", subset.len()), &subset);
    let descriptor = objects.add(format!(
        "<< /Type /FontDescriptor /FontName /{name} /Flags {flags} /FontBBox [{} {} {} {}] /ItalicAngle {} \
         /Ascent {} /Descent {} /CapHeight {} /StemV {} /FontFile2 {font_file} 0 R >>",
        scale(font.bbox[0]),
        scale(font.bbox[1]),
        scale(font.bbox[2]),
        scale(font.bbox[3]),
        num(font.italic_angle),
        scale(font.ascent),
        scale(font.descent),
        scale(font.cap_height),
        if font.weight >= 600 { 140 } else { 80 },
    ));
    let cid = objects.add(format!(
        "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /{name} \
         /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> \
         /FontDescriptor {descriptor} 0 R /CIDToGIDMap /Identity /W [0 [{}]] >>",
        widths.join(" ")
    ));
    let map = objects.stream("", &to_unicode(&unicode));
    let font_object = objects.add(format!(
        "<< /Type /Font /Subtype /Type0 /BaseFont /{name} /Encoding /Identity-H \
         /DescendantFonts [{cid} 0 R] /ToUnicode {map} 0 R >>"
    ));
    (font_object, numbers)
}

fn header_line(text: &str, style_of: Style, x: f32, max: f32) -> (f32, Vec<Glyph>) {
    let mut glyphs = shape(text, style_of);
    if width_of(&glyphs) > max {
        let ellipsis = shape("…", style_of);
        let room = max - width_of(&ellipsis);
        let mut used = 0.0;
        let mut kept = Vec::new();
        for glyph in glyphs {
            if used + glyph.width > room {
                break;
            }
            used += glyph.width;
            kept.push(glyph);
        }
        while kept.last().is_some_and(|glyph| glyph.c == ' ') {
            kept.pop();
        }
        kept.extend(ellipsis);
        glyphs = kept;
    }
    (x, glyphs)
}

/// The report as a PDF file.
pub fn pdf(report: &Report) -> Vec<u8> {
    render(report).0
}

/// The report as a PDF file, and how many pages it has.
pub fn render(report: &Report) -> (Vec<u8>, usize) {
    let mut blocks = Vec::new();
    let single = report.works.len() == 1;
    if single {
        work_blocks(&report.works[0], 0, report, 0, &mut blocks);
    } else {
        blocks.push(Block {
            bookmark: Some((0, "Relatório de estudo".to_owned())),
            ..Block::new(paragraph(
                shape("Relatório de estudo", style(Face::SerifBold, 22.0, INK)),
                LEFT,
                WIDTH,
                22.0,
                28.0,
            ))
        });
        let offset = report.utc_offset_minutes;
        blocks.push(plain_block(
            &format!(
                "{} obras, gerado em {} ({}) pelo {}.",
                report.works.len(),
                time::date_time(report.generated_at, offset),
                time::offset_label(offset),
                report.generator
            ),
            style(Face::SansItalic, 9.0, FAINT),
            13.0,
            5.0,
            None,
        ));
        blocks.push(rule_block(9.0, None));
        for work in &report.works {
            let mut glyphs = shape(&fold_spaces(&work.title), style(Face::SansBold, 9.6, INK));
            glyphs.extend(shape(
                &format!(" — {}", order::tally(&work.items)),
                style(Face::Sans, 9.6, SOFT),
            ));
            blocks.push(Block {
                before: 4.0,
                ..Block::new(paragraph(glyphs, LEFT, WIDTH, 9.6, 13.6))
            });
        }
        for (index, work) in report.works.iter().enumerate() {
            work_blocks(work, index, report, 1, &mut blocks);
        }
    }
    let (mut pages, bookmarks) = paginate(blocks);

    // The running heads: the work at the top of every page but the first, the page at the foot.
    let total = pages.len();
    let head = style(Face::Sans, 7.6, FAINT);
    let made = format!(
        "Musaeum · {}",
        time::date(report.generated_at, report.utc_offset_minutes)
    );
    for (number, page) in pages.iter_mut().enumerate() {
        if number > 0
            && let Some(work) = page.work.and_then(|index| report.works.get(index))
        {
            let label = shape("Relatório de estudo", head);
            let label_width = width_of(&label);
            let (x, glyphs) = header_line(
                &fold_spaces(&work.title),
                head,
                LEFT,
                WIDTH - label_width - 24.0,
            );
            let mut line = Line::text(x, glyphs, head.size, 10.0);
            line.runs.push((LEFT + WIDTH - label_width, label));
            line.fills.push(Fill {
                x: LEFT,
                y: 13.5,
                w: WIDTH,
                h: 0.5,
                color: RULE,
            });
            page.lines.push(Placed { y: 40.0, line });
        }
        let foot = shape(&format!("página {} de {total}", number + 1), head);
        let foot_width = width_of(&foot);
        let mut line = Line::text(LEFT, shape(&made, head), head.size, 10.0);
        line.runs.push((LEFT + WIDTH - foot_width, foot));
        page.lines.push(Placed {
            y: PAGE_H - 46.0,
            line,
        });
    }

    // Which glyphs of which faces are used.
    let mut uses: BTreeMap<Face, FaceUse> = BTreeMap::new();
    for page in &pages {
        for placed in &page.lines {
            for (_, run) in &placed.line.runs {
                for glyph in run {
                    let using = uses.entry(glyph.face).or_default();
                    match glyph.id {
                        Some(id) => {
                            using.glyphs.entry(id).or_insert(glyph.c);
                        }
                        None => {
                            using.blanks.insert(glyph.c);
                        }
                    }
                }
            }
        }
    }

    let mut objects = Objects { bodies: Vec::new() };
    let catalog = objects.reserve();
    let pages_id = objects.reserve();
    let mut fonts: BTreeMap<Face, (usize, Numbers)> = BTreeMap::new();
    for (face, using) in &uses {
        fonts.insert(*face, embed_face(&mut objects, *face, using));
    }
    let resources = format!(
        "<< /Font << {} >> >>",
        fonts
            .iter()
            .map(|(face, (id, _))| format!("/F{} {id} 0 R", *face as usize))
            .collect::<Vec<_>>()
            .join(" ")
    );
    let mut page_ids = Vec::with_capacity(pages.len());
    for page in &pages {
        let mut content = String::new();
        for band in &page.bands {
            let _ = writeln!(
                content,
                "{} rg {} {} {} {} re f",
                color(band.color),
                num(band.x),
                num(PAGE_H - band.y - band.h),
                num(band.w),
                num(band.h)
            );
        }
        for placed in &page.lines {
            for fill in &placed.line.fills {
                let _ = writeln!(
                    content,
                    "{} rg {} {} {} {} re f",
                    color(fill.color),
                    num(fill.x),
                    num(PAGE_H - placed.y - fill.y - fill.h),
                    num(fill.w),
                    num(fill.h)
                );
            }
        }
        content.push_str("BT\n");
        let mut current: Option<(Face, String)> = None;
        let mut painted: Option<[u8; 3]> = None;
        for placed in &page.lines {
            let baseline = PAGE_H - placed.y - placed.line.baseline;
            for (x, run) in &placed.line.runs {
                let mut at = *x;
                let mut pieces: Vec<(usize, usize)> = Vec::new();
                let mut start = 0;
                for index in 1..=run.len() {
                    let split = index == run.len()
                        || run[index].face != run[start].face
                        || run[index].size != run[start].size
                        || run[index].color != run[start].color;
                    if split {
                        pieces.push((start, index));
                        start = index;
                    }
                }
                for (from, to) in pieces {
                    let first = &run[from];
                    let font = (first.face, num(first.size));
                    if current.as_ref() != Some(&font) {
                        let _ = writeln!(content, "/F{} {} Tf", first.face as usize, font.1);
                        current = Some(font);
                    }
                    if painted != Some(first.color) {
                        let _ = writeln!(content, "{} rg", color(first.color));
                        painted = Some(first.color);
                    }
                    let numbers = &fonts[&first.face].1;
                    let hex: String = run[from..to]
                        .iter()
                        .map(|glyph| {
                            format!(
                                "{:04X}",
                                numbers.get(&(glyph.id, glyph.c)).copied().unwrap_or(0)
                            )
                        })
                        .collect();
                    let _ = writeln!(
                        content,
                        "1 0 0 1 {} {} Tm <{hex}> Tj",
                        num(at),
                        num(baseline)
                    );
                    at += width_of(&run[from..to]);
                }
            }
        }
        content.push_str("ET\n");
        let contents = objects.stream("", content.as_bytes());
        let id = objects.add(format!(
            "<< /Type /Page /Parent {pages_id} 0 R /MediaBox [0 0 {} {}] /Resources {resources} /Contents {contents} 0 R >>",
            num(PAGE_W),
            num(PAGE_H)
        ));
        page_ids.push(id);
    }
    objects.set(
        pages_id,
        format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            page_ids
                .iter()
                .map(|id| format!("{id} 0 R"))
                .collect::<Vec<_>>()
                .join(" "),
            page_ids.len()
        ),
    );

    let outlines = write_bookmarks(&mut objects, &bookmarks, &page_ids);
    let title = if single {
        fold_spaces(&report.works[0].title)
    } else {
        "Relatório de estudo".to_owned()
    };
    let date = time::pdf_date(report.generated_at, report.utc_offset_minutes);
    let info = objects.add(format!(
        "<< /Title {} /Subject {} /Creator {} /Producer {} /CreationDate ({date}) /ModDate ({date}) >>",
        text_string(&title),
        text_string("Relatório de estudo"),
        text_string("Musaeum"),
        text_string(&report.generator),
    ));
    let outline_entry = outlines
        .map(|id| format!(" /Outlines {id} 0 R /PageMode /UseOutlines"))
        .unwrap_or_default();
    objects.set(
        catalog,
        format!(
            "<< /Type /Catalog /Pages {pages_id} 0 R{outline_entry} /Lang (pt-BR) /ViewerPreferences << /DisplayDocTitle true >> >>"
        ),
    );

    let mut out = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.bodies.len());
    for (index, body) in objects.bodies.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let id: String = blake3::hash(&out).as_bytes()[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let xref = out.len();
    let mut table = format!(
        "xref\n0 {}\n0000000000 65535 f \n",
        objects.bodies.len() + 1
    );
    for offset in offsets {
        let _ = writeln!(table, "{offset:010} 00000 n ");
    }
    let _ = write!(
        table,
        "trailer\n<< /Size {} /Root {catalog} 0 R /Info {info} 0 R /ID [<{id}> <{id}>] >>\nstartxref\n{xref}\n%%EOF\n",
        objects.bodies.len() + 1
    );
    out.extend_from_slice(table.as_bytes());
    (out, page_ids.len())
}

/// The bookmarks as the outline tree, each under the last one before it of a lower level.
fn write_bookmarks(
    objects: &mut Objects,
    bookmarks: &[Bookmark],
    pages: &[usize],
) -> Option<usize> {
    if bookmarks.is_empty() {
        return None;
    }
    let root = objects.reserve();
    let ids: Vec<usize> = bookmarks.iter().map(|_| objects.reserve()).collect();
    // Each one's parent: the nearest before it with a lower level, or the root.
    let mut parent: Vec<Option<usize>> = Vec::with_capacity(bookmarks.len());
    for (index, mark) in bookmarks.iter().enumerate() {
        parent.push(
            (0..index)
                .rev()
                .find(|&before| bookmarks[before].level < mark.level),
        );
    }
    let children = |of: Option<usize>| -> Vec<usize> {
        (0..bookmarks.len())
            .filter(|&index| parent[index] == of)
            .collect()
    };
    let descendants = |of: usize| -> usize {
        let mut count = 0;
        let mut stack = vec![of];
        while let Some(at) = stack.pop() {
            for child in (0..bookmarks.len()).filter(|&index| parent[index] == Some(at)) {
                count += 1;
                stack.push(child);
            }
        }
        count
    };
    for (index, mark) in bookmarks.iter().enumerate() {
        let siblings = children(parent[index]);
        let place = siblings
            .iter()
            .position(|&sibling| sibling == index)
            .unwrap_or(0);
        let mut body = format!(
            "<< /Title {} /Parent {} 0 R /Dest [{} 0 R /XYZ {} {} 0]",
            text_string(&mark.title),
            parent[index].map_or(root, |up| ids[up]),
            pages[mark.page.min(pages.len() - 1)],
            num(LEFT),
            num(PAGE_H - mark.y + 4.0)
        );
        if place > 0 {
            let _ = write!(body, " /Prev {} 0 R", ids[siblings[place - 1]]);
        }
        if let Some(&next) = siblings.get(place + 1) {
            let _ = write!(body, " /Next {} 0 R", ids[next]);
        }
        let own = children(Some(index));
        if let (Some(&first), Some(&last)) = (own.first(), own.last()) {
            let _ = write!(
                body,
                " /First {} 0 R /Last {} 0 R /Count {}",
                ids[first],
                ids[last],
                descendants(index)
            );
        }
        body.push_str(" >>");
        objects.set(ids[index], body);
    }
    let top = children(None);
    objects.set(
        root,
        format!(
            "<< /Type /Outlines /First {} 0 R /Last {} 0 R /Count {} >>",
            ids[top[0]],
            ids[*top.last().unwrap_or(&top[0])],
            bookmarks.len()
        ),
    );
    Some(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_break_at_spaces_and_long_words_are_cut() {
        let glyphs = shape("um dois três quatro", NOTE);
        let lines = wrap(glyphs.clone(), width_of(&shape("um dois", NOTE)) + 1.0);
        let texts: Vec<String> = lines
            .iter()
            .map(|line| line.iter().map(|g| g.c).collect())
            .collect();
        assert_eq!(texts, ["um dois", "três", "quatro"]);
        let long = wrap(
            shape("abcdefghij", NOTE),
            width_of(&shape("abcd", NOTE)) + 0.5,
        );
        let texts: Vec<String> = long
            .iter()
            .map(|line| line.iter().map(|g| g.c).collect())
            .collect();
        assert_eq!(texts, ["abcd", "efgh", "ij"]);
        assert_eq!(
            wrap(Vec::new(), 100.0).len(),
            1,
            "an empty paragraph is one empty line"
        );
    }

    #[test]
    fn numbers_are_short_and_fixed() {
        assert_eq!(num(1.0), "1");
        assert_eq!(num(0.12345), "0.123");
        assert_eq!(num(-0.0001), "0");
        assert_eq!(num(841.89), "841.89");
    }

    #[test]
    fn the_same_report_gives_the_same_bytes() {
        let report = crate::markdown::sample();
        let first = pdf(&report);
        assert!(first.starts_with(b"%PDF-1.7"));
        assert!(first.ends_with(b"%%EOF\n"));
        assert_eq!(pdf(&report), first);
    }
}

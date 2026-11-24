//! A report as Markdown: UTF-8, line feeds, the same text for the same report, for Obsidian, Notion
//! or a Git repository.
//!
//! Its structure is the report's own (front matter, the work, its chapters, each annotation) and
//! nothing a book or a note says can break it: what a book says is text, escaped where Markdown,
//! or the extensions those tools add (tags, highlights, comments, math), would read it as
//! something else; what a note says is read as the reader reads it ([`crate::notes`]) and written
//! again from that, so a fence it never closed is closed and a heading in it stays below the
//! report's own.

use crate::notes::{self, Block, Inline};
use crate::order::{self, Ordered};
use crate::text::clean;
use crate::{Found, Heading, Item, Layout, Mark, Report, WorkReport, color_name, time};

/// A string as YAML writes it double-quoted, which is how JSON writes it.
fn yaml_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}' | '\u{FEFF}') => {
                out.push_str(&format!("\\u{:04X}", u32::from(c)));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn yaml_list(values: &[String]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(|value| yaml_string(value))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// A number as a person writes it: no `.0` on a whole one.
fn number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

/// Text that must stay text, escaped for Markdown and what Obsidian and GitHub read in it. `start`
/// says whether it begins a line, where more is read as structure.
pub fn escape(text: &str, start: bool) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + 8);
    for (at, &c) in chars.iter().enumerate() {
        let next = chars.get(at + 1).copied();
        let first = start && at == 0;
        let escape = match c {
            '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '#' | '|' | '~' | '$' => true,
            // An entity, as `&amp;`.
            '&' => next.is_some_and(|n| n.is_ascii_alphanumeric() || n == '#'),
            // Obsidian's ==highlight== and %%comment%%.
            '=' | '%' => next == Some(c) || (at > 0 && chars[at - 1] == c),
            // A list item or a heading's underline at the start of a line.
            '-' | '+' => first,
            // `1.` or `1)` at the start of a line begins an ordered list.
            '.' | ')' => {
                start
                    && at > 0
                    && chars[..at].iter().all(char::is_ascii_digit)
                    && next.is_none_or(char::is_whitespace)
            }
            _ => false,
        };
        if escape {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// What a book says, as a quotation: each of its lines behind `> `, and a line of its own between
/// paragraphs.
fn quotation(text: &str) -> String {
    let cleaned = clean(text);
    let mut lines: Vec<String> = Vec::new();
    for line in cleaned.split('\n') {
        let line = line.trim();
        if line.is_empty() {
            if lines.last().is_some_and(|last| last != ">") {
                lines.push(">".to_owned());
            }
            continue;
        }
        lines.push(format!("> {}", escape(line, true)));
    }
    while lines.last().is_some_and(|last| last == ">") {
        lines.pop();
    }
    lines.join("\n")
}

fn inline_markdown(pieces: &[Inline], start: bool) -> String {
    let mut out = String::new();
    for (index, piece) in pieces.iter().enumerate() {
        let start = start && index == 0;
        match piece {
            Inline::Text(text) => out.push_str(&escape(text, start)),
            Inline::Code(code) => {
                // The reader's code has no backtick in it.
                out.push('`');
                out.push_str(code);
                out.push('`');
            }
            Inline::Strong(inner) => {
                out.push_str("**");
                out.push_str(&inline_markdown(inner, false));
                out.push_str("**");
            }
            Inline::Emphasis(inner) => {
                out.push('*');
                out.push_str(&inline_markdown(inner, false));
                out.push('*');
            }
            Inline::Link { label, href } if notes::linkable(href) => {
                out.push('[');
                out.push_str(&escape(label, false));
                out.push_str("](");
                for c in href.chars() {
                    if matches!(c, '(' | ')' | '<' | '>' | '\\') {
                        out.push('\\');
                    }
                    out.push(c);
                }
                out.push(')');
            }
            // The reader shows any other link as its label and its address, as text.
            Inline::Link { label, href } => {
                out.push_str(&escape(&format!("{label} ({href})"), start));
            }
        }
    }
    out
}

fn inline_text(text: &str, start: bool) -> String {
    inline_markdown(&notes::inline(&clean(text)), start)
}

/// A note, written again from what the reader reads in it. Its headings go `shift` levels down,
/// never above six.
fn note_markdown(source: &str, shift: u8) -> String {
    let mut out: Vec<String> = Vec::new();
    for block in notes::blocks(source) {
        out.push(match block {
            Block::Heading { level, text } => {
                let level = (level + shift).min(6) as usize;
                format!("{} {}", "#".repeat(level), inline_text(&text, false))
            }
            Block::Paragraph(text) => inline_text(&text, true),
            Block::List { ordered, items } => items
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    let marker = if ordered {
                        format!("{}.", index + 1)
                    } else {
                        "-".to_owned()
                    };
                    format!("{marker} {}", inline_text(item, true))
                })
                .collect::<Vec<_>>()
                .join("\n"),
            Block::Quote(text) => format!("> {}", inline_text(&text, true)),
            Block::Code(code) => {
                // A fence longer than any run of backticks inside.
                let mut longest = 0;
                let mut run = 0;
                for c in code.chars() {
                    run = if c == '`' { run + 1 } else { 0 };
                    longest = longest.max(run);
                }
                let fence = "`".repeat((longest + 1).max(3));
                format!("{fence}\n{}\n{fence}", clean(&code))
            }
        });
    }
    out.join("\n\n")
}

fn role_label(role: &str) -> Option<&str> {
    match role {
        "author" | "" => None,
        "editor" => Some("org."),
        "translator" => Some("trad."),
        "illustrator" => Some("il."),
        other => Some(other),
    }
}

/// `A; B (trad.)`.
pub fn byline(work: &WorkReport) -> String {
    work.creators
        .iter()
        .map(|creator| match role_label(&creator.role) {
            Some(role) => format!("{} ({role})", creator.name.trim()),
            None => creator.name.trim().to_owned(),
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn scheme_label(scheme: &str) -> String {
    match scheme {
        "other" => String::new(),
        scheme => format!("{} ", scheme.to_ascii_uppercase()),
    }
}

/// `ISBN 9788521202981; DOI 10.1/2`.
pub fn identifiers(work: &WorkReport) -> String {
    work.identifiers
        .iter()
        .map(|id| format!("{}{}", scheme_label(&id.scheme), id.value.trim()))
        .collect::<Vec<_>>()
        .join("; ")
}

/// `PDF, 412 páginas`.
pub fn file_line(work: &WorkReport) -> String {
    let format = work.format.to_uppercase();
    match work.pages {
        Some(1) => format!("{format}, 1 página"),
        Some(pages) => format!("{format}, {pages} páginas"),
        None => format,
    }
}

/// `12 (7 realces, 3 sublinhados e 2 notas); 1 não encontrada`.
pub fn counts(work: &WorkReport) -> String {
    let total = work.items.len();
    let orphans = work
        .items
        .iter()
        .filter(|item| item.found == Found::Orphan)
        .count();
    let mut line = format!("{total} ({})", order::tally(&work.items));
    if orphans > 0 {
        line.push_str(&format!(
            "; {orphans} {}",
            if orphans == 1 {
                "não encontrada"
            } else {
                "não encontradas"
            }
        ));
    }
    line
}

/// The line over an annotation: what it is, its color, where it is, and when it was made.
pub fn meta(item: &Item, layout: Layout, offset: i32) -> Vec<String> {
    let mut parts = vec![color_name(&item.color)];
    if let Some(place) = item.place(layout) {
        parts.push(if item.found == Found::Orphan {
            format!("estava em {place}")
        } else {
            place
        });
    }
    if item.found == Found::Approximate {
        parts.push("aproximada".to_owned());
    }
    parts.push(time::date(item.created_at, offset));
    parts
}

/// Whether a mark was drawn on the picture of a page rather than on its text.
pub fn graphic(item: &Item) -> bool {
    !matches!(item.mark, Mark::Text)
}

fn item_markdown(item: &Item, work: &WorkReport, offset: i32, shift: u8) -> String {
    let mut parts = vec![format!(
        "**{}** · {}",
        item.label(),
        meta(item, work.layout, offset)
            .iter()
            .map(|part| escape(part, false))
            .collect::<Vec<_>>()
            .join(" · ")
    )];
    match item
        .quote
        .as_deref()
        .filter(|quote| !quote.trim().is_empty())
    {
        Some(quote) => parts.push(quotation(quote)),
        None if graphic(item) => {
            parts.push("*Sem texto: uma marca desenhada na página.*".to_owned())
        }
        None => {}
    }
    if !item.note.trim().is_empty() {
        parts.push(note_markdown(&item.note, shift));
    }
    parts.join("\n\n")
}

fn heading_line(level: usize, text: &str) -> String {
    format!(
        "{} {}",
        "#".repeat(level.min(6)),
        escape(&crate::text::fold_spaces(text), false)
    )
}

fn work_markdown(work: &WorkReport, base: usize, report: &Report) -> String {
    let offset = report.utc_offset_minutes;
    let mut out: Vec<String> = vec![heading_line(base, &work.title)];
    let by = byline(work);
    if !by.is_empty() {
        out.push(format!("*{}*", escape(&by, false)));
    }
    let mut facts = Vec::new();
    if let Some(series) = work.series.as_deref().filter(|s| !s.trim().is_empty()) {
        let position = work
            .series_position
            .map(|n| format!(", n.º {}", number(n)))
            .unwrap_or_default();
        facts.push(format!(
            "- **Série:** {}",
            escape(&format!("{}{position}", series.trim()), false)
        ));
    }
    if let Some(year) = work.year {
        facts.push(format!("- **Ano:** {year}"));
    }
    if let Some(language) = work.language.as_deref().filter(|l| !l.trim().is_empty()) {
        facts.push(format!("- **Idioma:** {}", escape(language.trim(), false)));
    }
    if !work.identifiers.is_empty() {
        facts.push(format!(
            "- **Identificadores:** {}",
            escape(&identifiers(work), false)
        ));
    }
    facts.push(format!(
        "- **Arquivo:** {}",
        escape(&file_line(work), false)
    ));
    facts.push(format!("- **Anotações:** {}", escape(&counts(work), false)));
    out.push(facts.join("\n"));

    let shift = (base + 3).min(5) as u8;
    let Ordered {
        whole,
        sections,
        orphans,
    } = order::order(work);
    if !whole.is_empty() {
        out.push(heading_line(base + 1, "Sobre o livro todo"));
        for item in whole {
            out.push(item_markdown(item, work, offset, shift));
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
            out.push(heading_line(base + 1 + depth.min(2), &heading.label));
        }
        printed = section.path.clone();
        for item in &section.items {
            out.push(item_markdown(item, work, offset, shift));
        }
    }
    if !orphans.is_empty() {
        out.push(heading_line(base + 1, "Não encontradas no livro"));
        out.push(
            "*O trecho destas anotações não foi encontrado no livro como ele está agora. Elas \
             ficam com a citação, a nota e o lugar em que estavam.*"
                .to_owned(),
        );
        for item in orphans {
            out.push(item_markdown(item, work, offset, shift));
        }
    }
    out.join("\n\n")
}

/// The report as Markdown.
pub fn markdown(report: &Report) -> String {
    let offset = report.utc_offset_minutes;
    let generated = time::iso_local(report.generated_at, offset);
    let mut out: Vec<String> = Vec::new();
    let mut front = vec!["---".to_owned()];
    let total: usize = report.works.iter().map(|work| work.items.len()).sum();
    let made = format!(
        "*Relatório de estudo gerado em {} ({}) pelo {}.*",
        time::date_time(report.generated_at, offset),
        time::offset_label(offset),
        escape(&report.generator, false)
    );
    if let [work] = report.works.as_slice() {
        front.push(format!("title: {}", yaml_string(&work.title)));
        let authors: Vec<String> = work
            .creators
            .iter()
            .map(|c| c.name.trim().to_owned())
            .collect();
        if !authors.is_empty() {
            front.push(format!("authors: {}", yaml_list(&authors)));
        }
        if let Some(series) = &work.series {
            front.push(format!("series: {}", yaml_string(series)));
        }
        if let Some(position) = work.series_position {
            front.push(format!("series_index: {}", number(position)));
        }
        if let Some(year) = work.year {
            front.push(format!("year: {year}"));
        }
        if let Some(language) = &work.language {
            front.push(format!("language: {}", yaml_string(language)));
        }
        if !work.identifiers.is_empty() {
            let ids: Vec<String> = work
                .identifiers
                .iter()
                .map(|id| format!("{}:{}", id.scheme, id.value.trim()))
                .collect();
            front.push(format!("identifiers: {}", yaml_list(&ids)));
        }
        front.push(format!("format: {}", yaml_string(&work.format)));
        front.push(format!("annotations: {total}"));
        front.push(format!("generated: {}", yaml_string(&generated)));
        front.push(format!("generator: {}", yaml_string(&report.generator)));
        front.push("---".to_owned());
        out.push(front.join("\n"));
        out.push(work_markdown(work, 1, report));
        out.push(made);
    } else {
        front.push(format!("title: {}", yaml_string("Relatório de estudo")));
        let titles: Vec<String> = report.works.iter().map(|w| w.title.clone()).collect();
        front.push(format!("works: {}", yaml_list(&titles)));
        front.push(format!("annotations: {total}"));
        front.push(format!("generated: {}", yaml_string(&generated)));
        front.push(format!("generator: {}", yaml_string(&report.generator)));
        front.push("---".to_owned());
        out.push(front.join("\n"));
        out.push("# Relatório de estudo".to_owned());
        out.push(made);
        let list: Vec<String> = report
            .works
            .iter()
            .map(|work| {
                format!(
                    "- {} ({})",
                    escape(&crate::text::fold_spaces(&work.title), false),
                    escape(&order::tally(&work.items), false)
                )
            })
            .collect();
        if !list.is_empty() {
            out.push(list.join("\n"));
        }
        for work in &report.works {
            out.push(work_markdown(work, 2, report));
        }
    }
    let mut text = out.join("\n\n");
    text.push('\n');
    text
}

/// A report to test with, for the PDF too.
#[cfg(test)]
pub(crate) use tests::sample;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Creator, Identifier, Kind};

    fn item(uuid: &str, kind: Kind, at: f64, quote: &str, note: &str) -> Item {
        Item {
            uuid: uuid.into(),
            kind,
            mark: Mark::Text,
            color: "yellow".into(),
            quote: Some(quote.into()),
            note: note.into(),
            at: Some(at),
            page: Some(at as u32),
            progression: Some(at / 10.0),
            found: Found::Exact,
            created_at: 1_795_466_405_123,
            updated_at: 1_795_466_405_123,
        }
    }

    pub(crate) fn sample() -> Report {
        let mut box_item = item("b", Kind::Highlight, 3.5, "", "Figura importante");
        box_item.mark = Mark::Box;
        box_item.quote = None;
        box_item.color = "green".into();
        let mut orphan = item("o", Kind::Note, 1.0, "trecho que sumiu", "");
        orphan.found = Found::Orphan;
        Report {
            generated_at: 1_795_466_405_123,
            utc_offset_minutes: -180,
            generator: "Musaeum 0.0.1".into(),
            works: vec![WorkReport {
                title: "Física *Básica* #1".into(),
                creators: vec![
                    Creator {
                        name: "H. Moysés Nussenzveig".into(),
                        role: "author".into(),
                    },
                    Creator {
                        name: "Tradutora".into(),
                        role: "translator".into(),
                    },
                ],
                series: Some("Curso".into()),
                series_position: Some(1.0),
                year: Some(2002),
                language: Some("pt-BR".into()),
                identifiers: vec![Identifier {
                    scheme: "isbn".into(),
                    value: "9788521202981".into(),
                }],
                format: "pdf".into(),
                layout: Layout::Fixed,
                pages: Some(12),
                contents: vec![
                    Heading {
                        label: "Capítulo 1".into(),
                        depth: 0,
                        at: 0.0,
                    },
                    Heading {
                        label: "Seção 1.1".into(),
                        depth: 1,
                        at: 2.0,
                    },
                ],
                items: vec![
                    item(
                        "q2",
                        Kind::Underline,
                        2.5,
                        "1. Ação & reação: $F = ma$ == <b>",
                        "",
                    ),
                    item(
                        "q1",
                        Kind::Highlight,
                        0.2,
                        "Linha um\nlinha dois\n\nOutro parágrafo",
                        "# Título da nota\n\n**forte** e *leve* [site](https://exemplo.org) [x](javascript:alert(1))\n\n```\nnunca fechado",
                    ),
                    box_item,
                    orphan,
                ],
            }],
        }
    }

    #[test]
    fn a_report_reads_as_the_book_does() {
        let text = markdown(&sample());
        let expected = "---
title: \"Física *Básica* #1\"
authors: [\"H. Moysés Nussenzveig\", \"Tradutora\"]
series: \"Curso\"
series_index: 1
year: 2002
language: \"pt-BR\"
identifiers: [\"isbn:9788521202981\"]
format: \"pdf\"
annotations: 4
generated: \"2026-11-23T17:40:05-03:00\"
generator: \"Musaeum 0.0.1\"
---

# Física \\*Básica\\* \\#1

*H. Moysés Nussenzveig; Tradutora (trad.)*

- **Série:** Curso, n.º 1
- **Ano:** 2002
- **Idioma:** pt-BR
- **Identificadores:** ISBN 9788521202981
- **Arquivo:** PDF, 12 páginas
- **Anotações:** 4 (1 realce, 1 sublinhado, 1 nota e 1 caixa); 1 não encontrada

## Capítulo 1

**Realce** · amarelo · p. 1 · 23/11/2026

> Linha um
> linha dois
>
> Outro parágrafo

##### Título da nota

**forte** e *leve* [site](https://exemplo.org) x (javascript:alert(1))

```
nunca fechado
```

### Seção 1.1

**Sublinhado** · amarelo · p. 3 · 23/11/2026

> 1\\. Ação & reação: \\$F = ma\\$ \\=\\= \\<b\\>

**Caixa** · verde · p. 4 · 23/11/2026

*Sem texto: uma marca desenhada na página.*

Figura importante

## Não encontradas no livro

*O trecho destas anotações não foi encontrado no livro como ele está agora. Elas ficam com a citação, a nota e o lugar em que estavam.*

**Nota** · amarelo · estava em p. 2 · 23/11/2026

> trecho que sumiu

*Relatório de estudo gerado em 23/11/2026 às 17:40 (UTC\u{2212}03:00) pelo Musaeum 0.0.1.*
";
        if text != expected {
            for (a, b) in text.lines().zip(expected.lines()) {
                assert_eq!(a, b);
            }
            assert_eq!(text, expected);
        }
        assert_eq!(markdown(&sample()), text, "the same every time");
    }

    #[test]
    fn what_a_book_says_stays_text() {
        assert_eq!(escape("- item", true), "\\- item");
        assert_eq!(escape("- item", false), "- item");
        assert_eq!(escape("12) doze", true), "12\\) doze");
        assert_eq!(escape("3.14 é pi", true), "3.14 é pi");
        assert_eq!(
            escape("#tag [[link]] %%x%% a&amp;b a & b", false),
            "\\#tag \\[\\[link\\]\\] \\%\\%x\\%\\% a\\&amp;b a & b"
        );
        assert_eq!(
            note_markdown("```\na```b\n```", 4),
            "````\na```b\n````",
            "backticks inside"
        );
        assert_eq!(
            note_markdown("- - x", 4),
            "- \\- x",
            "text, not a list inside a list"
        );
        assert_eq!(note_markdown("- a\n- b\n2. c", 4), "- a\n- b\n- c");
        assert_eq!(note_markdown("1. a\n- b", 4), "1. a\n2. b");
    }
}

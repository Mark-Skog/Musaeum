//! A note's Markdown, read exactly as the reader's own renderer reads it
//! (`apps/desktop/src/app/reader/annotations/markdown.tsx`): paragraphs, headings, lists, quotes,
//! code, emphasis and links, and nothing else. What the reader shows as text, a report shows as
//! text too, so a note looks the same in the app, in the Markdown and in the PDF.
//!
//! The rules are those of the regular expressions there, character for character, with
//! JavaScript's idea of white space.

/// A block of a note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Heading { level: u8, text: String },
    Paragraph(String),
    List { ordered: bool, items: Vec<String> },
    Quote(String),
    Code(String),
}

/// A piece of a block's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inline {
    Text(String),
    Code(String),
    Strong(Vec<Inline>),
    Emphasis(Vec<Inline>),
    Link { label: String, href: String },
}

/// JavaScript's `\s`: its white space and line terminators.
pub fn js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

/// JavaScript's `.`: anything but a line terminator.
fn js_dot(c: char) -> bool {
    !matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

fn js_trim(text: &str) -> &str {
    text.trim_matches(js_space)
}

/// `^(#{1,6})\s+(.*)$`: the level and the text, trimmed.
fn heading(line: &str) -> Option<(u8, String)> {
    let hashes = line.chars().take_while(|&c| c == '#').count();
    if hashes == 0 {
        return None;
    }
    // With seven or more, no shorter run of `#` is followed by white space.
    if hashes > 6 {
        return None;
    }
    let rest = &line[hashes..];
    let text = rest.trim_start_matches(js_space);
    if text.len() == rest.len() {
        return None;
    }
    // `(.*)$` takes the rest only if no line terminator is in it. Backtracking `\s+` would only
    // hand it more white space from the front, and never take one out of what follows.
    text.chars()
        .all(js_dot)
        .then(|| (hashes as u8, js_trim(text).to_owned()))
}

/// `^\s*([-*+]|\d+[.)])\s+`: the length of a list item's marker with the spaces around it.
fn list_marker(line: &str) -> Option<(usize, bool)> {
    let mut chars = line.char_indices().peekable();
    while chars.peek().is_some_and(|&(_, c)| js_space(c)) {
        chars.next();
    }
    let ordered = match chars.peek() {
        Some(&(_, '-' | '*' | '+')) => {
            chars.next();
            false
        }
        Some(&(_, c)) if c.is_ascii_digit() => {
            while chars.peek().is_some_and(|&(_, c)| c.is_ascii_digit()) {
                chars.next();
            }
            match chars.peek() {
                Some(&(_, '.' | ')')) => {
                    chars.next();
                }
                _ => return None,
            }
            true
        }
        _ => return None,
    };
    let mut spaces = 0;
    while let Some(&(_, c)) = chars.peek() {
        if !js_space(c) {
            break;
        }
        chars.next();
        spaces += 1;
    }
    if spaces == 0 {
        return None;
    }
    let end = chars.peek().map_or(line.len(), |&(at, _)| at);
    Some((end, ordered))
}

/// `^(#{1,6}\s|```|>|\s*([-*+]|\d+[.)])\s+)`: whether a line ends a paragraph.
fn ends_paragraph(line: &str) -> bool {
    // `#{1,6}\s` needs the whole run of `#`: a shorter one is followed by another `#`.
    let hashes = line.chars().take_while(|&c| c == '#').count();
    let heading_start =
        (1..=6).contains(&hashes) && line[hashes..].chars().next().is_some_and(js_space);
    heading_start || line.starts_with("```") || line.starts_with('>') || list_marker(line).is_some()
}

/// The blocks of a note.
pub fn blocks(source: &str) -> Vec<Block> {
    let normalized = source.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();
    let mut blocks = Vec::new();
    let mut at = 0;
    while at < lines.len() {
        let line = lines[at];
        if js_trim(line).is_empty() {
            at += 1;
            continue;
        }
        if line.starts_with("```") {
            let mut code = Vec::new();
            at += 1;
            while at < lines.len() && !lines[at].starts_with("```") {
                code.push(lines[at]);
                at += 1;
            }
            at += 1;
            blocks.push(Block::Code(code.join("\n")));
            continue;
        }
        if let Some((level, text)) = heading(line) {
            blocks.push(Block::Heading { level, text });
            at += 1;
            continue;
        }
        if let Some((_, ordered)) = list_marker(line) {
            let mut items = Vec::new();
            while at < lines.len() {
                let Some((end, _)) = list_marker(lines[at]) else {
                    break;
                };
                items.push(lines[at][end..].to_owned());
                at += 1;
            }
            blocks.push(Block::List { ordered, items });
            continue;
        }
        if line.starts_with('>') {
            let mut quoted = Vec::new();
            while at < lines.len() && lines[at].starts_with('>') {
                let rest = &lines[at][1..];
                let rest = match rest.chars().next() {
                    Some(c) if js_space(c) => &rest[c.len_utf8()..],
                    _ => rest,
                };
                quoted.push(rest);
                at += 1;
            }
            blocks.push(Block::Quote(quoted.join(" ")));
            continue;
        }
        let mut paragraph = Vec::new();
        while at < lines.len() && !js_trim(lines[at]).is_empty() && !ends_paragraph(lines[at]) {
            paragraph.push(js_trim(lines[at]));
            at += 1;
        }
        if paragraph.is_empty() {
            // A line that starts like a heading but is not one (a line terminator inside it): the
            // reader's loop would not move past it. It is a paragraph of its own here.
            paragraph.push(js_trim(line));
            at += 1;
        }
        blocks.push(Block::Paragraph(paragraph.join(" ")));
    }
    blocks
}

/// One of the inline patterns at the start of `text`, and how long it is, in bytes:
/// `` `[^`]+` ``, `\*\*[^*]+\*\*|__[^_]+__`, `\*[^*\s][^*]*\*|_[^_\s][^_]*_` and
/// `\[[^\]]+\]\([^)\s]+\)`, tried in that order as the alternation tries them.
fn token_at(text: &str) -> Option<(Inline, usize)> {
    // `` `x` ``: up to the next backtick, at least one character between.
    if let Some(rest) = text.strip_prefix('`')
        && let Some(end) = rest.find('`')
        && end > 0
    {
        return Some((Inline::Code(rest[..end].to_owned()), end + 2));
    }
    for mark in ["**", "__"] {
        let single = mark.chars().next().unwrap_or('*');
        if let Some(rest) = text.strip_prefix(mark) {
            let end = rest.find(single).unwrap_or(rest.len());
            if end > 0 && rest[end..].starts_with(mark) {
                let inner = &rest[..end];
                return Some((Inline::Strong(inline(inner)), end + 4));
            }
        }
    }
    for single in ['*', '_'] {
        if let Some(rest) = text.strip_prefix(single)
            && let Some(first) = rest.chars().next()
            && first != single
            && !js_space(first)
            && let Some(end) = rest.find(single)
        {
            let inner = &rest[..end];
            return Some((Inline::Emphasis(inline(inner)), end + 2));
        }
    }
    if let Some(rest) = text.strip_prefix('[')
        && let Some(close) = rest.find(']')
        && close > 0
        && rest[close + 1..].starts_with('(')
    {
        let after = &rest[close + 2..];
        let end = after
            .find(|c: char| c == ')' || js_space(c))
            .unwrap_or(after.len());
        if end > 0 && after[end..].starts_with(')') {
            let label = rest[..close].to_owned();
            let href = after[..end].to_owned();
            return Some((Inline::Link { label, href }, 1 + close + 2 + end + 1));
        }
    }
    None
}

/// Emphasis, code and links in a block's text.
pub fn inline(text: &str) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut plain = String::new();
    let mut at = 0;
    while at < text.len() {
        if let Some((token, length)) = token_at(&text[at..]) {
            if !plain.is_empty() {
                out.push(Inline::Text(std::mem::take(&mut plain)));
            }
            out.push(token);
            at += length;
            continue;
        }
        let c = text[at..].chars().next().unwrap_or(' ');
        plain.push(c);
        at += c.len_utf8();
    }
    if !plain.is_empty() {
        out.push(Inline::Text(plain));
    }
    out
}

/// Whether a link of a note leads somewhere the reader would offer to go: the web, mail or a phone.
pub fn linkable(href: &str) -> bool {
    let lower = href.to_ascii_lowercase();
    ["http:", "https:", "mailto:", "tel:"]
        .iter()
        .any(|scheme| lower.starts_with(scheme))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> Inline {
        Inline::Text(value.to_owned())
    }

    /// A block as the reader's parser writes it.
    fn as_json(block: &Block) -> serde_json::Value {
        use serde_json::json;
        match block {
            Block::Heading { level, text } => {
                json!({"type": "heading", "level": level, "text": text})
            }
            Block::Paragraph(text) => json!({"type": "paragraph", "text": text}),
            Block::List { ordered, items } => {
                json!({"type": "list", "ordered": ordered, "items": items})
            }
            Block::Quote(text) => json!({"type": "quote", "text": text}),
            Block::Code(text) => json!({"type": "code", "text": text}),
        }
    }

    #[test]
    fn every_note_of_the_shared_cases_reads_as_in_the_reader() {
        let shared: serde_json::Value = serde_json::from_str(include_str!(
            "../../../apps/desktop/src/app/reader/annotations/notes-cases.json"
        ))
        .unwrap();
        for case in shared["cases"].as_array().unwrap() {
            let note = case["note"].as_str().unwrap();
            let read: Vec<serde_json::Value> = blocks(note).iter().map(as_json).collect();
            assert_eq!(serde_json::Value::Array(read), case["blocks"], "{note:?}");
        }
    }

    #[test]
    fn blocks_are_read_as_the_reader_reads_them() {
        let note = "# Título\n\nUm parágrafo\nem duas linhas.\n\n- um\n- dois\n1. três\n\n> citado\n> e mais\n\n```\nlet x = 1;\n  y\n```\n####### sete\n";
        assert_eq!(
            blocks(note),
            vec![
                Block::Heading {
                    level: 1,
                    text: "Título".into()
                },
                Block::Paragraph("Um parágrafo em duas linhas.".into()),
                Block::List {
                    ordered: false,
                    items: vec!["um".into(), "dois".into(), "três".into()]
                },
                Block::Quote("citado e mais".into()),
                Block::Code("let x = 1;\n  y".into()),
                Block::Paragraph("####### sete".into()),
            ]
        );
        assert_eq!(
            blocks("```\nnunca fechado\n# não é título"),
            vec![Block::Code("nunca fechado\n# não é título".into())]
        );
        assert_eq!(
            blocks("#semespaço"),
            vec![Block::Paragraph("#semespaço".into())]
        );
        assert_eq!(
            blocks("1.x\n2) y"),
            vec![
                Block::Paragraph("1.x".into()),
                Block::List {
                    ordered: true,
                    items: vec!["y".into()]
                }
            ]
        );
    }

    #[test]
    fn a_line_that_only_starts_like_a_heading_is_text() {
        // `.` stops at U+2028, so this is no heading, yet it starts like one.
        assert_eq!(
            blocks("# a\u{2028}b\nfim"),
            vec![
                Block::Paragraph("# a\u{2028}b".into()),
                Block::Paragraph("fim".into())
            ]
        );
        assert_eq!(
            blocks("# \u{2028}x"),
            vec![Block::Heading {
                level: 1,
                text: "x".into()
            }],
            "the white space before the text may hold a line terminator"
        );
    }

    #[test]
    fn inline_marks_are_the_reader_s() {
        assert_eq!(
            inline("um **forte** e *leve*, `código` e [link](https://x.org/a)."),
            vec![
                text("um "),
                Inline::Strong(vec![text("forte")]),
                text(" e "),
                Inline::Emphasis(vec![text("leve")]),
                text(", "),
                Inline::Code("código".into()),
                text(" e "),
                Inline::Link {
                    label: "link".into(),
                    href: "https://x.org/a".into()
                },
                text("."),
            ]
        );
        assert_eq!(
            inline("2 * 3 * 4"),
            vec![text("2 * 3 * 4")],
            "a star before a space opens nothing"
        );
        assert_eq!(
            inline("**a*b**"),
            vec![text("*"), Inline::Emphasis(vec![text("a")]), text("b**")]
        );
        assert_eq!(
            inline("__forte__ _leve_"),
            vec![
                Inline::Strong(vec![text("forte")]),
                text(" "),
                Inline::Emphasis(vec![text("leve")])
            ]
        );
        assert_eq!(
            inline("[a](b c)"),
            vec![text("[a](b c)")],
            "no space in an address"
        );
        assert_eq!(inline("``"), vec![text("``")]);
        assert!(
            linkable("HTTPS://x") && linkable("mailto:a@b") && !linkable("javascript:alert(1)")
        );
    }
}

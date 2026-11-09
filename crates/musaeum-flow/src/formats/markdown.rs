//! Markdown, as CommonMark with GitHub's tables, footnotes, strikethrough and task lists.
//!
//! A leading `---` block gives the title, the author and the language. Every first-level heading
//! starts a document when there are several, and headings down to the third level make the table
//! of contents. Raw HTML is allowed in Markdown, so it goes through the same sanitizer as any book.
//! Images can only come along as `data:` URLs: the worker sees this one file and nothing next to it.

use pulldown_cmark::{Options, Parser, html};

use super::{Markup, parse_markup, retarget_fragment_links};
use crate::dom::{Dom, Name, NodeId, Ns};
use crate::package::{Contributor, SourceFormat, Title};
use crate::source::{
    Limits, NormalizeError, Result, SourceBook, SourceDocument, SourceNav, clean_field, fold_space,
};

/// The front matter's `key: value` lines, and the text after it.
fn front_matter(text: &str) -> (Vec<(String, String)>, &str) {
    let Some(rest) = text.strip_prefix("---\n") else {
        return (Vec::new(), text);
    };
    let Some(end) = rest.find("\n---\n").or_else(|| rest.find("\n...\n")) else {
        return (Vec::new(), text);
    };
    let fields = rest[..end]
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once(':')?;
            let value = value.trim().trim_matches(['"', '\'']).to_owned();
            Some((key.trim().to_ascii_lowercase(), value))
        })
        .collect();
    (fields, &rest[end + 5..])
}

fn heading_level(dom: &Dom, node: NodeId) -> Option<usize> {
    let name = dom.name(node)?;
    if name.ns != Ns::Html {
        return None;
    }
    match name.local.as_str() {
        "h1" => Some(1),
        "h2" => Some(2),
        "h3" => Some(3),
        _ => None,
    }
}

/// Builds the table of contents from headings, nested by level.
fn add_to_toc(toc: &mut Vec<SourceNav>, level: usize, entry: SourceNav) {
    let mut list = toc;
    for _ in 1..level {
        if list.last().is_none() {
            break;
        }
        list = &mut list.last_mut().expect("an entry").children;
    }
    list.push(entry);
}

pub fn read(bytes: &[u8], limits: &Limits) -> Result<SourceBook> {
    let decoded = crate::encoding::decode_guess(bytes);
    let text = decoded.text.replace("\r\n", "\n").replace('\r', "\n");
    let (fields, body_text) = front_matter(text.trim_start_matches('\u{feff}'));
    let mut book = SourceBook::new(SourceFormat::Markdown);
    for (key, value) in &fields {
        match key.as_str() {
            "title" => book
                .metadata
                .titles
                .extend(clean_field(value, 1000).map(|value| Title {
                    value,
                    ..Title::default()
                })),
            "author" | "authors" => {
                for name in value.trim_matches(['[', ']']).split([',', ';']) {
                    if let Some(name) = clean_field(name.trim_matches(['"', '\'', ' ']), 500) {
                        book.metadata.creators.push(Contributor {
                            name,
                            ..Contributor::default()
                        });
                    }
                }
            }
            "lang" | "language" => book.metadata.languages.extend(clean_field(value, 64)),
            "date" => book.metadata.published = clean_field(value, 64),
            "description" => book.metadata.description = clean_field(value, 20_000),
            _ => {}
        }
    }

    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_HEADING_ATTRIBUTES);
    let mut rendered = String::with_capacity(body_text.len() * 2);
    html::push_html(&mut rendered, Parser::new_ext(body_text, options));
    let page =
        format!("<!DOCTYPE html><html><head><title></title></head><body>{rendered}</body></html>");
    let mut warning = None;
    let (whole, _) = parse_markup(&page, Markup::Html, limits, &mut warning).ok_or_else(|| {
        NormalizeError::TooLarge(warning.unwrap_or_else(|| "the document".to_owned()))
    })?;
    let body = whole
        .body()
        .ok_or_else(|| NormalizeError::Malformed("no body".to_owned()))?;

    // Every heading gets an id, so the table of contents can point at it.
    let mut count = 0usize;
    let headings: Vec<NodeId> = whole
        .descendants(body)
        .filter(|&node| heading_level(&whole, node).is_some())
        .collect();
    let mut whole = whole;
    for &heading in &headings {
        if whole.attr(heading, "id").is_none() {
            count += 1;
            whole.set_attr(heading, Name::plain("id"), format!("section-{count}"));
        }
    }
    if book.metadata.titles.is_empty() {
        let first = headings
            .iter()
            .find(|&&h| heading_level(&whole, h) == Some(1))
            .or(headings.first())
            .map(|&h| fold_space(&whole.text_content(h)));
        book.metadata.titles.push(Title {
            value: first
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| "Markdown".to_owned()),
            ..Title::default()
        });
    }
    let title = book.metadata.titles[0].value.clone();

    // Documents start at first-level headings, if there are several.
    let children: Vec<NodeId> = whole.children(body).to_vec();
    let firsts = children
        .iter()
        .filter(|&&c| heading_level(&whole, c) == Some(1))
        .count();
    let mut groups: Vec<Vec<NodeId>> = vec![Vec::new()];
    for child in children {
        if firsts > 1
            && heading_level(&whole, child) == Some(1)
            && !groups.last().expect("a group").is_empty()
        {
            groups.push(Vec::new());
        }
        groups.last_mut().expect("a group").push(child);
    }
    for (index, group) in groups.into_iter().enumerate() {
        let path = format!("text/{index:04}.xhtml");
        let page_title = group
            .iter()
            .find(|&&c| heading_level(&whole, c).is_some())
            .map(|&c| fold_space(&whole.text_content(c)))
            .unwrap_or_else(|| title.clone());
        let (mut dom, new_body) = super::empty_page(
            &page_title,
            book.metadata.languages.first().map(String::as_str),
        );
        for node in group {
            let copy = whole.copy_into(node, &mut dom);
            dom.append(new_body, copy);
        }
        dom.normalize_text(Dom::DOCUMENT);
        for heading in dom.descendants(new_body).collect::<Vec<_>>() {
            if let Some(level) = heading_level(&dom, heading) {
                add_to_toc(
                    &mut book.toc,
                    level,
                    SourceNav {
                        label: fold_space(&dom.text_content(heading)),
                        target: Some((path.clone(), dom.attr(heading, "id").map(str::to_owned))),
                        ..SourceNav::default()
                    },
                );
            }
        }
        book.documents.push(SourceDocument {
            path,
            id: None,
            linear: true,
            properties: Vec::new(),
            dom,
            parsed: crate::source::Parsed::Generated,
            svg: false,
        });
    }
    retarget_fragment_links(&mut book.documents);
    Ok(book)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_matter_headings_and_footnotes() {
        let text = "---\ntitle: \"A Guide\"\nauthor: Ana, Bruno\nlang: pt-BR\n---\n# One\n\nText with a note.[^n]\n\n## Detail\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n# Two\n\nMore <script>alert(1)</script> text.\n\n[^n]: The note.\n";
        let book = read(text.as_bytes(), &Limits::default()).unwrap();
        assert_eq!(book.metadata.title(), Some("A Guide"));
        assert_eq!(book.metadata.creators.len(), 2);
        assert_eq!(book.metadata.languages, ["pt-BR"]);
        assert_eq!(book.documents.len(), 2);
        assert_eq!(book.toc.len(), 2);
        assert_eq!(book.toc[0].children.len(), 1);
        // The note's definition ends up in the second document; the link follows it there.
        let first = &book.documents[0].dom;
        let link = first
            .descendants(Dom::DOCUMENT)
            .find(|&n| first.is_html(n, "a"))
            .unwrap();
        assert!(
            first.attr(link, "href").unwrap().starts_with("0001.xhtml#"),
            "{:?}",
            first.attr(link, "href")
        );
        let table = first
            .descendants(Dom::DOCUMENT)
            .any(|n| first.is_html(n, "table"));
        assert!(table);
    }
}

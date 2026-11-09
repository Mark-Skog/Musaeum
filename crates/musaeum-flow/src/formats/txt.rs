//! Plain text: its encoding guessed, its paragraphs found, and its chapters too when their headings
//! are unmistakable.
//!
//! Two layouts are common. Text with blank lines between paragraphs is often wrapped at a fixed
//! width, so a paragraph's lines are joined. Text without blank lines puts one paragraph on each
//! line. A heading is a short paragraph of one line that starts with a word such as "Chapter" or
//! "Capítulo"; each one starts a document, so the book gets a table of contents and the documents
//! stay small.

use super::{add, empty_page};
use crate::dom::{Dom, NodeId};
use crate::package::{SourceFormat, Title};
use crate::source::{Limits, Parsed, Result, SourceBook, SourceDocument, SourceNav, clean_field};

/// Words that start a heading when a number follows them: "Chapter 12", "Capítulo IV".
const NUMBERED: [&str; 27] = [
    "chapter",
    "chapitre",
    "capítulo",
    "capitulo",
    "kapitel",
    "capitolo",
    "hoofdstuk",
    "глава",
    "part",
    "parte",
    "partie",
    "teil",
    "book",
    "livro",
    "libro",
    "livre",
    "buch",
    "act",
    "acto",
    "ato",
    "scene",
    "cena",
    "escena",
    "section",
    "seção",
    "secção",
    "sección",
];

/// Words that are a heading by themselves.
const ALONE: [&str; 13] = [
    "prologue",
    "prólogo",
    "prologo",
    "epilogue",
    "epílogo",
    "epilogo",
    "preface",
    "prefácio",
    "prefacio",
    "introduction",
    "introdução",
    "introducción",
    "foreword",
];

const NUMBER_WORDS: [&str; 40] = [
    "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven",
    "twelve", "first", "second", "third", "last", "um", "dois", "três", "tres", "quatro", "cinco",
    "seis", "sete", "oito", "nove", "dez", "primeiro", "primeira", "segundo", "segunda",
    "terceiro", "terceira", "uno", "dos", "cuatro", "primero", "último", "ultimo", "final",
];

fn is_roman(word: &str) -> bool {
    !word.is_empty()
        && word.chars().all(|c| {
            matches!(
                c.to_ascii_uppercase(),
                'I' | 'V' | 'X' | 'L' | 'C' | 'D' | 'M'
            )
        })
}

fn is_heading(paragraph: &str) -> bool {
    let line = paragraph.trim();
    if line.is_empty() || line.chars().count() > 80 {
        return false;
    }
    let mut words = line.split_whitespace().map(|word| {
        word.trim_matches(|c: char| matches!(c, '.' | ':' | ',' | '-' | '—'))
            .to_lowercase()
    });
    let first = words.next().unwrap_or_default();
    if ALONE.contains(&first.as_str()) {
        return line.chars().count() <= 60;
    }
    if !NUMBERED.contains(&first.as_str()) {
        return false;
    }
    let Some(second) = words.next() else {
        return false;
    };
    second.chars().all(|c| c.is_ascii_digit()) && !second.is_empty()
        || is_roman(&second)
        || NUMBER_WORDS.contains(&second.as_str())
}

/// The paragraphs of a text.
fn paragraphs(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let blank = lines.iter().filter(|line| line.trim().is_empty()).count();
    let filled = lines.len() - blank;
    // Blank lines between paragraphs, or one paragraph per line.
    let blocks = blank > 0 && blank * 20 >= filled;
    let mut out = Vec::new();
    if blocks {
        let mut current: Vec<&str> = Vec::new();
        for line in lines {
            if line.trim().is_empty() {
                if !current.is_empty() {
                    out.push(
                        current
                            .iter()
                            .map(|l| l.trim())
                            .collect::<Vec<_>>()
                            .join(" "),
                    );
                    current.clear();
                }
            } else {
                current.push(line);
            }
        }
        if !current.is_empty() {
            out.push(
                current
                    .iter()
                    .map(|l| l.trim())
                    .collect::<Vec<_>>()
                    .join(" "),
            );
        }
    } else {
        out.extend(
            lines
                .iter()
                .map(|line| line.trim())
                .filter(|line| !line.is_empty())
                .map(str::to_owned),
        );
    }
    out
}

pub fn read(bytes: &[u8], _limits: &Limits) -> Result<SourceBook> {
    let decoded = crate::encoding::decode_guess(bytes);
    let text = decoded
        .text
        .replace("\r\n", "\n")
        .replace(['\r', '\u{c}'], "\n")
        .replace('\u{0}', "");
    let paragraphs = paragraphs(&text);
    let mut book = SourceBook::new(SourceFormat::Txt);
    book.version = Some(decoded.encoding.to_owned());
    let title = paragraphs
        .first()
        .and_then(|first| clean_field(first, 200))
        .unwrap_or_else(|| "Text".to_owned());
    book.metadata.titles.push(Title {
        value: title.clone(),
        ..Title::default()
    });

    // Documents start at headings. Three or more in a row are a table of contents, not chapters.
    let mut headings: Vec<bool> = paragraphs.iter().map(|p| is_heading(p)).collect();
    let mut run_start = 0;
    for index in 0..=headings.len() {
        if index < headings.len() && headings[index] {
            continue;
        }
        if index - run_start >= 3 {
            // A list ends where a heading repeats one in it: that one is the first real chapter.
            let last = &paragraphs[index - 1];
            let repeats = paragraphs[run_start..index - 1]
                .iter()
                .any(|earlier| earlier.trim() == last.trim());
            let end = if repeats { index - 1 } else { index };
            headings[run_start..end].fill(false);
        }
        run_start = index + 1;
    }
    let mut chapters: Vec<(Option<String>, Vec<String>)> = vec![(None, Vec::new())];
    for (paragraph, heading) in paragraphs.into_iter().zip(headings) {
        if heading {
            chapters.push((Some(paragraph), Vec::new()));
        } else {
            chapters.last_mut().expect("a chapter").1.push(paragraph);
        }
    }
    if chapters[0].1.is_empty() && chapters.len() > 1 {
        chapters.remove(0);
    }
    for (index, (heading, body_paragraphs)) in chapters.into_iter().enumerate() {
        let path = format!("text/{index:04}.xhtml");
        let page_title = heading.clone().unwrap_or_else(|| title.clone());
        let (mut dom, body) = empty_page(&page_title, None);
        if let Some(heading) = &heading {
            let id = format!("chapter-{index}");
            let h2: NodeId = add(&mut dom, body, "h2", &[("id", &id)]);
            dom.append_text(h2, heading);
            book.toc.push(SourceNav {
                label: heading.clone(),
                target: Some((path.clone(), Some(id))),
                ..SourceNav::default()
            });
        }
        for paragraph in body_paragraphs {
            let p = add(&mut dom, body, "p", &[]);
            dom.append_text(p, &paragraph);
        }
        dom.normalize_text(Dom::DOCUMENT);
        book.documents.push(SourceDocument {
            path,
            id: None,
            linear: true,
            properties: Vec::new(),
            dom,
            parsed: Parsed::Generated,
            svg: false,
        });
    }
    Ok(book)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapped_paragraphs_are_joined_and_chapters_found() {
        let text = "My Book\n\nChapter 1\n\nIt was a dark\nand stormy night.\n\nSecond paragraph.\n\nCHAPTER II. The Next\n\nMore.\n";
        let book = read(text.as_bytes(), &Limits::default()).unwrap();
        assert_eq!(book.documents.len(), 3);
        assert_eq!(book.metadata.title(), Some("My Book"));
        let second = &book.documents[1].dom;
        assert!(
            second
                .text_content(second.body().unwrap())
                .contains("It was a dark and stormy night.")
        );
        assert_eq!(book.toc.len(), 2);
        assert_eq!(book.toc[1].label, "CHAPTER II. The Next");
    }

    #[test]
    fn only_unmistakable_headings_count() {
        for heading in [
            "Chapter 1",
            "CHAPTER XII.",
            "Capítulo três",
            "Part One: The Beginning",
            "Prologue",
            "Book IV",
        ] {
            assert!(is_heading(heading), "{heading}");
        }
        for text in [
            "Part of me knew.",
            "Chapter and verse were quoted.",
            "The Prologue to the tale was longer than this line is allowed to be, by far",
        ] {
            assert!(!is_heading(text), "{text}");
        }
    }

    #[test]
    fn a_list_of_chapters_is_not_a_run_of_chapters() {
        let text = "Contents

Chapter 1

Chapter 2

Chapter 3

Chapter 1

One.

Chapter 2

Two.
";
        let book = read(text.as_bytes(), &Limits::default()).unwrap();
        assert_eq!(book.toc.len(), 2);
        assert_eq!(book.documents.len(), 3);
    }

    #[test]
    fn one_paragraph_per_line_without_blank_lines() {
        let text = "First line.\nSecond line.\nThird line.\n";
        let book = read(text.as_bytes(), &Limits::default()).unwrap();
        let dom = &book.documents[0].dom;
        assert_eq!(dom.element_children(dom.body().unwrap()).count(), 3);
    }

    #[test]
    fn the_encoding_is_guessed() {
        let latin = b"Cora\xe7\xe3o\n\nN\xe3o h\xe1 nada a temer, cora\xe7\xe3o valente.\n";
        let book = read(latin, &Limits::default()).unwrap();
        assert_eq!(book.metadata.title(), Some("Coração"));
    }
}

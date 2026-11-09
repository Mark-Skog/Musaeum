//! FictionBook 2: one XML file with the metadata, the text as nested sections, notes in a second
//! body and pictures in base64. A `.fb2.zip` holding one is read too.
//!
//! Each top-level section of the main body becomes a document, as do the notes; titles become
//! headings by depth, poems and epigraphs get classes a stylesheet can find.

use super::zip::Archive;
use super::{base64_decode, empty_page, retarget_fragment_links};
use crate::dom::{Attr, Dom, Name, NodeId, Ns};
use crate::package::{Contributor, Identifier, Series, SourceFormat, Title};
use crate::source::{
    Limits, NormalizeError, Parsed, Result, SourceBook, SourceDocument, SourceNav, SourceResource,
    clean_field, fold_space,
};
use crate::url;

const MAX_FIELD: usize = 4096;

fn local(dom: &Dom, node: NodeId) -> &str {
    dom.name(node).map_or("", |name| name.local.as_str())
}

fn child<'d>(dom: &'d Dom, node: NodeId, name: &'d str) -> Option<NodeId> {
    dom.element_children(node).find(|&n| local(dom, n) == name)
}

fn children<'d>(dom: &'d Dom, node: NodeId, name: &'d str) -> impl Iterator<Item = NodeId> + 'd {
    dom.element_children(node)
        .filter(move |&n| local(dom, n) == name)
}

fn text(dom: &Dom, node: NodeId) -> String {
    fold_space(&dom.text_content(node))
}

fn href(dom: &Dom, node: NodeId) -> Option<&str> {
    dom.attr_ns(node, &Ns::Xlink, "href")
        .or_else(|| dom.attr(node, "href"))
}

fn person(dom: &Dom, node: NodeId) -> Option<Contributor> {
    let part = |name: &str| {
        child(dom, node, name)
            .map(|n| text(dom, n))
            .filter(|t| !t.is_empty())
    };
    let (first, middle, last) = (part("first-name"), part("middle-name"), part("last-name"));
    let name = [first.clone(), middle, last.clone()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    let name = if name.is_empty() {
        part("nickname")?
    } else {
        name
    };
    Some(Contributor {
        name: clean_field(&name, MAX_FIELD)?,
        roles: Vec::new(),
        file_as: last.map(|last| match first {
            Some(first) => format!("{last}, {first}"),
            None => last,
        }),
    })
}

struct Converter<'a> {
    source: &'a Dom,
    /// Binary ids that exist, so images can point at them.
    binaries: &'a std::collections::HashSet<String>,
    sections: usize,
}

const INLINE: [&str; 9] = [
    "strong",
    "emphasis",
    "style",
    "strikethrough",
    "sub",
    "sup",
    "code",
    "a",
    "image",
];

impl Converter<'_> {
    fn element(out: &mut Dom, parent: NodeId, local: &str, attrs: Vec<Attr>) -> NodeId {
        let node = out.create_element(Name::html(local), attrs);
        out.append(parent, node);
        node
    }

    fn attrs(&self, node: NodeId, class: Option<&str>) -> Vec<Attr> {
        let mut attrs = Vec::new();
        if let Some(id) = self.source.attr(node, "id") {
            attrs.push(Attr {
                name: Name::plain("id"),
                value: id.to_owned(),
            });
        }
        if let Some(class) = class {
            attrs.push(Attr {
                name: Name::plain("class"),
                value: class.to_owned(),
            });
        }
        attrs
    }

    fn image(&self, node: NodeId, out: &mut Dom, parent: NodeId, inline: bool) {
        let Some(target) = href(self.source, node).and_then(|h| h.strip_prefix('#')) else {
            return;
        };
        let target = url::percent_decode(target);
        if !self.binaries.contains(&target) {
            return;
        }
        let mut attrs = vec![
            Attr {
                name: Name::plain("src"),
                value: url::encode_path(&target),
            },
            Attr {
                name: Name::plain("alt"),
                value: self.source.attr(node, "alt").unwrap_or("").to_owned(),
            },
        ];
        if let Some(id) = self.source.attr(node, "id") {
            attrs.push(Attr {
                name: Name::plain("id"),
                value: id.to_owned(),
            });
        }
        if inline {
            Self::element(out, parent, "img", attrs);
        } else {
            let div = Self::element(
                out,
                parent,
                "div",
                vec![Attr {
                    name: Name::plain("class"),
                    value: "image".to_owned(),
                }],
            );
            Self::element(out, div, "img", attrs);
        }
    }

    /// Converts `node`'s children into `parent`. `depth` is the section depth, for headings.
    fn children(
        &mut self,
        node: NodeId,
        out: &mut Dom,
        parent: NodeId,
        depth: usize,
        toc: &mut Vec<SourceNav>,
        path: &str,
    ) {
        let kids: Vec<NodeId> = self.source.children(node).to_vec();
        for kid in kids {
            if let Some(value) = self.source.text(kid) {
                out.append_text(parent, value);
                continue;
            }
            self.convert(kid, out, parent, depth, toc, path);
        }
    }

    fn convert(
        &mut self,
        node: NodeId,
        out: &mut Dom,
        parent: NodeId,
        depth: usize,
        toc: &mut Vec<SourceNav>,
        path: &str,
    ) {
        if depth > 64 {
            return;
        }
        let name = local(self.source, node).to_owned();
        let parent_is_inline = out.name(parent).is_some_and(|n| {
            matches!(
                n.local.as_str(),
                "p" | "span" | "em" | "strong" | "a" | "s" | "sub" | "sup" | "code" | "td" | "th"
            )
        });
        match name.as_str() {
            "section" => {
                self.sections += 1;
                let mut attrs = self.attrs(node, None);
                if self.source.attr(node, "id").is_none() {
                    attrs.push(Attr {
                        name: Name::plain("id"),
                        value: format!("fb2-section-{}", self.sections),
                    });
                }
                let id = attrs
                    .iter()
                    .find(|a| a.name.local == "id")
                    .map(|a| a.value.clone());
                let section = Self::element(out, parent, "section", attrs);
                let label = child(self.source, node, "title")
                    .map(|t| text(self.source, t))
                    .unwrap_or_default();
                let mut entry = SourceNav {
                    label,
                    target: Some((path.to_owned(), id)),
                    ..SourceNav::default()
                };
                self.children(node, out, section, depth + 1, &mut entry.children, path);
                if !entry.label.is_empty() || !entry.children.is_empty() {
                    toc.push(entry);
                }
            }
            "title" => {
                let level = (depth + 1).clamp(1, 6);
                let heading = Self::element(
                    out,
                    parent,
                    &format!("h{level}"),
                    self.attrs(node, Some("title")),
                );
                let mut first = true;
                for line in self.source.element_children(node) {
                    if !first {
                        Self::element(out, heading, "br", Vec::new());
                    }
                    first = false;
                    if local(self.source, line) == "p" {
                        self.children(line, out, heading, depth, toc, path);
                    }
                }
            }
            "subtitle" => {
                let p = Self::element(out, parent, "p", self.attrs(node, Some("subtitle")));
                self.children(node, out, p, depth, toc, path);
            }
            "epigraph" | "cite" | "annotation" => {
                let quote = Self::element(out, parent, "blockquote", self.attrs(node, Some(&name)));
                self.children(node, out, quote, depth, toc, path);
            }
            "poem" | "stanza" => {
                let div = Self::element(out, parent, "div", self.attrs(node, Some(&name)));
                self.children(node, out, div, depth, toc, path);
            }
            "v" | "text-author" | "date" | "p" => {
                let class = if name == "p" {
                    None
                } else {
                    Some(name.as_str())
                };
                let p = Self::element(out, parent, "p", self.attrs(node, class));
                self.children(node, out, p, depth, toc, path);
            }
            "empty-line" => {
                Self::element(
                    out,
                    parent,
                    "p",
                    vec![Attr {
                        name: Name::plain("class"),
                        value: "empty-line".to_owned(),
                    }],
                );
            }
            "image" => self.image(node, out, parent, parent_is_inline),
            "table" | "tr" | "th" | "td" => {
                let mut attrs = self.attrs(node, None);
                for key in ["colspan", "rowspan", "align", "valign"] {
                    if let Some(value) = self.source.attr(node, key) {
                        attrs.push(Attr {
                            name: Name::plain(key),
                            value: value.to_owned(),
                        });
                    }
                }
                let element = Self::element(out, parent, &name, attrs);
                self.children(node, out, element, depth, toc, path);
            }
            "a" => {
                let mut attrs = self.attrs(
                    node,
                    (self.source.attr(node, "type") == Some("note")).then_some("note"),
                );
                if let Some(target) = href(self.source, node) {
                    attrs.push(Attr {
                        name: Name::plain("href"),
                        value: target.to_owned(),
                    });
                }
                let a = Self::element(out, parent, "a", attrs);
                self.children(node, out, a, depth, toc, path);
            }
            "strong" | "sub" | "sup" | "code" => {
                let element = Self::element(out, parent, &name, self.attrs(node, None));
                self.children(node, out, element, depth, toc, path);
            }
            "emphasis" => {
                let element = Self::element(out, parent, "em", self.attrs(node, None));
                self.children(node, out, element, depth, toc, path);
            }
            "strikethrough" => {
                let element = Self::element(out, parent, "s", self.attrs(node, None));
                self.children(node, out, element, depth, toc, path);
            }
            _ => {
                let tag = if INLINE.contains(&name.as_str()) || parent_is_inline {
                    "span"
                } else {
                    "div"
                };
                let element = Self::element(
                    out,
                    parent,
                    tag,
                    self.attrs(
                        node,
                        Some(&name).filter(|n| !n.is_empty()).map(|n| n.as_str()),
                    ),
                );
                self.children(node, out, element, depth, toc, path);
            }
        }
    }
}

/// The FB2 document inside a `.fb2.zip`, or the bytes themselves.
fn unpack(bytes: &[u8], limits: &Limits) -> Result<Vec<u8>> {
    if !bytes.starts_with(b"PK\x03\x04") {
        return Ok(bytes.to_vec());
    }
    let archive = Archive::open(bytes, *limits)?;
    let name = archive
        .names()
        .find(|name| name.to_ascii_lowercase().ends_with(".fb2"))
        .map(str::to_owned)
        .ok_or(NormalizeError::NotThisFormat("FB2"))?;
    archive.read(&name)
}

/// Parses a FictionBook's XML, declaring `l:` if it forgot to.
fn parse(mut markup: String, limits: &Limits) -> Result<(Dom, NodeId)> {
    // `l:` is XLink in every FictionBook, declared or not.
    if !markup.contains("xmlns:l=")
        && let Some(at) = markup.find("<FictionBook")
    {
        markup.insert_str(
            at + "<FictionBook".len(),
            " xmlns:l=\"http://www.w3.org/1999/xlink\"",
        );
    }
    let source = crate::xml::parse(
        &markup,
        &crate::xml::Limits {
            max_nodes: limits.max_nodes,
            ..crate::xml::Limits::default()
        },
    )
    .map_err(|error| NormalizeError::Malformed(format!("FB2: {error}")))?;
    let root = source
        .root_element()
        .filter(|&root| local(&source, root) == "FictionBook")
        .ok_or(NormalizeError::NotThisFormat("FB2"))?;
    Ok((source, root))
}

/// Only what `<description>` says, without reading the text or the pictures.
pub fn read_metadata(bytes: &[u8], limits: &Limits) -> Result<crate::package::Metadata> {
    let bytes = unpack(bytes, limits)?;
    let markup = crate::encoding::decode_xml(&bytes).text;
    let end = markup
        .find("</description>")
        .ok_or(NormalizeError::NotThisFormat("FB2"))?;
    let (source, root) = parse(
        format!("{}</description></FictionBook>", &markup[..end]),
        limits,
    )?;
    let mut book = SourceBook::new(SourceFormat::Fb2);
    describe(&source, root, &mut book);
    Ok(book.metadata)
}

pub fn read(bytes: &[u8], limits: &Limits) -> Result<SourceBook> {
    let bytes = unpack(bytes, limits)?;
    let (source, root) = parse(crate::encoding::decode_xml(&bytes).text, limits)?;
    let mut book = SourceBook::new(SourceFormat::Fb2);
    book.version = Some("2".to_owned());

    // Pictures, by id.
    let mut binaries = std::collections::HashSet::new();
    for binary in children(&source, root, "binary") {
        let (Some(id), content) = (source.attr(binary, "id"), source.text_content(binary)) else {
            continue;
        };
        let Some(data) = base64_decode(&content) else {
            book.warnings
                .push(format!("binary {id} is not valid base64"));
            continue;
        };
        binaries.insert(id.to_owned());
        book.resources.push(SourceResource {
            path: format!("text/{id}"),
            media_type: source.attr(binary, "content-type").unwrap_or("").to_owned(),
            data,
        });
    }

    describe(&source, root, &mut book);
    let title = book.metadata.title().unwrap_or("FictionBook").to_owned();
    let lang = book.metadata.languages.first().cloned();
    body(&source, root, book, &binaries, &title, lang)
}

/// The metadata in `<description>`.
fn describe(source: &Dom, root: NodeId, book: &mut SourceBook) {
    if let Some(description) = child(source, root, "description") {
        let m = &mut book.metadata;
        if let Some(info) = child(source, description, "title-info") {
            m.titles.extend(
                child(source, info, "book-title")
                    .and_then(|t| clean_field(&text(source, t), MAX_FIELD))
                    .map(|value| Title {
                        value,
                        ..Title::default()
                    }),
            );
            m.creators
                .extend(children(source, info, "author").filter_map(|a| person(source, a)));
            m.contributors.extend(
                children(source, info, "translator")
                    .filter_map(|a| person(source, a))
                    .map(|mut c| {
                        c.roles.push("trl".to_owned());
                        c
                    }),
            );
            m.subjects.extend(
                children(source, info, "genre").filter_map(|g| clean_field(&text(source, g), 200)),
            );
            if let Some(keywords) = child(source, info, "keywords") {
                m.subjects.extend(
                    text(source, keywords)
                        .split(',')
                        .filter_map(|k| clean_field(k, 200)),
                );
            }
            m.description = child(source, info, "annotation")
                .and_then(|a| clean_field(&source.text_content(a), 20_000));
            m.published = child(source, info, "date").and_then(|d| {
                source
                    .attr(d, "value")
                    .map(str::to_owned)
                    .or_else(|| clean_field(&text(source, d), 64))
            });
            m.languages.extend(
                child(source, info, "lang").and_then(|l| clean_field(&text(source, l), 64)),
            );
            if let Some(sequence) = child(source, info, "sequence")
                && let Some(name) = source
                    .attr(sequence, "name")
                    .and_then(|n| clean_field(n, MAX_FIELD))
            {
                m.series = Some(Series {
                    name,
                    position: source.attr(sequence, "number").map(str::to_owned),
                });
            }
            if let Some(image) =
                child(source, info, "coverpage").and_then(|c| child(source, c, "image"))
                && let Some(target) = href(source, image).and_then(|h| h.strip_prefix('#'))
            {
                book.cover = Some(format!("text/{}", url::percent_decode(target)));
            }
        }
        if let Some(info) = child(source, description, "publish-info") {
            m.publisher = child(source, info, "publisher")
                .and_then(|p| clean_field(&text(source, p), MAX_FIELD));
            if m.published.is_none() {
                m.published =
                    child(source, info, "year").and_then(|y| clean_field(&text(source, y), 16));
            }
            if let Some(isbn) =
                child(source, info, "isbn").and_then(|i| clean_field(&text(source, i), 64))
            {
                m.identifiers.push(Identifier {
                    value: isbn,
                    scheme: Some("ISBN".to_owned()),
                });
            }
        }
        if let Some(info) = child(source, description, "document-info")
            && let Some(id) =
                child(source, info, "id").and_then(|i| clean_field(&text(source, i), 512))
        {
            m.unique_identifier = Some(id.clone());
            m.identifiers.push(Identifier {
                value: id,
                scheme: Some("FB2".to_owned()),
            });
        }
    }
}

/// The documents, from the bodies.
fn body(
    source: &Dom,
    root: NodeId,
    mut book: SourceBook,
    binaries: &std::collections::HashSet<String>,
    title: &str,
    lang: Option<String>,
) -> Result<SourceBook> {
    let title = title.to_owned();

    // Documents: what comes before the first section, each top-level section, then each other body.
    let mut converter = Converter {
        source,
        binaries,
        sections: 0,
    };
    let bodies: Vec<NodeId> = children(source, root, "body").collect();
    let mut groups: Vec<(Vec<NodeId>, bool, usize)> = Vec::new();
    for (index, &body) in bodies.iter().enumerate() {
        let main = index == 0
            && source
                .attr(body, "name")
                .is_none_or(|name| name != "notes" && name != "comments");
        if !main {
            groups.push((vec![body], false, 0));
            continue;
        }
        let mut preamble = Vec::new();
        for kid in source.element_children(body) {
            if local(source, kid) == "section" {
                if !preamble.is_empty() {
                    groups.push((std::mem::take(&mut preamble), true, 1));
                }
                groups.push((vec![kid], true, 1));
            } else {
                preamble.push(kid);
            }
        }
        if !preamble.is_empty() {
            groups.push((preamble, true, 1));
        }
    }
    for (index, (nodes, linear, depth)) in groups.into_iter().enumerate() {
        let path = format!("text/{index:04}.xhtml");
        let label = nodes
            .first()
            .and_then(|&n| child(source, n, "title"))
            .map(|t| text(source, t))
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| title.clone());
        let (mut out, body) = empty_page(&label, lang.as_deref());
        let mut toc = Vec::new();
        for node in nodes {
            if local(source, node) == "body" {
                // A body of notes: its title is the document's heading, its sections the notes.
                let id = format!("fb2-body-{index}");
                let section = Converter::element(
                    &mut out,
                    body,
                    "section",
                    vec![
                        Attr {
                            name: Name::plain("id"),
                            value: id.clone(),
                        },
                        Attr {
                            name: Name::plain("class"),
                            value: "notes".to_owned(),
                        },
                    ],
                );
                converter.children(node, &mut out, section, 0, &mut Vec::new(), &path);
                toc.push(SourceNav {
                    label: label.clone(),
                    target: Some((path.clone(), Some(id))),
                    ..SourceNav::default()
                });
            } else {
                // Sections start at depth 0, so their titles are h2 and the book's own title h1.
                let _ = depth;
                converter.convert(node, &mut out, body, 0, &mut toc, &path);
            }
        }
        out.normalize_text(Dom::DOCUMENT);
        book.toc.extend(toc);
        book.documents.push(SourceDocument {
            path,
            id: None,
            linear,
            properties: Vec::new(),
            dom: out,
            parsed: Parsed::Generated,
            svg: false,
        });
    }
    if book.documents.is_empty() {
        return Err(NormalizeError::Malformed("FB2 without a body".to_owned()));
    }
    retarget_fragment_links(&mut book.documents);
    Ok(book)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOOK: &str = r##"<?xml version="1.0" encoding="utf-8"?>
<FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2.0" xmlns:l="http://www.w3.org/1999/xlink">
<description><title-info><genre>sf</genre><author><first-name>Ana</first-name><last-name>Lima</last-name></author>
<book-title>Livro de Teste</book-title><annotation><p>Sobre o livro.</p></annotation><lang>pt</lang>
<coverpage><image l:href="#cover.png"/></coverpage><sequence name="Série" number="2"/></title-info>
<publish-info><publisher>Editora</publisher><year>2024</year><isbn>9780000000002</isbn></publish-info></description>
<body><title><p>Livro de Teste</p></title>
<section><title><p>Capítulo 1</p></title><epigraph><p>Uma epígrafe.</p><text-author>Alguém</text-author></epigraph>
<p>Texto com <emphasis>ênfase</emphasis> e nota<a l:href="#n1" type="note">1</a>.</p><empty-line/>
<poem><stanza><v>Verso um</v><v>Verso dois</v></stanza></poem>
<section><title><p>Parte A</p></title><p>Dentro.</p><image l:href="#cover.png"/></section></section>
<section><title><p>Capítulo 2</p></title><p>Fim.</p></section></body>
<body name="notes"><title><p>Notas</p></title><section id="n1"><title><p>1</p></title><p>A nota.</p></section></body>
<binary id="cover.png" content-type="image/png">iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==</binary>
</FictionBook>"##;

    #[test]
    fn sections_become_documents_and_notes_are_linked() {
        let book = read(BOOK.as_bytes(), &Limits::default()).unwrap();
        assert_eq!(book.metadata.title(), Some("Livro de Teste"));
        assert_eq!(book.metadata.creators[0].name, "Ana Lima");
        assert_eq!(
            book.metadata.creators[0].file_as.as_deref(),
            Some("Lima, Ana")
        );
        assert_eq!(
            book.metadata.series.as_ref().unwrap().position.as_deref(),
            Some("2")
        );
        assert_eq!(book.cover.as_deref(), Some("text/cover.png"));
        assert_eq!(book.documents.len(), 4, "preamble, two chapters, notes");
        assert!(!book.documents[3].linear);
        assert_eq!(book.resources.len(), 1);
        assert_eq!(&book.resources[0].data[..4], b"\x89PNG");
        let chapter = &book.documents[1].dom;
        let link = chapter
            .descendants(Dom::DOCUMENT)
            .find(|&n| chapter.is_html(n, "a"))
            .unwrap();
        assert_eq!(chapter.attr(link, "href"), Some("0003.xhtml#n1"));
        let headings: Vec<String> = chapter
            .descendants(Dom::DOCUMENT)
            .filter(|&n| {
                chapter
                    .name(n)
                    .is_some_and(|name| name.local.starts_with('h') && name.local.len() == 2)
            })
            .map(|n| {
                format!(
                    "{}:{}",
                    chapter.name(n).unwrap().local,
                    chapter.text_content(n)
                )
            })
            .collect();
        assert_eq!(headings, ["h2:Capítulo 1", "h3:Parte A"]);
        let labels: Vec<&str> = book.toc.iter().map(|e| e.label.as_str()).collect();
        assert_eq!(labels, ["Capítulo 1", "Capítulo 2", "Notas"]);
        assert_eq!(book.toc[0].children[0].label, "Parte A");
    }
}

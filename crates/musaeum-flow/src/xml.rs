//! Well-formed XML into a [`Dom`], strictly: anything a browser's XML parser would refuse is an
//! error here too, so that the caller can fall back to the HTML parser knowingly.
//!
//! Two leniencies, both about names a browser would also know: HTML's named entities resolve in any
//! document, not only in those with an XHTML doctype, and a handful of well-known prefixes (`epub:`,
//! `xlink:`) work without being declared. Nothing is ever fetched: external entities and DTDs are
//! ignored, and internal entities are plain text with a cap on how much they expand.

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use thiserror::Error;

use crate::dom::{Attr, Data, Dom, Name, NodeId, Ns};

/// Limits on what one document may build.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_depth: usize,
    pub max_nodes: usize,
    /// Characters that entities declared in the document may expand to, altogether.
    pub max_entity_chars: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_depth: 1024,
            max_nodes: 2_000_000,
            max_entity_chars: 1 << 20,
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum XmlError {
    #[error("not well-formed: {0}")]
    Syntax(String),
    #[error("the prefix {0} is not declared")]
    Prefix(String),
    #[error("&{0}; is not a known entity")]
    Entity(String),
    #[error("nested more than {0} deep")]
    TooDeep(usize),
    #[error("more than {0} nodes")]
    TooLarge(usize),
    #[error("there is no root element")]
    NoRoot,
}

/// Replaces `\r\n` and lone `\r` with `\n`, which XML does before anything else.
fn normalize_newlines(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains('\r') {
        return std::borrow::Cow::Borrowed(text);
    }
    std::borrow::Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n"))
}

/// What an entity reference stands for: `name` is what is between `&` and `;`.
fn resolve_reference(
    name: &str,
    declared: &[(String, String)],
    budget: &mut usize,
) -> Result<String, XmlError> {
    if let Some(number) = name.strip_prefix('#') {
        let value = match number.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16),
            None => number.parse::<u32>(),
        }
        .map_err(|_| XmlError::Entity(name.to_owned()))?;
        return char::from_u32(value)
            .filter(|&c| is_xml_char(c))
            .map(|c| c.to_string())
            .ok_or_else(|| XmlError::Entity(name.to_owned()));
    }
    let predefined = match name {
        "amp" => Some("&"),
        "lt" => Some("<"),
        "gt" => Some(">"),
        "quot" => Some("\""),
        "apos" => Some("'"),
        _ => None,
    };
    if let Some(text) = predefined {
        return Ok(text.to_owned());
    }
    if let Some((_, value)) = declared.iter().find(|(entity, _)| entity == name) {
        if value.len() > *budget {
            return Err(XmlError::TooLarge(0));
        }
        *budget -= value.len();
        return Ok(value.clone());
    }
    crate::entities::named(name).ok_or_else(|| XmlError::Entity(name.to_owned()))
}

/// Whether XML 1.0 allows this character in a document at all.
pub fn is_xml_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..)
}

/// An attribute's value, as XML normalizes it: literal white space becomes a space, references
/// resolve, and a reference to white space stays what it is.
fn attribute_value(
    raw: &str,
    declared: &[(String, String)],
    budget: &mut usize,
) -> Result<String, XmlError> {
    let mut value = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(at) = rest.find(['&', '\t', '\n', '<']) {
        value.push_str(&rest[..at]);
        let c = rest.as_bytes()[at];
        match c {
            b'&' => {
                let end = rest[at..]
                    .find(';')
                    .filter(|&end| end <= 64)
                    .ok_or_else(|| XmlError::Syntax("an & that starts no reference".to_owned()))?;
                value.push_str(&resolve_reference(
                    &rest[at + 1..at + end],
                    declared,
                    budget,
                )?);
                rest = &rest[at + end + 1..];
            }
            b'<' => return Err(XmlError::Syntax("< inside an attribute".to_owned())),
            _ => {
                value.push(' ');
                rest = &rest[at + 1..];
            }
        }
    }
    value.push_str(rest);
    Ok(value)
}

/// The simple entities an internal DTD subset declares: `<!ENTITY name "text">`. Parameter and
/// external entities are skipped, and so is any whose text has markup or other entities in it.
fn declared_entities(doctype: &str) -> Vec<(String, String)> {
    let Some(open) = doctype.find('[') else {
        return Vec::new();
    };
    let mut entities = Vec::new();
    let mut rest = &doctype[open + 1..];
    while let Some(at) = rest.find("<!ENTITY") {
        rest = &rest[at + "<!ENTITY".len()..];
        let declaration = rest.trim_start();
        if declaration.starts_with('%') {
            continue;
        }
        let name_end = declaration
            .find(|c: char| c.is_whitespace())
            .unwrap_or(declaration.len());
        let name = &declaration[..name_end];
        let after = declaration[name_end..].trim_start();
        let Some(quote) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else {
            continue;
        };
        let Some(end) = after[1..].find(quote) else {
            break;
        };
        let text = &after[1..1 + end];
        if name.is_empty() || entities.len() >= 256 || text.len() > 16 * 1024 {
            continue;
        }
        if text.contains('<') || text.contains('%') {
            continue;
        }
        // Character references are the only ones resolved in an entity's text here.
        if let Ok(value) = resolve_char_refs(text) {
            entities.push((name.to_owned(), value));
        }
    }
    entities
}

fn resolve_char_refs(text: &str) -> Result<String, XmlError> {
    let mut value = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        value.push_str(&rest[..at]);
        let end = rest[at..]
            .find(';')
            .ok_or_else(|| XmlError::Syntax("a dangling &".to_owned()))?;
        let name = &rest[at + 1..at + end];
        if !name.starts_with('#') {
            return Err(XmlError::Entity(name.to_owned()));
        }
        let mut budget = 0;
        value.push_str(&resolve_reference(name, &[], &mut budget)?);
        rest = &rest[at + end + 1..];
    }
    value.push_str(rest);
    Ok(value)
}

fn split_qname(name: &str) -> (Option<&str>, &str) {
    match name.split_once(':') {
        Some((prefix, local)) => (Some(prefix), local),
        None => (None, name),
    }
}

struct Builder<'a> {
    dom: Dom,
    limits: &'a Limits,
    /// In-scope declarations, innermost last: a prefix, or `None` for the default namespace.
    scope: Vec<(Option<String>, Ns)>,
    /// Open elements, and the length of `scope` before each one.
    open: Vec<(NodeId, usize)>,
    entities: Vec<(String, String)>,
    budget: usize,
}

impl Builder<'_> {
    fn lookup(&self, prefix: Option<&str>) -> Result<Ns, XmlError> {
        if prefix == Some("xml") {
            return Ok(Ns::Xml);
        }
        if let Some((_, ns)) = self
            .scope
            .iter()
            .rev()
            .find(|(declared, _)| declared.as_deref() == prefix)
        {
            return Ok(ns.clone());
        }
        match prefix {
            None => Ok(Ns::None),
            Some(prefix) => {
                Ns::of_known_prefix(prefix).ok_or_else(|| XmlError::Prefix(prefix.to_owned()))
            }
        }
    }

    fn parent(&self) -> NodeId {
        self.open.last().map_or(Dom::DOCUMENT, |(node, _)| *node)
    }

    fn open_element(&mut self, start: &BytesStart<'_>) -> Result<NodeId, XmlError> {
        if self.open.len() >= self.limits.max_depth {
            return Err(XmlError::TooDeep(self.limits.max_depth));
        }
        if self.dom.len() >= self.limits.max_nodes {
            return Err(XmlError::TooLarge(self.limits.max_nodes));
        }
        let mark = self.scope.len();
        let mut plain = Vec::new();
        let mut attributes = start.attributes();
        attributes.with_checks(false);
        for attribute in attributes {
            let attribute = attribute.map_err(|error| XmlError::Syntax(error.to_string()))?;
            let key: &str = attribute.key.as_ref();
            let value = attribute_value(&attribute.value, &self.entities, &mut self.budget)?;
            if key == "xmlns" {
                self.scope.push((None, Ns::from_uri(&value)));
            } else if let Some(prefix) = key.strip_prefix("xmlns:") {
                self.scope
                    .push((Some(prefix.to_owned()), Ns::from_uri(&value)));
            } else {
                plain.push((key.to_owned(), value));
            }
        }
        let qname = start.name();
        let raw_name: &str = qname.as_ref();
        let (prefix, local) = split_qname(raw_name);
        if local.is_empty() {
            return Err(XmlError::Syntax(format!("<{raw_name}> has no name")));
        }
        let ns = self.lookup(prefix)?;
        let mut attrs: Vec<Attr> = Vec::with_capacity(plain.len());
        for (key, value) in plain {
            let (attr_prefix, attr_local) = split_qname(&key);
            if attr_local.is_empty() {
                return Err(XmlError::Syntax(format!("an attribute named {key}")));
            }
            let attr_ns = match attr_prefix {
                None => Ns::None,
                Some(attr_prefix) => self.lookup(Some(attr_prefix))?,
            };
            // A repeated attribute is an error in XML; the first one is what HTML would keep.
            if attrs
                .iter()
                .any(|attr| attr.name.ns == attr_ns && attr.name.local == attr_local)
            {
                return Err(XmlError::Syntax(format!("{key} is repeated")));
            }
            attrs.push(Attr {
                name: Name {
                    ns: attr_ns,
                    local: attr_local.to_owned(),
                    prefix: attr_prefix.map(str::to_owned),
                },
                value,
            });
        }
        let node = self.dom.create_element(
            Name {
                ns,
                local: local.to_owned(),
                prefix: prefix.map(str::to_owned),
            },
            attrs,
        );
        let parent = self.parent();
        if parent == Dom::DOCUMENT && self.dom.root_element().is_some() {
            return Err(XmlError::Syntax("a second root element".to_owned()));
        }
        self.dom.append(parent, node);
        self.open.push((node, mark));
        Ok(node)
    }

    fn close_element(&mut self) -> Result<(), XmlError> {
        let (_, mark) = self
            .open
            .pop()
            .ok_or_else(|| XmlError::Syntax("an end tag that ends nothing".to_owned()))?;
        self.scope.truncate(mark);
        Ok(())
    }

    fn text(&mut self, text: &str) -> Result<(), XmlError> {
        if text.is_empty() {
            return Ok(());
        }
        if let Some(bad) = text.chars().find(|&c| !is_xml_char(c)) {
            return Err(XmlError::Syntax(format!(
                "the character U+{:04X}",
                bad as u32
            )));
        }
        let parent = self.parent();
        if parent == Dom::DOCUMENT {
            if text.trim_matches(['\t', '\n', ' ']).is_empty() {
                return Ok(());
            }
            return Err(XmlError::Syntax("text outside the root element".to_owned()));
        }
        self.dom.append_text(parent, text);
        Ok(())
    }
}

/// Parses an XML document that is already text.
pub fn parse(text: &str, limits: &Limits) -> Result<Dom, XmlError> {
    let text = normalize_newlines(text);
    let mut reader = Reader::from_str(&text);
    let config = reader.config_mut();
    config.check_end_names = true;
    config.expand_empty_elements = false;
    config.check_comments = false;
    let mut builder = Builder {
        dom: Dom::new(),
        limits,
        scope: Vec::new(),
        open: Vec::new(),
        entities: Vec::new(),
        budget: limits.max_entity_chars,
    };
    loop {
        let event = reader
            .read_event()
            .map_err(|error| XmlError::Syntax(error.to_string()))?;
        match event {
            Event::Start(start) => {
                builder.open_element(&start)?;
            }
            Event::Empty(start) => {
                builder.open_element(&start)?;
                builder.close_element()?;
            }
            Event::End(_) => builder.close_element()?,
            Event::Text(text) => builder.text(&text.into_inner())?,
            Event::CData(data) => {
                if builder.parent() == Dom::DOCUMENT {
                    return Err(XmlError::Syntax(
                        "CDATA outside the root element".to_owned(),
                    ));
                }
                builder.text(&data.into_inner())?;
            }
            Event::GeneralRef(reference) => {
                let name = reference.into_inner();
                let value = resolve_reference(&name, &builder.entities, &mut builder.budget)?;
                if builder.parent() == Dom::DOCUMENT {
                    return Err(XmlError::Syntax(
                        "a reference outside the root element".to_owned(),
                    ));
                }
                builder.text(&value)?;
            }
            Event::DocType(doctype) => {
                builder.entities = declared_entities(&doctype.into_inner());
            }
            Event::Comment(_) | Event::PI(_) | Event::Decl(_) => {}
            Event::Eof => break,
        }
    }
    if !builder.open.is_empty() {
        return Err(XmlError::Syntax("an element is never closed".to_owned()));
    }
    let mut dom = builder.dom;
    let root = dom.root_element().ok_or(XmlError::NoRoot)?;
    // Old XHTML without its namespace: a browser would show it as unknown XML, and every reading
    // system that shows it at all treats it as XHTML.
    if dom.is(root, &Ns::None, "html") {
        let nodes: Vec<NodeId> = dom.descendants(root).collect();
        for node in nodes {
            if let Some(name) = dom.name_mut(node)
                && name.ns == Ns::None
            {
                name.ns = Ns::Html;
                name.prefix = None;
            }
        }
    }
    dom.normalize_text(Dom::DOCUMENT);
    Ok(dom)
}

/// The whole text of a node that holds text, whatever the source wrapped it in.
pub fn text_of(dom: &Dom, node: NodeId) -> String {
    match dom.data(node) {
        Data::Text(text) => text.clone(),
        _ => dom.text_content(node),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(text: &str) -> Dom {
        parse(text, &Limits::default()).unwrap()
    }

    #[test]
    fn namespaces_resolve_and_text_merges() {
        let dom = parse_ok(
            "<?xml version='1.0'?><!DOCTYPE html><html xmlns='http://www.w3.org/1999/xhtml' \
             xmlns:epub='http://www.idpf.org/2007/ops'><body><p epub:type='x'>a&amp;b<![CDATA[<c>]]>&#x44;&nbsp;\
             <!-- gone -->e</p><svg xmlns='http://www.w3.org/2000/svg'><a xlink:href='#x' \
             xmlns:xlink='http://www.w3.org/1999/xlink'/></svg></body></html>",
        );
        let body = dom.body().unwrap();
        let p = dom.element_children(body).next().unwrap();
        assert!(dom.is_html(p, "p"));
        assert_eq!(dom.attr_ns(p, &Ns::Ops, "type"), Some("x"));
        assert_eq!(dom.children(p).len(), 1, "one run of text");
        assert_eq!(dom.text_content(p), "a&b<c>D\u{a0}e");
        let svg = dom.element_children(body).nth(1).unwrap();
        assert!(dom.is(svg, &Ns::Svg, "svg"));
        let a = dom.element_children(svg).next().unwrap();
        assert_eq!(dom.attr_ns(a, &Ns::Xlink, "href"), Some("#x"));
    }

    #[test]
    fn line_ends_and_attribute_white_space_follow_xml() {
        let dom = parse_ok("<a b='x\ty&#10;z'>1\r\n2\r3</a>");
        let root = dom.root_element().unwrap();
        assert_eq!(dom.attr(root, "b"), Some("x y\nz"));
        assert_eq!(dom.text_content(root), "1\n2\n3");
    }

    #[test]
    fn what_a_browser_refuses_is_refused() {
        for bad in [
            "<p>unclosed",
            "<p></q>",
            "<p a='1' a='2'/>",
            "<p>&bogus;</p>",
            "<p>a & b</p>",
            "<x:p/>",
            "<a/><b/>",
            "",
            "<p>\u{1}</p>",
        ] {
            assert!(parse(bad, &Limits::default()).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn xhtml_without_its_namespace_becomes_xhtml() {
        let dom = parse_ok("<html><body><p>x</p></body></html>");
        assert!(dom.body().is_some());
    }

    #[test]
    fn declared_entities_expand_and_external_ones_do_not() {
        let dom = parse_ok(
            "<!DOCTYPE a [<!ENTITY me \"Musaeum &#169;\"><!ENTITY ext SYSTEM \"file:///c:/x\">]><a>&me;</a>",
        );
        assert_eq!(dom.text_content(dom.root_element().unwrap()), "Musaeum ©");
        assert!(
            parse(
                "<!DOCTYPE a [<!ENTITY ext SYSTEM \"file:///c:/x\">]><a>&ext;</a>",
                &Limits::default()
            )
            .is_err()
        );
    }

    #[test]
    fn depth_is_limited() {
        let deep = "<a>".repeat(2000) + &"</a>".repeat(2000);
        assert_eq!(
            parse(&deep, &Limits::default()).unwrap_err(),
            XmlError::TooDeep(1024)
        );
    }
}

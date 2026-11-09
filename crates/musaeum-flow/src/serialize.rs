//! A [`Dom`] as XML text that a browser's XML parser turns back into exactly the same tree.
//!
//! That round trip is what lets a CFI computed here mean the same node in the reader: every element
//! comes back with its namespace, every run of text with its characters. So the escaping covers what
//! XML would otherwise change on the way in (carriage returns in text, white space in attributes),
//! namespaces are declared wherever they are needed, and characters XML does not allow are left out.

use std::collections::BTreeMap;

use crate::dom::{Data, Dom, Name, NodeId, Ns};
use crate::xml::is_xml_char;

/// HTML elements that never have content, written as `<br/>`.
const VOID: [&str; 14] = [
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

pub const XML_DECLARATION: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n";

/// Whether `name` is an XML name without a colon.
pub fn is_ncname(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    let start = |c: char| {
        c.is_ascii_alphabetic()
            || c == '_'
            || matches!(c, '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}'
                | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}' | '\u{200C}'..='\u{200D}'
                | '\u{2070}'..='\u{218F}' | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}'
                | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}' | '\u{10000}'..='\u{EFFFF}')
    };
    start(first) && chars.all(|c| {
        start(c)
            || c.is_ascii_digit()
            || matches!(c, '-' | '.' | '\u{B7}' | '\u{300}'..='\u{36F}' | '\u{203F}'..='\u{2040}')
    })
}

fn escape_text(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            // A literal carriage return would come back as a line feed.
            '\r' => out.push_str("&#13;"),
            c if is_xml_char(c) => out.push(c),
            _ => {}
        }
    }
}

fn escape_attr(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            // Literal white space in an attribute would come back as a space.
            '\t' => out.push_str("&#9;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            c if is_xml_char(c) => out.push(c),
            _ => {}
        }
    }
}

/// Namespace bindings in force: a prefix, or `None` for the default namespace.
#[derive(Clone, Default)]
struct Scope {
    bindings: Vec<(Option<String>, Ns)>,
}

impl Scope {
    fn default_ns(&self) -> Ns {
        self.bindings
            .iter()
            .rev()
            .find(|(prefix, _)| prefix.is_none())
            .map_or(Ns::None, |(_, ns)| ns.clone())
    }

    fn prefix_for(&self, ns: &Ns) -> Option<&str> {
        // The innermost binding of a prefix wins, so a prefix only counts if nothing shadows it.
        for (index, (prefix, bound)) in self.bindings.iter().enumerate().rev() {
            let Some(prefix) = prefix else { continue };
            if bound == ns
                && !self.bindings[index + 1..]
                    .iter()
                    .any(|(other, _)| other.as_deref() == Some(prefix))
            {
                return Some(prefix);
            }
        }
        None
    }

    fn is_bound(&self, prefix: &str) -> bool {
        self.bindings
            .iter()
            .any(|(bound, _)| bound.as_deref() == Some(prefix))
    }
}

/// The prefixed namespaces a document's attributes and elements use, with the prefix each gets,
/// declared once on the root element.
fn root_prefixes(dom: &Dom, root: NodeId) -> BTreeMap<String, Ns> {
    let mut by_ns: BTreeMap<Ns, String> = BTreeMap::new();
    let mut taken: BTreeMap<String, Ns> = BTreeMap::new();
    let mut want = |ns: &Ns, hint: Option<&str>| {
        if matches!(ns, Ns::None | Ns::Xml | Ns::Xmlns) || by_ns.contains_key(ns) {
            return;
        }
        let candidates = [hint, ns.usual_prefix()];
        let chosen = candidates
            .into_iter()
            .flatten()
            .find(|prefix| {
                is_ncname(prefix)
                    && *prefix != "xml"
                    && !prefix.starts_with("xmlns")
                    && !taken.contains_key(*prefix)
            })
            .map(str::to_owned)
            .unwrap_or_else(|| {
                (1..)
                    .map(|n| format!("ns{n}"))
                    .find(|prefix| !taken.contains_key(prefix))
                    .expect("an unused prefix")
            });
        taken.insert(chosen.clone(), ns.clone());
        by_ns.insert(ns.clone(), chosen);
    };
    for node in dom.descendants(root) {
        if let Data::Element { attrs, .. } = dom.data(node) {
            for attr in attrs {
                want(&attr.name.ns, attr.name.prefix.as_deref());
            }
        }
    }
    taken
}

struct Writer<'a> {
    dom: &'a Dom,
    out: String,
    sizes: Option<Vec<usize>>,
    /// The qualified names chosen for start tags that have a prefix, so their end tags match.
    prefixed_names: std::collections::HashMap<NodeId, String>,
}

impl Writer<'_> {
    fn element_name(name: &Name) -> &str {
        if is_ncname(&name.local) {
            &name.local
        } else {
            "span"
        }
    }

    fn write(&mut self, root: NodeId) {
        enum Step {
            Open(NodeId, Scope),
            Close(NodeId, usize),
        }
        let mut stack = vec![Step::Open(root, Scope::default())];
        while let Some(step) = stack.pop() {
            match step {
                Step::Close(node, start) => {
                    if let Some(name) = self.dom.name(node) {
                        let empty = self.dom.children(node).is_empty();
                        let void = name.ns == Ns::Html && VOID.contains(&name.local.as_str());
                        if !(empty && (void || name.ns != Ns::Html)) {
                            let qualified = self.qualified_end(node);
                            self.out.push_str("</");
                            self.out.push_str(&qualified);
                            self.out.push('>');
                        }
                    }
                    if let Some(sizes) = &mut self.sizes {
                        sizes[node] = self.out.len() - start;
                    }
                }
                Step::Open(node, scope) => {
                    let start = self.out.len();
                    match self.dom.data(node) {
                        Data::Document => {
                            stack.push(Step::Close(node, start));
                            for &child in self.dom.children(node).iter().rev() {
                                stack.push(Step::Open(child, scope.clone()));
                            }
                        }
                        Data::Text(text) => {
                            escape_text(text, &mut self.out);
                            if let Some(sizes) = &mut self.sizes {
                                sizes[node] = self.out.len() - start;
                            }
                        }
                        Data::Element { .. } => {
                            let inner = self.open_tag(node, &scope);
                            stack.push(Step::Close(node, start));
                            for &child in self.dom.children(node).iter().rev() {
                                stack.push(Step::Open(child, inner.clone()));
                            }
                        }
                    }
                }
            }
        }
    }

    /// The end tag's name, which is the start tag's. Kept per element during the walk.
    fn qualified_end(&self, node: NodeId) -> String {
        self.prefixed_names.get(&node).cloned().unwrap_or_else(|| {
            Self::element_name(self.dom.name(node).expect("an element")).to_owned()
        })
    }
}

impl Writer<'_> {
    fn open_tag(&mut self, node: NodeId, scope: &Scope) -> Scope {
        let Data::Element { name, attrs } = self.dom.data(node) else {
            unreachable!()
        };
        let mut inner = scope.clone();
        let mut declarations: Vec<(Option<String>, String)> = Vec::new();
        let is_root = self.dom.parent(node) == Some(Dom::DOCUMENT);

        // The element's own namespace: the default, or the prefix the source gave if that is bound.
        let local = Self::element_name(name);
        let mut qualified = local.to_owned();
        if name.ns != inner.default_ns() {
            let bound_prefix = name
                .prefix
                .as_deref()
                .filter(|prefix| inner.prefix_for(&name.ns) == Some(*prefix));
            match bound_prefix {
                Some(prefix) => qualified = format!("{prefix}:{local}"),
                None => {
                    inner.bindings.push((None, name.ns.clone()));
                    declarations.push((None, name.ns.uri().to_owned()));
                }
            }
        }
        if is_root {
            for (prefix, ns) in root_prefixes(self.dom, node) {
                inner.bindings.push((Some(prefix.clone()), ns.clone()));
                declarations.push((Some(prefix), ns.uri().to_owned()));
            }
        }

        let mut written_attrs = String::new();
        for attr in attrs {
            if !is_ncname(&attr.name.local) {
                continue;
            }
            let key = match &attr.name.ns {
                Ns::None => attr.name.local.clone(),
                Ns::Xml => format!("xml:{}", attr.name.local),
                Ns::Xmlns => continue,
                ns => {
                    let prefix = match inner.prefix_for(ns) {
                        Some(prefix) => prefix.to_owned(),
                        None => {
                            let prefix = [attr.name.prefix.as_deref(), ns.usual_prefix()]
                                .into_iter()
                                .flatten()
                                .find(|prefix| {
                                    is_ncname(prefix) && *prefix != "xml" && !inner.is_bound(prefix)
                                })
                                .map(str::to_owned)
                                .unwrap_or_else(|| {
                                    (1..)
                                        .map(|n| format!("ns{n}"))
                                        .find(|prefix| !inner.is_bound(prefix))
                                        .expect("an unused prefix")
                                });
                            inner.bindings.push((Some(prefix.clone()), ns.clone()));
                            declarations.push((Some(prefix.clone()), ns.uri().to_owned()));
                            prefix
                        }
                    };
                    format!("{prefix}:{}", attr.name.local)
                }
            };
            written_attrs.push(' ');
            written_attrs.push_str(&key);
            written_attrs.push_str("=\"");
            escape_attr(&attr.value, &mut written_attrs);
            written_attrs.push('"');
        }

        self.out.push('<');
        self.out.push_str(&qualified);
        for (prefix, uri) in &declarations {
            match prefix {
                None => self.out.push_str(" xmlns=\""),
                Some(prefix) => {
                    self.out.push_str(" xmlns:");
                    self.out.push_str(prefix);
                    self.out.push_str("=\"");
                }
            }
            escape_attr(uri, &mut self.out);
            self.out.push('"');
        }
        self.out.push_str(&written_attrs);
        let empty = self.dom.children(node).is_empty();
        let void = name.ns == Ns::Html && VOID.contains(&name.local.as_str());
        if empty && (void || name.ns != Ns::Html) {
            self.out.push_str("/>");
        } else {
            self.out.push('>');
        }
        if qualified != local {
            self.prefixed_names.insert(node, qualified);
        }
        inner
    }
}

/// Writes the document `dom` as XML, after `prolog`.
pub fn to_xml(dom: &Dom, prolog: &str) -> String {
    let mut writer = Writer::new(dom, false);
    writer.out.push_str(prolog);
    writer.write(Dom::DOCUMENT);
    writer.out.push('\n');
    writer.out
}

/// Writes an XHTML document, with an XML declaration and the HTML doctype.
pub fn to_xhtml(dom: &Dom) -> String {
    to_xml(dom, &format!("{XML_DECLARATION}<!DOCTYPE html>\n"))
}

/// How many bytes each node takes in [`to_xhtml`]'s output, by node: what the splitter weighs.
/// Detached nodes count 0.
pub fn sizes(dom: &Dom) -> Vec<usize> {
    let mut writer = Writer::new(dom, true);
    writer.write(Dom::DOCUMENT);
    writer.sizes.unwrap_or_default()
}

/// The length [`to_xhtml`] would give, without keeping the text.
pub fn xhtml_len(dom: &Dom) -> usize {
    to_xhtml(dom).len()
}

impl<'a> Writer<'a> {
    fn new(dom: &'a Dom, with_sizes: bool) -> Self {
        Self {
            dom,
            out: String::new(),
            sizes: with_sizes.then(|| vec![0; dom.len()]),
            prefixed_names: std::collections::HashMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xml::{Limits, parse};

    fn round_trip(text: &str) -> (String, Dom, Dom) {
        let first = parse(text, &Limits::default()).unwrap();
        let written = to_xhtml(&first);
        let second = parse(&written, &Limits::default()).unwrap();
        (written, first, second)
    }

    fn same_tree(a: &Dom, b: &Dom) -> bool {
        let left: Vec<_> = a.descendants(Dom::DOCUMENT).collect();
        let right: Vec<_> = b.descendants(Dom::DOCUMENT).collect();
        left.len() == right.len()
            && left.iter().zip(&right).all(|(&x, &y)| {
                a.data(x) == b.data(y)
                    || matches!((a.data(x), b.data(y)), (Data::Element { name: m, attrs: p }, Data::Element { name: n, attrs: q })
                        if m.ns == n.ns && m.local == n.local && p.len() == q.len()
                            && p.iter().zip(q).all(|(s, t)| s.name.ns == t.name.ns && s.name.local == t.name.local && s.value == t.value))
            })
    }

    #[test]
    fn what_is_written_parses_back_to_the_same_tree() {
        let (written, first, second) = round_trip(
            "<html xmlns='http://www.w3.org/1999/xhtml' xmlns:epub='http://www.idpf.org/2007/ops'>\
             <head><title>t</title></head><body><p epub:type='a&amp;b' title='x&#10;y&#9;z'>1 &lt; 2 &amp;&#13;3</p>\
             <svg xmlns='http://www.w3.org/2000/svg' xmlns:xl='http://www.w3.org/1999/xlink'><use xl:href='#a'/>\
             <foreignObject><div xmlns='http://www.w3.org/1999/xhtml'>in</div></foreignObject></svg>\
             <m:math xmlns:m='http://www.w3.org/1998/Math/MathML'><m:mi>x</m:mi></m:math><br/><div/></body></html>",
        );
        assert!(same_tree(&first, &second), "{written}");
        assert!(written.contains("<br/>"));
        assert!(written.contains("<div></div>"));
        assert!(written.contains("x&#10;y&#9;z"));
        assert!(written.contains("&amp;&#13;3"));
    }

    #[test]
    fn element_sizes_add_up_to_the_document() {
        let dom = parse(
            "<html xmlns='http://www.w3.org/1999/xhtml'><body><p>a<b>é</b></p><p/></body></html>",
            &Limits::default(),
        )
        .unwrap();
        let sizes = sizes(&dom);
        let text = to_xhtml(&dom);
        let body = dom.body().unwrap();
        let p = dom.element_children(body).next().unwrap();
        assert_eq!(sizes[p], "<p>a<b>é</b></p>".len());
        assert_eq!(
            sizes[Dom::DOCUMENT] + format!("{XML_DECLARATION}<!DOCTYPE html>\n").len() + 1,
            text.len()
        );
    }

    #[test]
    fn names_xml_would_refuse_are_never_written() {
        let mut dom = Dom::new();
        let root = dom.create_element(Name::html("html"), Vec::new());
        dom.append(Dom::DOCUMENT, root);
        let bad = dom.create_element(
            Name::html("o:p"),
            vec![crate::dom::Attr {
                name: Name::plain("1bad"),
                value: "x".to_owned(),
            }],
        );
        dom.append(root, bad);
        dom.append_text(bad, "\u{1}ok\u{FFFF}");
        let written = to_xhtml(&dom);
        assert!(written.contains("<span>ok</span>"), "{written}");
        assert!(parse(&written, &Limits::default()).is_ok());
    }
}

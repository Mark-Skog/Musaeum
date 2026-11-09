//! HTML into a [`Dom`], with the parsing algorithm browsers use: for HTML documents, for MOBI's
//! markup, for what Markdown leaves, and for XHTML that is not well-formed XML.
//!
//! Scripting counts as disabled while parsing, so `<noscript>` content becomes markup: no script ever
//! runs in a book here, and that content is what a reader without scripts should see.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

use html5ever::interface::{ElemName, ElementFlags, NodeOrText, QuirksMode, TreeSink};
use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::tree_builder::TreeBuilderOpts;
use html5ever::{Attribute, LocalName, Namespace, ParseOpts, QualName, parse_document};

use crate::dom::{Attr, Dom, Name, NodeId, Ns};

/// An element's name, owned, so the tree can change while html5ever holds it.
pub struct OwnedName(QualName);

impl std::fmt::Debug for OwnedName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.0)
    }
}

impl ElemName for OwnedName {
    fn ns(&self) -> &Namespace {
        &self.0.ns
    }

    fn local_name(&self) -> &LocalName {
        &self.0.local
    }
}

struct Sink {
    dom: RefCell<Dom>,
    names: RefCell<HashMap<NodeId, QualName>>,
    templates: RefCell<HashMap<NodeId, NodeId>>,
    integration_points: RefCell<HashSet<NodeId>>,
    errors: Cell<usize>,
    max_nodes: usize,
    over_limit: Cell<bool>,
}

fn convert_name(name: &QualName) -> Name {
    Name {
        ns: Ns::from_uri(&name.ns),
        local: name.local.to_string(),
        prefix: name.prefix.as_ref().map(|prefix| prefix.to_string()),
    }
}

/// Attributes as HTML gives them, put in their namespaces. HTML has no namespaces outside SVG and
/// MathML, so `epub:type` arrives as a plain attribute with a colon in its name.
fn convert_attrs(attrs: Vec<Attribute>) -> Vec<Attr> {
    let mut converted: Vec<Attr> = Vec::with_capacity(attrs.len());
    for attribute in attrs {
        let mut name = convert_name(&attribute.name);
        if name.ns == Ns::Xmlns || (name.ns == Ns::None && name.local == "xmlns") {
            continue;
        }
        if name.ns == Ns::None
            && let Some((prefix, local)) = name.local.split_once(':')
        {
            if prefix == "xmlns" {
                continue;
            }
            match Ns::of_known_prefix(prefix) {
                Some(ns) if !local.is_empty() => {
                    name = Name {
                        ns,
                        local: local.to_owned(),
                        prefix: Some(prefix.to_owned()),
                    };
                }
                // An unknown prefix would not survive as XML; the attribute means nothing here.
                _ => continue,
            }
        }
        if converted
            .iter()
            .any(|attr| attr.name.ns == name.ns && attr.name.local == name.local)
        {
            continue;
        }
        converted.push(Attr {
            name,
            value: attribute.value.to_string(),
        });
    }
    converted
}

impl Sink {
    fn new_node(&self, make: impl FnOnce(&mut Dom) -> NodeId) -> NodeId {
        let mut dom = self.dom.borrow_mut();
        if dom.len() >= self.max_nodes {
            self.over_limit.set(true);
        }
        make(&mut dom)
    }

    fn insert_text(&self, parent: NodeId, index: Option<usize>, text: &str) {
        let mut dom = self.dom.borrow_mut();
        match index {
            None => dom.append_text(parent, text),
            Some(index) => {
                // Text goes into the text node before, if there is one, as a browser does it.
                if index > 0 {
                    let previous = dom.children(parent)[index - 1];
                    if let Some(existing) = dom.text_mut(previous) {
                        existing.push_str(text);
                        return;
                    }
                }
                let node = dom.create_text(text.to_owned());
                dom.insert(parent, index, node);
            }
        }
    }
}

impl TreeSink for Sink {
    type Handle = NodeId;
    type Output = Self;
    type ElemName<'a> = OwnedName;

    fn finish(self) -> Self {
        self
    }

    fn parse_error(&self, _msg: Cow<'static, str>) {
        self.errors.set(self.errors.get() + 1);
    }

    fn get_document(&self) -> NodeId {
        Dom::DOCUMENT
    }

    fn elem_name<'a>(&'a self, target: &'a NodeId) -> OwnedName {
        OwnedName(self.names.borrow().get(target).cloned().unwrap_or_else(|| {
            QualName::new(None, html5ever::ns!(html), html5ever::local_name!("span"))
        }))
    }

    fn create_element(&self, name: QualName, attrs: Vec<Attribute>, flags: ElementFlags) -> NodeId {
        let converted = convert_name(&name);
        let attrs = convert_attrs(attrs);
        let node = self.new_node(|dom| dom.create_element(converted, attrs));
        self.names.borrow_mut().insert(node, name);
        if flags.template {
            // A template's content is inert and is not among its children in a browser's DOM.
            let content =
                self.new_node(|dom| dom.create_element(Name::html("template"), Vec::new()));
            self.templates.borrow_mut().insert(node, content);
        }
        if flags.mathml_annotation_xml_integration_point {
            self.integration_points.borrow_mut().insert(node);
        }
        node
    }

    // Comments and processing instructions become empty text, which disappears with the rest of
    // the empty text at the end. They have to exist until then, because the tree builder may use
    // them as a place to insert before.
    fn create_comment(&self, _text: StrTendril) -> NodeId {
        self.new_node(|dom| dom.create_text(String::new()))
    }

    fn create_pi(&self, _target: StrTendril, _data: StrTendril) -> NodeId {
        self.new_node(|dom| dom.create_text(String::new()))
    }

    fn append(&self, parent: &NodeId, child: NodeOrText<NodeId>) {
        match child {
            NodeOrText::AppendNode(node) => self.dom.borrow_mut().append(*parent, node),
            NodeOrText::AppendText(text) => self.insert_text(*parent, None, &text),
        }
    }

    fn append_based_on_parent_node(
        &self,
        element: &NodeId,
        prev_element: &NodeId,
        child: NodeOrText<NodeId>,
    ) {
        let has_parent = self.dom.borrow().parent(*element).is_some();
        if has_parent {
            self.append_before_sibling(element, child);
        } else {
            self.append(prev_element, child);
        }
    }

    fn append_doctype_to_document(&self, _: StrTendril, _: StrTendril, _: StrTendril) {}

    fn get_template_contents(&self, target: &NodeId) -> NodeId {
        if let Some(&content) = self.templates.borrow().get(target) {
            return content;
        }
        let content = self.new_node(|dom| dom.create_element(Name::html("template"), Vec::new()));
        self.templates.borrow_mut().insert(*target, content);
        content
    }

    fn same_node(&self, x: &NodeId, y: &NodeId) -> bool {
        x == y
    }

    fn set_quirks_mode(&self, _mode: QuirksMode) {}

    fn append_before_sibling(&self, sibling: &NodeId, new_node: NodeOrText<NodeId>) {
        let (parent, index) = {
            let dom = self.dom.borrow();
            let Some(parent) = dom.parent(*sibling) else {
                return;
            };
            let Some(index) = dom.index_in_parent(*sibling) else {
                return;
            };
            (parent, index)
        };
        match new_node {
            NodeOrText::AppendNode(node) => self.dom.borrow_mut().insert(parent, index, node),
            NodeOrText::AppendText(text) => self.insert_text(parent, Some(index), &text),
        }
    }

    fn add_attrs_if_missing(&self, target: &NodeId, attrs: Vec<Attribute>) {
        let mut dom = self.dom.borrow_mut();
        let missing: Vec<Attr> = convert_attrs(attrs)
            .into_iter()
            .filter(|attr| {
                dom.attr_ns(*target, &attr.name.ns, &attr.name.local)
                    .is_none()
            })
            .collect();
        if let Some(existing) = dom.attrs_mut(*target) {
            existing.extend(missing);
        }
    }

    fn remove_from_parent(&self, target: &NodeId) {
        self.dom.borrow_mut().detach(*target);
    }

    fn reparent_children(&self, node: &NodeId, new_parent: &NodeId) {
        let mut dom = self.dom.borrow_mut();
        let children = dom.children(*node).to_vec();
        for child in children {
            dom.append(*new_parent, child);
        }
    }

    fn is_mathml_annotation_xml_integration_point(&self, handle: &NodeId) -> bool {
        self.integration_points.borrow().contains(handle)
    }
}

/// What parsing HTML gave.
#[derive(Debug)]
pub struct Parsed {
    pub dom: Dom,
    /// Parse errors the algorithm recovered from, for the record.
    pub errors: usize,
}

/// Parses an HTML document. Nothing is refused, except a document that would build more than
/// `max_nodes` nodes.
pub fn parse(text: &str, max_nodes: usize) -> Option<Parsed> {
    let sink = Sink {
        dom: RefCell::new(Dom::new()),
        names: RefCell::new(HashMap::new()),
        templates: RefCell::new(HashMap::new()),
        integration_points: RefCell::new(HashSet::new()),
        errors: Cell::new(0),
        max_nodes,
        over_limit: Cell::new(false),
    };
    let options = ParseOpts {
        tree_builder: TreeBuilderOpts {
            scripting_enabled: false,
            drop_doctype: true,
            ..TreeBuilderOpts::default()
        },
        ..ParseOpts::default()
    };
    let sink = parse_document(sink, options).one(text);
    if sink.over_limit.get() {
        return None;
    }
    let errors = sink.errors.get();
    let mut dom = sink.dom.into_inner();
    dom.normalize_text(Dom::DOCUMENT);
    Some(Parsed { dom, errors })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broken_markup_gets_the_browser_tree() {
        // With a doctype, so the table closes the paragraph: without one, quirks mode puts it
        // inside, as a browser would too.
        let parsed = parse(
            "<!DOCTYPE html><p>one<p>two <b>bold <i>both</b> italic</i><table><td>cell</table>\
             <section epub:type='chapter' xmlns:epub='x'>s</section><!-- c --><template><p>t</p></template>",
            100_000,
        )
        .unwrap();
        let dom = &parsed.dom;
        let body = dom.body().unwrap();
        let names: Vec<&str> = dom
            .element_children(body)
            .map(|child| dom.name(child).unwrap().local.as_str())
            .collect();
        assert_eq!(names, ["p", "p", "table", "section", "template"]);
        let table = dom.element_children(body).nth(2).unwrap();
        let tbody = dom.element_children(table).next().unwrap();
        assert!(dom.is_html(tbody, "tbody"), "the implied tbody is there");
        let section = dom.element_children(body).nth(3).unwrap();
        assert_eq!(dom.attr_ns(section, &Ns::Ops, "type"), Some("chapter"));
        let template = dom.element_children(body).nth(4).unwrap();
        assert!(
            dom.children(template).is_empty(),
            "template content is not a child"
        );
        assert!(parsed.errors > 0);
    }

    #[test]
    fn noscript_content_is_markup() {
        let parsed = parse("<body><noscript><p>shown</p></noscript></body>", 1000).unwrap();
        let dom = &parsed.dom;
        let noscript = dom.element_children(dom.body().unwrap()).next().unwrap();
        assert!(dom.is_html(dom.element_children(noscript).next().unwrap(), "p"));
    }

    #[test]
    fn svg_keeps_its_names() {
        let parsed = parse(
            "<svg><foreignobject/><lineargradient/><a xlink:href='#x'/></svg>",
            1000,
        )
        .unwrap();
        let dom = &parsed.dom;
        let svg = dom.element_children(dom.body().unwrap()).next().unwrap();
        let names: Vec<&str> = dom
            .element_children(svg)
            .map(|child| dom.name(child).unwrap().local.as_str())
            .collect();
        assert_eq!(names, ["foreignObject", "linearGradient", "a"]);
        let a = dom.element_children(svg).nth(2).unwrap();
        assert_eq!(dom.attr_ns(a, &Ns::Xlink, "href"), Some("#x"));
    }

    #[test]
    fn a_node_bomb_is_refused() {
        assert!(parse(&"<b>".repeat(5000), 1000).is_none());
    }
}

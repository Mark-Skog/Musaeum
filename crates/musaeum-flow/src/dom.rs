//! A document tree that owns its nodes: elements and text, and nothing else.
//!
//! Comments, processing instructions and doctypes never get in, CDATA becomes text, and adjacent text
//! is one node. That is the tree an EPUB CFI counts in anyway: it steps through elements and runs of
//! character data, and nothing else.

/// A node, by its place in its [`Dom`].
pub type NodeId = usize;

pub const HTML_NS: &str = "http://www.w3.org/1999/xhtml";
pub const SVG_NS: &str = "http://www.w3.org/2000/svg";
pub const MATHML_NS: &str = "http://www.w3.org/1998/Math/MathML";
pub const XLINK_NS: &str = "http://www.w3.org/1999/xlink";
pub const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";
pub const XMLNS_NS: &str = "http://www.w3.org/2000/xmlns/";
pub const OPS_NS: &str = "http://www.idpf.org/2007/ops";

/// A namespace: the ones books use by name, any other by its URI.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Ns {
    /// No namespace, as the attributes of XHTML elements have.
    None,
    Html,
    Svg,
    MathMl,
    Xlink,
    Xml,
    Xmlns,
    /// EPUB's `epub:` namespace.
    Ops,
    Other(String),
}

impl Ns {
    pub fn from_uri(uri: &str) -> Self {
        match uri {
            "" => Self::None,
            HTML_NS => Self::Html,
            SVG_NS => Self::Svg,
            MATHML_NS => Self::MathMl,
            XLINK_NS => Self::Xlink,
            XML_NS => Self::Xml,
            XMLNS_NS => Self::Xmlns,
            OPS_NS => Self::Ops,
            other => Self::Other(other.to_owned()),
        }
    }

    pub fn uri(&self) -> &str {
        match self {
            Self::None => "",
            Self::Html => HTML_NS,
            Self::Svg => SVG_NS,
            Self::MathMl => MATHML_NS,
            Self::Xlink => XLINK_NS,
            Self::Xml => XML_NS,
            Self::Xmlns => XMLNS_NS,
            Self::Ops => OPS_NS,
            Self::Other(uri) => uri,
        }
    }

    /// The prefix this namespace usually has, to use when a document did not give one.
    pub fn usual_prefix(&self) -> Option<&'static str> {
        match self {
            Self::Svg => Some("svg"),
            Self::MathMl => Some("m"),
            Self::Xlink => Some("xlink"),
            Self::Xml => Some("xml"),
            Self::Ops => Some("epub"),
            _ => None,
        }
    }

    /// The namespace a well-known prefix stands for, for documents that use one without declaring
    /// it, which browsers refuse and books do anyway.
    pub fn of_known_prefix(prefix: &str) -> Option<Self> {
        match prefix {
            "xml" => Some(Self::Xml),
            "xlink" => Some(Self::Xlink),
            "epub" | "ops" => Some(Self::Ops),
            "svg" => Some(Self::Svg),
            "m" | "mml" | "math" | "mathml" => Some(Self::MathMl),
            "xhtml" | "html" => Some(Self::Html),
            _ => None,
        }
    }
}

/// An element's or an attribute's name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Name {
    pub ns: Ns,
    pub local: String,
    /// The prefix the source used, kept so the output looks like the input. Only a hint: the
    /// serializer declares whatever the output needs.
    pub prefix: Option<String>,
}

impl Name {
    pub fn new(ns: Ns, local: &str) -> Self {
        Self {
            ns,
            local: local.to_owned(),
            prefix: None,
        }
    }

    /// An XHTML element's name.
    pub fn html(local: &str) -> Self {
        Self::new(Ns::Html, local)
    }

    /// An attribute's name with no namespace, which is what nearly every attribute has.
    pub fn plain(local: &str) -> Self {
        Self::new(Ns::None, local)
    }

    pub fn is(&self, ns: &Ns, local: &str) -> bool {
        self.ns == *ns && self.local == local
    }

    pub fn is_html(&self, local: &str) -> bool {
        self.is(&Ns::Html, local)
    }
}

/// One attribute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attr {
    pub name: Name,
    pub value: String,
}

/// What a node is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Data {
    Document,
    Element { name: Name, attrs: Vec<Attr> },
    Text(String),
}

/// A node and where it is.
#[derive(Clone, Debug)]
pub struct Node {
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    pub data: Data,
}

/// A whole document. Node 0 is the document itself; detached nodes stay in the arena unused.
#[derive(Clone, Debug)]
pub struct Dom {
    nodes: Vec<Node>,
}

impl Default for Dom {
    fn default() -> Self {
        Self::new()
    }
}

impl Dom {
    pub const DOCUMENT: NodeId = 0;

    pub fn new() -> Self {
        Self {
            nodes: vec![Node {
                parent: None,
                children: Vec::new(),
                data: Data::Document,
            }],
        }
    }

    /// How many nodes the arena holds, detached ones included.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.len() <= 1
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id]
    }

    pub fn data(&self, id: NodeId) -> &Data {
        &self.nodes[id].data
    }

    pub fn children(&self, id: NodeId) -> &[NodeId] {
        &self.nodes[id].children
    }

    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.nodes[id].parent
    }

    pub fn name(&self, id: NodeId) -> Option<&Name> {
        match &self.nodes[id].data {
            Data::Element { name, .. } => Some(name),
            _ => None,
        }
    }

    pub fn name_mut(&mut self, id: NodeId) -> Option<&mut Name> {
        match &mut self.nodes[id].data {
            Data::Element { name, .. } => Some(name),
            _ => None,
        }
    }

    pub fn is_element(&self, id: NodeId) -> bool {
        matches!(self.nodes[id].data, Data::Element { .. })
    }

    pub fn is_text(&self, id: NodeId) -> bool {
        matches!(self.nodes[id].data, Data::Text(_))
    }

    /// Whether `id` is the element `local` in `ns`.
    pub fn is(&self, id: NodeId, ns: &Ns, local: &str) -> bool {
        self.name(id).is_some_and(|name| name.is(ns, local))
    }

    pub fn is_html(&self, id: NodeId, local: &str) -> bool {
        self.is(id, &Ns::Html, local)
    }

    pub fn text(&self, id: NodeId) -> Option<&str> {
        match &self.nodes[id].data {
            Data::Text(text) => Some(text),
            _ => None,
        }
    }

    pub fn text_mut(&mut self, id: NodeId) -> Option<&mut String> {
        match &mut self.nodes[id].data {
            Data::Text(text) => Some(text),
            _ => None,
        }
    }

    pub fn attrs(&self, id: NodeId) -> &[Attr] {
        match &self.nodes[id].data {
            Data::Element { attrs, .. } => attrs,
            _ => &[],
        }
    }

    pub fn attrs_mut(&mut self, id: NodeId) -> Option<&mut Vec<Attr>> {
        match &mut self.nodes[id].data {
            Data::Element { attrs, .. } => Some(attrs),
            _ => None,
        }
    }

    /// An attribute with no namespace.
    pub fn attr(&self, id: NodeId, local: &str) -> Option<&str> {
        self.attr_ns(id, &Ns::None, local)
    }

    pub fn attr_ns(&self, id: NodeId, ns: &Ns, local: &str) -> Option<&str> {
        self.attrs(id)
            .iter()
            .find(|attr| attr.name.is(ns, local))
            .map(|attr| attr.value.as_str())
    }

    /// Sets an attribute, replacing one with the same name.
    pub fn set_attr(&mut self, id: NodeId, name: Name, value: String) {
        if let Some(attrs) = self.attrs_mut(id) {
            if let Some(existing) = attrs
                .iter_mut()
                .find(|attr| attr.name.ns == name.ns && attr.name.local == name.local)
            {
                existing.value = value;
            } else {
                attrs.push(Attr { name, value });
            }
        }
    }

    pub fn remove_attr(&mut self, id: NodeId, ns: &Ns, local: &str) {
        if let Some(attrs) = self.attrs_mut(id) {
            attrs.retain(|attr| !attr.name.is(ns, local));
        }
    }

    pub fn create_element(&mut self, name: Name, attrs: Vec<Attr>) -> NodeId {
        self.push(Data::Element { name, attrs })
    }

    pub fn create_text(&mut self, text: String) -> NodeId {
        self.push(Data::Text(text))
    }

    fn push(&mut self, data: Data) -> NodeId {
        self.nodes.push(Node {
            parent: None,
            children: Vec::new(),
            data,
        });
        self.nodes.len() - 1
    }

    /// Appends a detached node as the last child of `parent`.
    pub fn append(&mut self, parent: NodeId, child: NodeId) {
        self.detach(child);
        self.nodes[child].parent = Some(parent);
        self.nodes[parent].children.push(child);
    }

    /// Appends text, into the last child if that is text already.
    pub fn append_text(&mut self, parent: NodeId, text: &str) {
        if text.is_empty() {
            return;
        }
        if let Some(&last) = self.nodes[parent].children.last()
            && let Data::Text(existing) = &mut self.nodes[last].data
        {
            existing.push_str(text);
            return;
        }
        let node = self.create_text(text.to_owned());
        self.append(parent, node);
    }

    /// Inserts a detached node so that it becomes child number `index` of `parent`.
    pub fn insert(&mut self, parent: NodeId, index: usize, child: NodeId) {
        self.detach(child);
        self.nodes[child].parent = Some(parent);
        let index = index.min(self.nodes[parent].children.len());
        self.nodes[parent].children.insert(index, child);
    }

    /// Takes a node out of its parent. It stays in the arena and can be put elsewhere.
    pub fn detach(&mut self, id: NodeId) {
        if let Some(parent) = self.nodes[id].parent.take() {
            self.nodes[parent].children.retain(|&child| child != id);
        }
    }

    /// Removes every child of `id`.
    pub fn clear_children(&mut self, id: NodeId) {
        for child in std::mem::take(&mut self.nodes[id].children) {
            self.nodes[child].parent = None;
        }
    }

    /// The position of `id` among its parent's children.
    pub fn index_in_parent(&self, id: NodeId) -> Option<usize> {
        let parent = self.nodes[id].parent?;
        self.nodes[parent]
            .children
            .iter()
            .position(|&child| child == id)
    }

    /// The document's root element.
    pub fn root_element(&self) -> Option<NodeId> {
        self.nodes[Self::DOCUMENT]
            .children
            .iter()
            .copied()
            .find(|&child| self.is_element(child))
    }

    pub fn element_children(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        self.nodes[id]
            .children
            .iter()
            .copied()
            .filter(|&child| self.is_element(child))
    }

    /// The first child element of `id` that is `local` in `ns`.
    pub fn child(&self, id: NodeId, ns: &Ns, local: &str) -> Option<NodeId> {
        self.element_children(id)
            .find(|&child| self.is(child, ns, local))
    }

    /// `<body>` under the root `<html>`, if the document is XHTML.
    pub fn body(&self) -> Option<NodeId> {
        let root = self.root_element()?;
        if !self.is_html(root, "html") {
            return None;
        }
        self.child(root, &Ns::Html, "body")
    }

    /// `<head>` under the root `<html>`.
    pub fn head(&self) -> Option<NodeId> {
        let root = self.root_element()?;
        if !self.is_html(root, "html") {
            return None;
        }
        self.child(root, &Ns::Html, "head")
    }

    /// `id` and everything under it, in document order.
    pub fn descendants(&self, id: NodeId) -> Descendants<'_> {
        Descendants {
            dom: self,
            stack: vec![id],
        }
    }

    /// The text under `id`, in document order.
    pub fn text_content(&self, id: NodeId) -> String {
        let mut text = String::new();
        for node in self.descendants(id) {
            if let Data::Text(value) = &self.nodes[node].data {
                text.push_str(value);
            }
        }
        text
    }

    /// How deep the deepest node under `id` is, `id` counting as 1.
    pub fn depth(&self, id: NodeId) -> usize {
        let mut deepest = 0;
        let mut stack = vec![(id, 1usize)];
        while let Some((node, depth)) = stack.pop() {
            deepest = deepest.max(depth);
            for &child in &self.nodes[node].children {
                stack.push((child, depth + 1));
            }
        }
        deepest
    }

    /// Merges adjacent text nodes under `id` and drops empty ones, which is how a browser's parser
    /// leaves character data.
    pub fn normalize_text(&mut self, id: NodeId) {
        let mut stack = vec![id];
        while let Some(node) = stack.pop() {
            let children = std::mem::take(&mut self.nodes[node].children);
            let mut kept: Vec<NodeId> = Vec::with_capacity(children.len());
            for child in children {
                let is_text = matches!(self.nodes[child].data, Data::Text(_));
                if is_text {
                    let text = match &mut self.nodes[child].data {
                        Data::Text(text) => std::mem::take(text),
                        _ => unreachable!(),
                    };
                    if text.is_empty() {
                        self.nodes[child].parent = None;
                        continue;
                    }
                    if let Some(&previous) = kept.last()
                        && let Data::Text(existing) = &mut self.nodes[previous].data
                    {
                        existing.push_str(&text);
                        self.nodes[child].parent = None;
                        continue;
                    }
                    self.nodes[child].data = Data::Text(text);
                } else {
                    stack.push(child);
                }
                kept.push(child);
            }
            self.nodes[node].children = kept;
        }
    }

    /// Copies the subtree under `id` into `target`, returning the copy's root, detached.
    pub fn copy_into(&self, id: NodeId, target: &mut Dom) -> NodeId {
        let root = target.push(self.nodes[id].data.clone());
        let mut stack = vec![(id, root)];
        while let Some((source, copy)) = stack.pop() {
            for &child in &self.nodes[source].children {
                let child_copy = target.push(self.nodes[child].data.clone());
                target.nodes[child_copy].parent = Some(copy);
                target.nodes[copy].children.push(child_copy);
                stack.push((child, child_copy));
            }
        }
        root
    }

    /// A copy of the element `id` with its attributes and no children, detached.
    pub fn shallow_copy_into(&self, id: NodeId, target: &mut Dom) -> NodeId {
        target.push(self.nodes[id].data.clone())
    }
}

/// Pre-order traversal.
pub struct Descendants<'a> {
    dom: &'a Dom,
    stack: Vec<NodeId>,
}

impl Iterator for Descendants<'_> {
    type Item = NodeId;

    fn next(&mut self) -> Option<NodeId> {
        let node = self.stack.pop()?;
        self.stack
            .extend(self.dom.nodes[node].children.iter().rev().copied());
        Some(node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_merged_and_empty_text_dropped() {
        let mut dom = Dom::new();
        let root = dom.create_element(Name::html("p"), Vec::new());
        dom.append(Dom::DOCUMENT, root);
        for text in ["a", "", "b"] {
            let node = dom.create_text(text.to_owned());
            dom.append(root, node);
        }
        let em = dom.create_element(Name::html("em"), Vec::new());
        dom.append(root, em);
        let c = dom.create_text("c".to_owned());
        dom.append(root, c);
        dom.normalize_text(Dom::DOCUMENT);
        assert_eq!(dom.children(root).len(), 3);
        assert_eq!(dom.text(dom.children(root)[0]), Some("ab"));
        assert_eq!(dom.text_content(root), "abc");
    }

    #[test]
    fn a_subtree_is_copied_whole() {
        let mut dom = Dom::new();
        let root = dom.create_element(Name::html("div"), Vec::new());
        dom.append(Dom::DOCUMENT, root);
        let p = dom.create_element(Name::html("p"), Vec::new());
        dom.append(root, p);
        dom.append_text(p, "hello");
        let mut other = Dom::new();
        let copy = dom.copy_into(root, &mut other);
        other.append(Dom::DOCUMENT, copy);
        assert_eq!(other.text_content(copy), "hello");
        assert_eq!(other.descendants(Dom::DOCUMENT).count(), 4);
    }
}

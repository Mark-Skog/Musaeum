//! The content of a book, made inert: no script, no event handler, nothing that loads by itself
//! from outside the book.
//!
//! Elements that could run code or embed something live are not removed but neutralized: they
//! become a `span` or `div` in the same place, marked `data-musaeum-removed`, so that the elements
//! after them keep their positions and a CFI into the original book still points at the same node.
//! In `<head>`, where positions mean nothing, they are removed.
//!
//! Every URL goes through a [`Resolver`], which knows the book: references into it are rewritten,
//! links to web pages stay as links (they are only followed on a click, which the reader handles),
//! and anything else is dropped.

use crate::css::{self, UrlUse};
use crate::dom::{Attr, Dom, Name, NodeId, Ns};
use crate::serialize::is_ncname;

/// What a URL in a document is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Usage {
    /// A hyperlink, followed only by a click.
    Link,
    /// An image, loaded with the document.
    Image,
    /// Audio, video or a text track.
    Media,
    /// A stylesheet from `<link>`.
    Stylesheet,
    /// An image, font or the like from CSS in the document.
    CssResource,
    /// A reference that must stay inside the document, such as SVG's `<use href="#a">`.
    FragmentOnly,
}

/// Knows the book, and decides what each reference becomes.
pub trait Resolver {
    /// The value to write instead of `url`, or `None` to drop it.
    fn resolve(&mut self, url: &str, usage: Usage) -> Option<String>;
}

/// What was neutralized or dropped, by kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Report {
    /// `<script>` elements, in HTML and SVG.
    pub scripts: u32,
    /// `on…` attributes.
    pub event_handlers: u32,
    /// Frames, objects, embeds and applets.
    pub embedded: u32,
    /// `<meta http-equiv>`, `<base>` and `<link>` other than stylesheets.
    pub head_directives: u32,
    /// Form submissions (`action`, `formaction`) and forms.
    pub forms: u32,
    /// URLs dropped: outside the book (other than links), `javascript:` and the like, or broken.
    pub dropped_urls: u32,
    /// Links to web pages kept as links.
    pub external_links: u32,
    /// Other attributes that could load or run something (`srcdoc`, `ping`, `srcset`, …).
    pub dropped_attributes: u32,
    /// SVG animations that could rewrite a link.
    pub animations: u32,
    /// Elements whose names XML cannot carry.
    pub renamed: u32,
}

impl Report {
    pub fn add(&mut self, other: &Report) {
        self.scripts += other.scripts;
        self.event_handlers += other.event_handlers;
        self.embedded += other.embedded;
        self.head_directives += other.head_directives;
        self.forms += other.forms;
        self.dropped_urls += other.dropped_urls;
        self.external_links += other.external_links;
        self.dropped_attributes += other.dropped_attributes;
        self.animations += other.animations;
        self.renamed += other.renamed;
    }

    pub fn css(&mut self, report: &css::CssReport) {
        self.dropped_urls += (report.dropped_urls + report.dropped_imports) as u32;
        self.dropped_attributes += report.dropped_expressions as u32;
    }
}

/// The attribute that marks a neutralized element, with the original element's name.
pub const REMOVED_ATTRIBUTE: &str = "data-musaeum-removed";

/// HTML elements whose content shows as a block, to neutralize into a `div` rather than a `span`.
const BLOCKISH: [&str; 6] = ["form", "frameset", "noscript", "iframe", "object", "applet"];

/// Attributes that load or run something and have no safe form here.
const DROPPED_ATTRIBUTES: [&str; 19] = [
    "srcdoc",
    "srcset",
    "imagesrcset",
    "sizes",
    "ping",
    "download",
    "target",
    "longdesc",
    "lowsrc",
    "dynsrc",
    "manifest",
    "codebase",
    "archive",
    "classid",
    "profile",
    "icon",
    "formtarget",
    "autofocus",
    "referrerpolicy",
];

struct Walk<'a> {
    dom: &'a mut Dom,
    resolver: &'a mut dyn Resolver,
    report: Report,
}

impl Walk<'_> {
    fn css_rewriter(&mut self) -> impl FnMut(&str, UrlUse) -> Option<String> + '_ {
        let resolver = &mut *self.resolver;
        move |url: &str, usage: UrlUse| match usage {
            UrlUse::Import => resolver.resolve(url, Usage::Stylesheet),
            UrlUse::Resource => {
                if url.trim_start().starts_with('#') {
                    // A fragment in CSS names something in the document, like an SVG filter.
                    Some(url.to_owned())
                } else {
                    resolver.resolve(url, Usage::CssResource)
                }
            }
        }
    }

    fn sanitize_style_value(&mut self, value: &str) -> String {
        let (clean, report) = {
            let mut rewrite = self.css_rewriter();
            css::sanitize_declarations(value, &mut rewrite)
        };
        self.report.css(&report);
        clean
    }

    /// Turns `node` into an inert `span` or `div` at the same place, keeping only `id` and
    /// `class`. Its children stay if they are content a reader would see in its place.
    fn neutralize(&mut self, node: NodeId, keep_children: bool, hidden: bool) {
        let Some(original) = self.dom.name(node).cloned() else {
            return;
        };
        let block = original.ns == Ns::Html && BLOCKISH.contains(&original.local.as_str());
        let mut attrs: Vec<Attr> = self
            .dom
            .attrs(node)
            .iter()
            .filter(|attr| {
                attr.name.ns == Ns::None && matches!(attr.name.local.as_str(), "id" | "class")
            })
            .cloned()
            .collect();
        let marker = if is_ncname(&original.local) {
            original.local.clone()
        } else {
            "element".to_owned()
        };
        attrs.push(Attr {
            name: Name::plain(REMOVED_ATTRIBUTE),
            value: marker,
        });
        if hidden {
            attrs.push(Attr {
                name: Name::plain("hidden"),
                value: "hidden".to_owned(),
            });
        }
        if let Some(name) = self.dom.name_mut(node) {
            *name = Name::html(if block { "div" } else { "span" });
        }
        if let Some(existing) = self.dom.attrs_mut(node) {
            *existing = attrs;
        }
        if !keep_children {
            self.dom.clear_children(node);
        }
    }

    /// An SVG element that could run something, made an empty, hidden group.
    fn neutralize_svg(&mut self, node: NodeId) {
        let original = self
            .dom
            .name(node)
            .map(|name| name.local.clone())
            .unwrap_or_default();
        if let Some(name) = self.dom.name_mut(node) {
            *name = Name::new(Ns::Svg, "g");
        }
        if let Some(attrs) = self.dom.attrs_mut(node) {
            *attrs = vec![
                Attr {
                    name: Name::plain(REMOVED_ATTRIBUTE),
                    value: if is_ncname(&original) {
                        original
                    } else {
                        "element".to_owned()
                    },
                },
                Attr {
                    name: Name::plain("display"),
                    value: "none".to_owned(),
                },
            ];
        }
        self.dom.clear_children(node);
    }

    fn resolve_attr(&mut self, node: NodeId, index: usize, usage: Usage) {
        let Some(value) = self
            .dom
            .attrs(node)
            .get(index)
            .map(|attr| attr.value.clone())
        else {
            return;
        };
        let resolved = self.resolver.resolve(&value, usage);
        let attrs = self.dom.attrs_mut(node).expect("an element");
        match resolved {
            Some(new) => {
                if usage == Usage::Link
                    && let crate::url::Reference::External { scheme, .. } =
                        crate::url::resolve("", &value)
                    && matches!(scheme.as_str(), "http" | "https" | "mailto" | "tel")
                {
                    self.report.external_links += 1;
                }
                attrs[index].value = new;
            }
            None => {
                attrs.remove(index);
                self.report.dropped_urls += 1;
            }
        }
    }

    /// What one attribute of an element is for, if it holds a URL.
    fn url_usage(element: &Name, attr: &Name) -> Option<Usage> {
        let is_href = attr.local == "href" && matches!(attr.ns, Ns::None | Ns::Xlink);
        match element.ns {
            Ns::Html => match (element.local.as_str(), attr.local.as_str(), &attr.ns) {
                ("a" | "area", "href", Ns::None) => Some(Usage::Link),
                ("img" | "input", "src", Ns::None) => Some(Usage::Image),
                ("video", "poster", Ns::None) => Some(Usage::Image),
                ("body" | "table" | "td" | "th" | "tr", "background", Ns::None) => {
                    Some(Usage::Image)
                }
                ("audio" | "video" | "source" | "track", "src", Ns::None) => Some(Usage::Media),
                ("link", "href", Ns::None) => Some(Usage::Stylesheet),
                ("img" | "object", "usemap", Ns::None) => Some(Usage::FragmentOnly),
                // Any other URL attribute on an HTML element: an XLink means nothing there.
                (_, "href", Ns::Xlink) => Some(Usage::FragmentOnly),
                (_, "src" | "data", Ns::None) => Some(Usage::FragmentOnly),
                _ => None,
            },
            Ns::Svg => match element.local.as_str() {
                "a" if is_href => Some(Usage::Link),
                "image" | "feImage" if is_href => Some(Usage::Image),
                _ if is_href => Some(Usage::FragmentOnly),
                _ => None,
            },
            Ns::MathMl if is_href => Some(Usage::Link),
            _ if is_href => Some(Usage::FragmentOnly),
            _ => None,
        }
    }

    fn sanitize_attributes(&mut self, node: NodeId) {
        let Some(element) = self.dom.name(node).cloned() else {
            return;
        };
        let mut index = 0;
        while index < self.dom.attrs(node).len() {
            let attr = self.dom.attrs(node)[index].clone();
            let local = attr.name.local.to_ascii_lowercase();
            let attrs_len = self.dom.attrs(node).len();
            if local.starts_with("on") {
                self.dom.attrs_mut(node).expect("an element").remove(index);
                self.report.event_handlers += 1;
                continue;
            }
            if attr.name.ns == Ns::Xml && local == "base" {
                self.dom.attrs_mut(node).expect("an element").remove(index);
                self.report.dropped_attributes += 1;
                continue;
            }
            if attr.name.ns == Ns::None
                && matches!(
                    local.as_str(),
                    "action" | "formaction" | "formmethod" | "formenctype"
                )
            {
                self.dom.attrs_mut(node).expect("an element").remove(index);
                self.report.forms += 1;
                continue;
            }
            if attr.name.ns == Ns::None && DROPPED_ATTRIBUTES.contains(&local.as_str()) {
                self.dom.attrs_mut(node).expect("an element").remove(index);
                self.report.dropped_attributes += 1;
                continue;
            }
            if attr.name.ns == Ns::None && local == "style" {
                let clean = self.sanitize_style_value(&attr.value);
                self.dom.attrs_mut(node).expect("an element")[index].value = clean;
                index += 1;
                continue;
            }
            if let Some(usage) = Self::url_usage(&element, &attr.name) {
                self.resolve_attr(node, index, usage);
                if self.dom.attrs(node).len() < attrs_len {
                    continue;
                }
                index += 1;
                continue;
            }
            // SVG and MathML presentation attributes take url(#id), and could take a URL outside.
            if element.ns != Ns::Html && attr.value.to_ascii_lowercase().contains("url(") {
                let clean = self.sanitize_style_value(&format!("x:{}", attr.value));
                let value = clean.strip_prefix("x:").unwrap_or("none").to_owned();
                self.dom.attrs_mut(node).expect("an element")[index].value = value;
            }
            index += 1;
        }
    }

    fn sanitize_style_element(&mut self, node: NodeId) {
        let text = self.dom.text_content(node);
        let (clean, report) = {
            let mut rewrite = self.css_rewriter();
            css::sanitize_stylesheet(&text, &mut rewrite)
        };
        self.report.css(&report);
        self.dom.clear_children(node);
        if !clean.is_empty() {
            self.dom.append_text(node, &clean);
        }
        // Only CSS is CSS; anything else typed here would mean nothing to a reader.
        self.dom.remove_attr(node, &Ns::None, "type");
        self.dom.remove_attr(node, &Ns::None, "src");
    }

    /// Whether `node` is inside `<head>`.
    fn in_head(&self, node: NodeId) -> bool {
        let mut current = self.dom.parent(node);
        while let Some(parent) = current {
            if self.dom.is_html(parent, "head") {
                return true;
            }
            current = self.dom.parent(parent);
        }
        false
    }

    /// One element. Returns whether to look at its children.
    fn element(&mut self, node: NodeId) -> bool {
        let Some(name) = self.dom.name(node).cloned() else {
            return true;
        };
        match name.ns {
            Ns::Html => self.html_element(node, &name),
            Ns::Svg => self.svg_element(node, &name),
            _ => {
                if !is_ncname(&name.local) {
                    self.report.renamed += 1;
                    self.neutralize(node, true, false);
                }
                self.sanitize_attributes(node);
                true
            }
        }
    }

    fn html_element(&mut self, node: NodeId, name: &Name) -> bool {
        let local = name.local.as_str();
        let in_head = self.in_head(node);
        match local {
            "script" => {
                self.report.scripts += 1;
                if in_head {
                    self.dom.detach(node);
                } else {
                    self.neutralize(node, false, true);
                }
                false
            }
            "iframe" | "frame" | "frameset" | "portal" | "fencedframe" => {
                self.report.embedded += 1;
                self.neutralize(node, true, true);
                true
            }
            "object" | "applet" | "embed" => {
                self.report.embedded += 1;
                // An object's content is what shows when it cannot; that is the case here.
                self.neutralize(node, true, false);
                true
            }
            "param" => {
                self.neutralize(node, false, true);
                false
            }
            "base" => {
                self.report.head_directives += 1;
                if in_head {
                    self.dom.detach(node);
                } else {
                    self.neutralize(node, false, true);
                }
                false
            }
            "meta" => {
                let directive = self.dom.attr(node, "http-equiv").is_some()
                    || self.dom.attr(node, "charset").is_some();
                if directive {
                    self.report.head_directives += 1;
                    if in_head {
                        self.dom.detach(node);
                    } else {
                        self.neutralize(node, false, true);
                    }
                    return false;
                }
                self.sanitize_attributes(node);
                false
            }
            "link" => {
                let rel = self
                    .dom
                    .attr(node, "rel")
                    .unwrap_or("")
                    .to_ascii_lowercase();
                let stylesheet = rel
                    .split_ascii_whitespace()
                    .any(|word| word == "stylesheet");
                if stylesheet {
                    self.sanitize_attributes(node);
                    if self.dom.attr(node, "href").is_some() {
                        // Kept only as the stylesheet it is, whatever else it claimed to be.
                        self.dom
                            .set_attr(node, Name::plain("rel"), "stylesheet".to_owned());
                        self.dom
                            .set_attr(node, Name::plain("type"), "text/css".to_owned());
                        return false;
                    }
                } else {
                    self.report.head_directives += 1;
                }
                if in_head {
                    self.dom.detach(node);
                } else {
                    self.neutralize(node, false, true);
                }
                false
            }
            "form" => {
                self.report.forms += 1;
                self.neutralize(node, true, false);
                true
            }
            "noscript" => {
                // No script runs here, so this is what a reader should see.
                if in_head {
                    self.dom.detach(node);
                    return false;
                }
                self.neutralize(node, true, false);
                true
            }
            "style" => {
                self.sanitize_attributes(node);
                self.sanitize_style_element(node);
                false
            }
            "template" => {
                // Its content is inert, and not among its children in a browser.
                self.dom.clear_children(node);
                self.sanitize_attributes(node);
                false
            }
            _ => {
                if !is_ncname(local) {
                    self.report.renamed += 1;
                    self.neutralize(node, true, false);
                    return true;
                }
                self.sanitize_attributes(node);
                true
            }
        }
    }

    fn svg_element(&mut self, node: NodeId, name: &Name) -> bool {
        match name.local.as_str() {
            "script" => {
                self.report.scripts += 1;
                self.neutralize_svg(node);
                false
            }
            "set" | "animate" | "animateTransform" | "animateMotion" | "animateColor" => {
                let target = self
                    .dom
                    .attr(node, "attributeName")
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if target.ends_with("href") || target.starts_with("on") || target == "style" {
                    self.report.animations += 1;
                    self.neutralize_svg(node);
                    return false;
                }
                self.sanitize_attributes(node);
                true
            }
            "style" => {
                self.sanitize_attributes(node);
                self.sanitize_style_element(node);
                false
            }
            "iframe" | "embed" | "object" | "handler" | "listener" => {
                self.report.embedded += 1;
                self.neutralize_svg(node);
                false
            }
            local => {
                if !is_ncname(local) {
                    self.report.renamed += 1;
                    self.neutralize_svg(node);
                    return false;
                }
                self.sanitize_attributes(node);
                true
            }
        }
    }
}

/// Makes the document in `dom` inert, through `resolver` for every reference.
pub fn sanitize(dom: &mut Dom, resolver: &mut dyn Resolver) -> Report {
    let mut walk = Walk {
        dom,
        resolver,
        report: Report::default(),
    };
    let mut stack = vec![Dom::DOCUMENT];
    while let Some(node) = stack.pop() {
        if walk.dom.is_element(node) && !walk.element(node) {
            continue;
        }
        // The element may have been detached; its children are then out of the document too.
        if node != Dom::DOCUMENT && walk.dom.parent(node).is_none() {
            continue;
        }
        stack.extend(walk.dom.children(node).iter().rev().copied());
    }
    walk.dom.normalize_text(Dom::DOCUMENT);
    walk.report
}

/// A stylesheet file, through `resolver`, which knows the file's own location.
pub fn sanitize_stylesheet_file(css_text: &str, resolver: &mut dyn Resolver) -> (String, Report) {
    let mut report = Report::default();
    let (clean, css_report) = {
        let mut rewrite = |url: &str, usage: UrlUse| match usage {
            UrlUse::Import => resolver.resolve(url, Usage::Stylesheet),
            UrlUse::Resource if url.trim_start().starts_with('#') => Some(url.to_owned()),
            UrlUse::Resource => resolver.resolve(url, Usage::CssResource),
        };
        css::sanitize_stylesheet(css_text, &mut rewrite)
    };
    report.css(&css_report);
    (clean, report)
}

/// Whether anything in `dom` could still run or load from outside: what the tests and the corpus
/// check assert after sanitizing, written apart from the sanitizer so one cannot hide the other's
/// mistake. Returns a description of the first problem found.
pub fn find_danger(dom: &Dom) -> Option<String> {
    for node in dom.descendants(Dom::DOCUMENT) {
        let Some(name) = dom.name(node) else { continue };
        let local = name.local.to_ascii_lowercase();
        if matches!(name.ns, Ns::Html | Ns::Svg)
            && matches!(
                local.as_str(),
                "script"
                    | "iframe"
                    | "frame"
                    | "frameset"
                    | "object"
                    | "embed"
                    | "applet"
                    | "base"
                    | "portal"
            )
        {
            return Some(format!("a <{local}> element"));
        }
        if name.ns == Ns::Html && local == "meta" && dom.attr(node, "http-equiv").is_some() {
            return Some("a <meta http-equiv>".to_owned());
        }
        for attr in dom.attrs(node) {
            let key = attr.name.local.to_ascii_lowercase();
            if key.starts_with("on") {
                return Some(format!("an {key} attribute"));
            }
            if matches!(key.as_str(), "srcdoc" | "action" | "formaction" | "ping") {
                return Some(format!("a {key} attribute"));
            }
            let value = attr.value.trim().to_ascii_lowercase();
            let compact: String = value
                .chars()
                .filter(|c| !c.is_whitespace() && !c.is_control())
                .collect();
            let carries_url = matches!(
                key.as_str(),
                "href"
                    | "src"
                    | "data"
                    | "poster"
                    | "background"
                    | "to"
                    | "from"
                    | "values"
                    | "cite"
                    | "usemap"
            );
            for scheme in ["javascript:", "vbscript:", "data:text/html", "file:"] {
                if carries_url && compact.starts_with(scheme) {
                    return Some(format!("{key}=\"{}\"", attr.value));
                }
            }
            let is_link = key == "href"
                && ((name.ns == Ns::Html && matches!(local.as_str(), "a" | "area"))
                    || (name.ns == Ns::Svg && local == "a")
                    || name.ns == Ns::MathMl);
            let loads = matches!(
                key.as_str(),
                "src" | "href" | "poster" | "background" | "data"
            );
            if loads
                && !is_link
                && (compact.starts_with("http:")
                    || compact.starts_with("https:")
                    || compact.starts_with("//"))
            {
                return Some(format!("{key}=\"{}\" loads from outside", attr.value));
            }
            if key == "style"
                && (compact.contains("url(http")
                    || compact.contains("url(\"http")
                    || compact.contains("url('http")
                    || compact.contains("url(//")
                    || compact.contains("url(\"//")
                    || compact.contains("expression("))
            {
                return Some(format!("style=\"{}\"", attr.value));
            }
        }
        if local == "style" {
            let text = dom.text_content(node).to_ascii_lowercase();
            let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
            if compact.contains("url(http")
                || compact.contains("url(\"http")
                || compact.contains("url('http")
                || compact.contains("url(//")
                || compact.contains("url(\"//")
                || compact.contains("@importurl(\"http")
                || compact.contains("expression(")
            {
                return Some("a <style> that loads from outside".to_owned());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serialize::to_xhtml;
    use crate::xml::{Limits, parse};

    /// Keeps `ok/…` files, keeps web links, drops the rest.
    struct Fake;

    impl Resolver for Fake {
        fn resolve(&mut self, url: &str, usage: Usage) -> Option<String> {
            let url = url.trim();
            if usage == Usage::Link && (url.starts_with("https://") || url.starts_with("mailto:")) {
                return Some(url.to_owned());
            }
            if usage == Usage::FragmentOnly {
                return url.starts_with('#').then(|| url.to_owned());
            }
            if url.starts_with('#') && usage == Usage::Link {
                return Some(url.to_owned());
            }
            url.strip_prefix("ok/").map(|rest| format!("new/{rest}"))
        }
    }

    fn run(body: &str) -> (Dom, Report, String) {
        let source = format!(
            "<html xmlns='http://www.w3.org/1999/xhtml' xmlns:xlink='http://www.w3.org/1999/xlink'>\
             <head><title>t</title><script>alert(1)</script><base href='http://evil/'/>\
             <meta http-equiv='refresh' content='0;url=javascript:alert(1)'/><meta name='viewport' content='width=600'/>\
             <link rel='stylesheet' href='ok/a.css'/><link rel='stylesheet' href='http://evil/b.css'/>\
             <link rel='prefetch' href='http://evil/c'/><style>p {{ background: url(http://evil/p.png) }}</style></head>\
             <body onload='alert(1)'>{body}</body></html>"
        );
        let mut dom = parse(&source, &Limits::default()).unwrap();
        let report = sanitize(&mut dom, &mut Fake);
        let text = to_xhtml(&dom);
        (dom, report, text)
    }

    #[test]
    fn the_head_keeps_only_what_is_safe() {
        let (dom, report, text) = run("<p>x</p>");
        let head = dom.head().unwrap();
        let names: Vec<&str> = dom
            .element_children(head)
            .map(|child| dom.name(child).unwrap().local.as_str())
            .collect();
        assert_eq!(names, ["title", "meta", "link", "style"], "{text}");
        assert!(text.contains("href=\"new/a.css\""));
        assert!(!text.contains("evil"), "{text}");
        assert_eq!(report.scripts, 1);
        assert_eq!(report.event_handlers, 1);
        assert!(report.head_directives >= 3);
        assert_eq!(find_danger(&dom), None);
    }

    #[test]
    fn dangerous_elements_keep_their_place() {
        let (dom, report, text) = run("<p id='a'>1</p><script>alert(1)</script><p id='b'>2</p>\
             <iframe src='javascript:alert(1)'><p>never shown</p></iframe>\
             <object data='http://evil/x.swf'><img src='ok/fallback.png' alt='f'/></object>\
             <form action='http://evil/'><input formaction='http://evil/' autofocus='autofocus' onfocus='alert(1)'/></form>\
             <p id='c'>3</p>");
        let body = dom.body().unwrap();
        let children: Vec<NodeId> = dom.element_children(body).collect();
        assert_eq!(children.len(), 7, "every element keeps its place: {text}");
        assert_eq!(dom.attr(children[1], REMOVED_ATTRIBUTE), Some("script"));
        assert_eq!(dom.attr(children[1], "hidden"), Some("hidden"));
        assert!(dom.children(children[1]).is_empty());
        assert_eq!(dom.attr(children[3], REMOVED_ATTRIBUTE), Some("iframe"));
        assert_eq!(dom.attr(children[4], REMOVED_ATTRIBUTE), Some("object"));
        assert!(text.contains("src=\"new/fallback.png\""), "{text}");
        assert_eq!(dom.attr(children[6], "id"), Some("c"));
        assert_eq!(report.embedded, 2);
        assert!(report.forms >= 2);
        assert_eq!(find_danger(&dom), None, "{text}");
    }

    #[test]
    fn urls_are_rewritten_or_dropped() {
        let (dom, report, text) = run("<p style='background:url(http://evil/x.png); color: red'>\
             <a href='JaVaScRiPt:alert(1)'>js</a> <a href='https://example.com/' ping='http://evil/'>web</a>\
             <a href='#n1'>note</a> <img src='http://evil/t.png' srcset='http://evil/2x.png 2x'/>\
             <img src='ok/i.png'/><video poster='http://evil/p.png' src='ok/v.mp4'/></p>");
        assert!(!text.contains("evil"), "{text}");
        assert!(!text.to_ascii_lowercase().contains("javascript"), "{text}");
        assert!(text.contains("href=\"https://example.com/\""));
        assert!(text.contains("href=\"#n1\""));
        assert!(text.contains("src=\"new/i.png\""));
        assert!(text.contains("src=\"new/v.mp4\""));
        assert!(text.contains("color: red"));
        assert_eq!(report.external_links, 1);
        assert_eq!(find_danger(&dom), None);
    }

    #[test]
    fn svg_cannot_script_or_animate_a_link() {
        let (dom, report, text) = run(
            "<svg xmlns='http://www.w3.org/2000/svg' onload='alert(1)'><script>alert(1)</script>\
             <a xlink:href='javascript:alert(1)'><set attributeName='xlink:href' to='javascript:alert(1)'/>\
             <text>x</text></a><use xlink:href='http://evil/s.svg#a'/><use xlink:href='#local'/>\
             <rect fill='url(http://evil/g#x)' stroke='url(#grad)'/><image xlink:href='ok/p.png'/>\
             <foreignObject><div xmlns='http://www.w3.org/1999/xhtml' onclick='x()'>in</div></foreignObject></svg>",
        );
        assert!(!text.contains("evil"), "{text}");
        assert!(!text.contains("javascript"), "{text}");
        assert!(text.contains("xlink:href=\"#local\""), "{text}");
        assert!(
            text.contains("stroke=\"url(&quot;#grad&quot;)\"")
                || text.contains("stroke=\"url(#grad)\""),
            "{text}"
        );
        assert!(text.contains("xlink:href=\"new/p.png\""), "{text}");
        assert_eq!(report.scripts, 2);
        assert_eq!(report.animations, 1);
        assert_eq!(find_danger(&dom), None);
    }
}

//! From a format reader's book to the normalized book: every document made inert and cut to size,
//! every file renamed into one layout, the navigation pointed at the parts, and the package written.
//!
//! The result depends only on the input bytes and [`NORMALIZER_VERSION`]: maps are only looked up,
//! never iterated into the output, and nothing reads a clock.

use std::collections::HashMap;

use crate::NORMALIZER_VERSION;
use crate::cfi::{self, Cfi, Point, Step};
use crate::dom::{Dom, NodeId};
use crate::formats::{self, Kind};
use crate::package::{
    self, Document, Manifest, NavPoint, NormalizedBook, PartInfo, Resource, Source, SourceFormat,
    Stats,
};
use crate::sanitize::{self, Report, Resolver, Usage};
use crate::serialize;
use crate::source::{Limits, NormalizeError, Result, SourceBook, SourceNav};
use crate::split;
use crate::url::{self, Reference};

/// The prefix of a link written before the parts exist, replaced once they do.
const LINK_PREFIX: &str = "musaeum-link:";

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub part_budget: usize,
    pub limits: Limits,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            part_budget: split::PART_BUDGET,
            limits: Limits::default(),
        }
    }
}

/// Normalizes a book.
pub fn normalize(format: SourceFormat, bytes: &[u8], options: &Options) -> Result<NormalizedBook> {
    let source = match format {
        SourceFormat::Epub => formats::epub::read(bytes, &options.limits)?,
        SourceFormat::Mobi | SourceFormat::Azw3 => formats::mobi::read(bytes, &options.limits)?,
        SourceFormat::Fb2 => formats::fb2::read(bytes, &options.limits)?,
        SourceFormat::Txt => formats::txt::read(bytes, &options.limits)?,
        SourceFormat::Markdown => formats::markdown::read(bytes, &options.limits)?,
    };
    build(source, options)
}

struct Kept {
    href: String,
    kind: Kind,
    media_type: String,
    source: usize,
}

/// Lookups by source path: exact, then ignoring case.
struct Paths {
    exact: HashMap<String, usize>,
    lower: HashMap<String, usize>,
}

impl Paths {
    fn new<'a>(paths: impl Iterator<Item = &'a str>) -> Self {
        let mut exact = HashMap::new();
        let mut lower = HashMap::new();
        for (index, path) in paths.enumerate() {
            exact.entry(path.to_owned()).or_insert(index);
            lower.entry(path.to_lowercase()).or_insert(index);
        }
        Self { exact, lower }
    }

    fn get(&self, path: &str) -> Option<usize> {
        self.exact
            .get(path)
            .or_else(|| self.lower.get(&path.to_lowercase()))
            .copied()
    }
}

/// A `data:` URL that is an image, or a font when `fonts` is set: inline, and nothing that runs.
fn safe_data_url(url: &str, fonts: bool) -> bool {
    let lower: String = url
        .chars()
        .take(64)
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    let images = [
        "data:image/png",
        "data:image/jpeg",
        "data:image/jpg",
        "data:image/gif",
        "data:image/webp",
        "data:image/avif",
        "data:image/bmp",
    ];
    let font_types = [
        "data:font/",
        "data:application/font-woff",
        "data:application/x-font-",
        "data:application/vnd.ms-opentype",
    ];
    images.iter().any(|prefix| lower.starts_with(prefix))
        || (fonts && font_types.iter().any(|prefix| lower.starts_with(prefix)))
}

/// Resolves references for one file of the source, to be written from `from` in the output.
struct BookResolver<'a> {
    base: &'a str,
    from: &'a str,
    document: Option<usize>,
    documents: &'a Paths,
    resources: &'a Paths,
    kept: &'a [Kept],
    links: Option<&'a mut Vec<(usize, Option<String>)>>,
}

impl BookResolver<'_> {
    fn resource(&self, path: &str, usage: Usage) -> Option<String> {
        let index = self.resources.get(path)?;
        let kept = &self.kept[index];
        let fits = match usage {
            Usage::Image => kept.kind == Kind::Image,
            Usage::Media => matches!(kept.kind, Kind::Media | Kind::Image),
            Usage::Stylesheet => kept.kind == Kind::Style,
            Usage::CssResource => matches!(kept.kind, Kind::Image | Kind::Font),
            Usage::Link => true,
            Usage::FragmentOnly => false,
        };
        fits.then(|| url::relative(self.from, &kept.href))
    }

    fn link(&mut self, document: usize, fragment: Option<String>) -> Option<String> {
        let links = self.links.as_mut()?;
        links.push((document, fragment));
        Some(format!("{LINK_PREFIX}{}", links.len() - 1))
    }
}

impl Resolver for BookResolver<'_> {
    fn resolve(&mut self, href: &str, usage: Usage) -> Option<String> {
        match url::resolve(self.base, href) {
            Reference::Empty => None,
            Reference::Fragment(fragment) => match usage {
                Usage::Link => match self.document {
                    Some(document) => self.link(document, Some(fragment)),
                    None => Some(format!("#{}", url::encode_fragment(&fragment))),
                },
                Usage::FragmentOnly | Usage::CssResource | Usage::Image => {
                    Some(format!("#{}", url::encode_fragment(&fragment)))
                }
                _ => None,
            },
            Reference::External { scheme, url } => match usage {
                Usage::Link if matches!(scheme.as_str(), "http" | "https" | "mailto" | "tel") => {
                    Some(url)
                }
                Usage::Image if scheme == "data" && safe_data_url(&url, false) => Some(url),
                Usage::CssResource if scheme == "data" && safe_data_url(&url, true) => Some(url),
                _ => None,
            },
            Reference::Internal { path, fragment } => {
                if usage == Usage::Link {
                    if let Some(document) = self.documents.get(&path) {
                        return self.link(document, fragment);
                    }
                    return self.resource(&path, usage);
                }
                if let Some(found) = self.resource(&path, usage) {
                    return Some(found);
                }
                // An SVG `<use>` of another file, or an image that is a document: nothing to load.
                None
            }
        }
    }
}

fn part_href(document: usize, part: usize, parts: usize, svg: bool) -> String {
    let extension = if svg { "svg" } else { "xhtml" };
    if parts <= 1 || part == 0 {
        format!("text/{document:04}.{extension}")
    } else {
        format!("text/{document:04}-{part}.{extension}")
    }
}

/// The text under `<body>` (or the root), in UTF-16 code units.
fn body_text_length(dom: &Dom) -> u64 {
    let Some(root) = dom.body().or_else(|| dom.root_element()) else {
        return 0;
    };
    dom.descendants(root)
        .filter_map(|node| dom.text(node))
        .map(|text| u64::from(cfi::utf16_len(text)))
        .sum()
}

/// One document after sanitizing and cutting.
struct Prepared {
    logical: Dom,
    parts: Vec<split::Part>,
    hrefs: Vec<String>,
    ids: HashMap<String, usize>,
}

/// Builds the normalized book from what a format reader read.
pub fn build(source: SourceBook, options: &Options) -> Result<NormalizedBook> {
    let mut stats = Stats::default();
    let mut warnings = source.warnings.clone();
    stats.dropped_files = source.dropped_files;

    // Files, renamed by kind and order.
    let mut kept: Vec<Kept> = Vec::new();
    let mut counters: HashMap<&'static str, usize> = HashMap::new();
    let mut kept_paths: Vec<&str> = Vec::new();
    for (index, resource) in source.resources.iter().enumerate() {
        if kept_paths
            .iter()
            .any(|path| path.eq_ignore_ascii_case(&resource.path))
        {
            continue;
        }
        match formats::classify(&resource.media_type, &resource.path, &resource.data) {
            None => stats.dropped_files += 1,
            Some((kind, media_type, extension)) => {
                let directory = match kind {
                    Kind::Style => "styles",
                    Kind::Image => "images",
                    Kind::Font => "fonts",
                    Kind::Media => "media",
                };
                let counter = counters.entry(directory).or_insert(0);
                let href = format!("{directory}/{:04}.{extension}", *counter);
                *counter += 1;
                kept.push(Kept {
                    href,
                    kind,
                    media_type,
                    source: index,
                });
                kept_paths.push(&resource.path);
            }
        }
    }
    let resources = Paths::new(
        kept.iter()
            .map(|k| source.resources[k.source].path.as_str()),
    );
    let documents = Paths::new(source.documents.iter().map(|d| d.path.as_str()));

    // Stylesheets and SVG images are made inert like documents.
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut manifest_resources: Vec<Resource> = Vec::new();
    for kept_file in &kept {
        let original = &source.resources[kept_file.source];
        let mut resolver = BookResolver {
            base: &original.path,
            from: &kept_file.href,
            document: None,
            documents: &documents,
            resources: &resources,
            kept: &kept,
            links: None,
        };
        let bytes = if kept_file.kind == Kind::Style {
            let text = crate::encoding::decode_xml(&original.data).text;
            let (clean, report) = sanitize::sanitize_stylesheet_file(&text, &mut resolver);
            stats.sanitized.add(&report);
            clean.into_bytes()
        } else if kept_file.media_type == "image/svg+xml" {
            let text = crate::encoding::decode_xml(&original.data).text;
            match crate::xml::parse(&text, &crate::xml::Limits::default()) {
                Ok(mut dom) => {
                    let report = sanitize::sanitize(&mut dom, &mut resolver);
                    stats.sanitized.add(&report);
                    serialize::to_xml(&dom, serialize::XML_DECLARATION).into_bytes()
                }
                Err(error) => {
                    warnings.push(format!("{}: {error}; left out", original.path));
                    stats.dropped_files += 1;
                    continue;
                }
            }
        } else {
            original.data.clone()
        };
        stats.resources += 1;
        stats.resource_bytes += bytes.len() as u64;
        manifest_resources.push(Resource {
            href: kept_file.href.clone(),
            media_type: kept_file.media_type.clone(),
            bytes: bytes.len() as u64,
            source: Some(original.path.clone()),
        });
        files.push((kept_file.href.clone(), bytes));
    }

    // Documents: inert, then cut, with links waiting for the parts to exist.
    let mut links: Vec<(usize, Option<String>)> = Vec::new();
    let mut prepared: Vec<Prepared> = Vec::with_capacity(source.documents.len());
    for (index, document) in source.documents.iter().enumerate() {
        let mut dom = document.dom.clone();
        let from = part_href(index, 0, 1, document.svg);
        let report: Report = {
            let mut resolver = BookResolver {
                base: &document.path,
                from: &from,
                document: Some(index),
                documents: &documents,
                resources: &resources,
                kept: &kept,
                links: Some(&mut links),
            };
            sanitize::sanitize(&mut dom, &mut resolver)
        };
        stats.sanitized.add(&report);
        if document.parsed == crate::source::Parsed::Html {
            stats.html_fallbacks += 1;
        }
        let parts = if document.svg || source.fixed_layout {
            vec![split::Part {
                dom: dom.clone(),
                start: Vec::new(),
            }]
        } else {
            split::split(&dom, options.part_budget)
        };
        let whole = serialize::xhtml_len(&dom) as u64;
        stats.largest_document = stats.largest_document.max(whole);
        if parts.len() > 1 {
            stats.split_documents += 1;
        }
        let hrefs: Vec<String> = (0..parts.len())
            .map(|part| part_href(index, part, parts.len(), document.svg))
            .collect();
        let mut ids: HashMap<String, usize> = HashMap::new();
        for (number, part) in parts.iter().enumerate() {
            for node in part.dom.descendants(Dom::DOCUMENT) {
                if let Some(id) = part.dom.attr(node, "id") {
                    ids.entry(id.to_owned()).or_insert(number);
                }
            }
        }
        prepared.push(Prepared {
            logical: dom,
            parts,
            hrefs,
            ids,
        });
    }

    // Links now point at the part that holds their target.
    let targets: Vec<(String, Option<String>)> = links
        .iter()
        .map(|(document, fragment)| {
            let target = &prepared[*document];
            let part = fragment
                .as_ref()
                .and_then(|fragment| target.ids.get(fragment))
                .copied()
                .unwrap_or(0);
            (target.hrefs[part].clone(), fragment.clone())
        })
        .collect();
    let mut manifest_documents: Vec<Document> = Vec::with_capacity(prepared.len());
    for (index, document) in prepared.iter_mut().enumerate() {
        let source_document = &source.documents[index];
        let mut part_infos = Vec::with_capacity(document.parts.len());
        let mut text_start = 0u64;
        let part_count = document.parts.len();
        for (number, part) in document.parts.iter_mut().enumerate() {
            let href = document.hrefs[number].clone();
            let nodes: Vec<NodeId> = part.dom.descendants(Dom::DOCUMENT).collect();
            for node in nodes {
                let Some(attrs) = part.dom.attrs_mut(node) else {
                    continue;
                };
                for attr in attrs.iter_mut() {
                    if let Some(number) = attr.value.strip_prefix(LINK_PREFIX)
                        && let Some((target, fragment)) =
                            number.parse::<usize>().ok().and_then(|n| targets.get(n))
                    {
                        let mut value = if *target == href {
                            String::new()
                        } else {
                            url::relative(&href, target)
                        };
                        if let Some(fragment) = fragment {
                            value.push('#');
                            value.push_str(&url::encode_fragment(fragment));
                        }
                        if value.is_empty() {
                            value = url::relative(&href, target);
                        }
                        attr.value = value;
                    }
                }
            }
            let text = if source_document.svg {
                serialize::to_xml(&part.dom, serialize::XML_DECLARATION)
            } else {
                serialize::to_xhtml(&part.dom)
            };
            let bytes = text.len() as u64;
            if bytes > options.part_budget as u64 && part_count > 1 {
                stats.oversize_parts += 1;
            }
            stats.largest_part = stats.largest_part.max(bytes);
            let text_length = body_text_length(&part.dom);
            part_infos.push(PartInfo {
                href: href.clone(),
                start: part.start.clone(),
                bytes,
                text_start,
                text_length,
            });
            text_start += text_length;
            files.push((href, text.into_bytes()));
        }
        let text_length = body_text_length(&document.logical);
        if text_length != text_start {
            warnings.push(format!(
                "{}: the parts hold {text_start} units of text, the document {text_length}",
                source_document.path
            ));
        }
        let body_step = document.logical.body().and_then(|body| {
            let root = document.logical.root_element()?;
            Some(cfi::step_of(&document.logical, root, body).index)
        });
        let title = document
            .logical
            .head()
            .and_then(|head| document.logical.child(head, &crate::dom::Ns::Html, "title"))
            .map(|title| crate::source::fold_space(&document.logical.text_content(title)))
            .filter(|title| !title.is_empty());
        manifest_documents.push(Document {
            href: source_document.path.clone(),
            id: source_document.id.clone(),
            media_type: if source_document.svg {
                "image/svg+xml"
            } else {
                "application/xhtml+xml"
            }
            .to_owned(),
            linear: source_document.linear,
            properties: source_document.properties.clone(),
            title,
            parsed: source_document.parsed.as_str().to_owned(),
            body_step,
            text_length,
            parts: part_infos,
        });
        stats.documents += 1;
        stats.parts += document.parts.len() as u32;
    }

    // Navigation, pointed at the parts, with the CFI of each target.
    let navigation = |entries: &[SourceNav]| -> Vec<NavPoint> {
        fn convert(
            entries: &[SourceNav],
            documents: &Paths,
            prepared: &[Prepared],
            manifest_documents: &[Document],
            kept: &[Kept],
            resources: &Paths,
            depth: usize,
        ) -> Vec<NavPoint> {
            if depth > 64 {
                return Vec::new();
            }
            entries
                .iter()
                .map(|entry| {
                    let mut point = NavPoint {
                        label: if entry.label.is_empty() {
                            "\u{2026}".to_owned()
                        } else {
                            entry.label.clone()
                        },
                        kind: entry.kind.clone(),
                        children: convert(
                            &entry.children,
                            documents,
                            prepared,
                            manifest_documents,
                            kept,
                            resources,
                            depth + 1,
                        ),
                        ..NavPoint::default()
                    };
                    if let Some((path, fragment)) = &entry.target {
                        if let Some(index) = documents.get(path) {
                            let document = &prepared[index];
                            let found = fragment
                                .as_ref()
                                .and_then(|f| cfi::element_by_id(&document.logical, f));
                            let part = fragment
                                .as_ref()
                                .and_then(|f| document.ids.get(f))
                                .copied()
                                .unwrap_or(0);
                            let mut href = document.hrefs[part].clone();
                            if let (Some(fragment), Some(_)) = (fragment, found) {
                                href.push('#');
                                href.push_str(&url::encode_fragment(fragment));
                            }
                            point.href = Some(href);
                            let node = found
                                .or_else(|| document.logical.body())
                                .or_else(|| document.logical.root_element());
                            if let Some(node) = node {
                                let (path, offset) =
                                    cfi::point_to_path(&document.logical, Point::Element(node));
                                point.cfi = Some(
                                    Cfi {
                                        package: vec![
                                            Step::new(6),
                                            Step {
                                                index: 2 * (index as u32 + 1),
                                                id: manifest_documents[index].id.clone(),
                                            },
                                        ],
                                        path,
                                        offset,
                                    }
                                    .to_string(),
                                );
                            }
                        } else if let Some(resource) = resources.get(path) {
                            point.href = Some(kept[resource].href.clone());
                        }
                    }
                    point
                })
                .collect()
        }
        convert(
            entries,
            &documents,
            &prepared,
            &manifest_documents,
            &kept,
            &resources,
            0,
        )
    };
    let toc = navigation(&source.toc);
    let page_list = navigation(&source.page_list);
    let landmarks = navigation(&source.landmarks);
    // A document without a <title> takes its name from the first entry that leads to its start.
    fn first_labels(points: &[NavPoint], into: &mut HashMap<String, String>) {
        for point in points {
            if let Some(href) = &point.href {
                into.entry(href.split('#').next().unwrap_or("").to_owned())
                    .or_insert_with(|| point.label.clone());
            }
            first_labels(&point.children, into);
        }
    }
    let mut labels = HashMap::new();
    first_labels(&toc, &mut labels);
    for document in &mut manifest_documents {
        if let Some(label) = document
            .parts
            .first()
            .and_then(|part| labels.get(&part.href))
        {
            document.title = Some(label.clone());
        }
    }

    let mut metadata = source.metadata.clone();
    metadata.cover = source
        .cover
        .as_deref()
        .and_then(|path| resources.get(path))
        .filter(|&index| kept[index].kind == Kind::Image)
        .map(|index| kept[index].href.clone());

    let manifest = Manifest {
        normalizer: NORMALIZER_VERSION,
        source: Source {
            format: source.format,
            version: source.version.clone(),
            fixed_layout: source.fixed_layout,
            direction: source.direction.clone(),
        },
        metadata,
        documents: manifest_documents,
        resources: manifest_resources,
        toc,
        page_list,
        landmarks,
        stats,
        warnings,
    };
    if manifest.documents.is_empty() {
        return Err(NormalizeError::Malformed(
            "the book has no document to read".to_owned(),
        ));
    }
    files.push((
        package::NAV_PATH.to_owned(),
        package::nav_xhtml(&manifest).into_bytes(),
    ));
    files.push((
        package::PACKAGE_PATH.to_owned(),
        package::package_opf(&manifest).into_bytes(),
    ));
    files.push((
        package::CONTAINER_PATH.to_owned(),
        package::container_xml().into_bytes(),
    ));
    files.push((
        package::MANIFEST_PATH.to_owned(),
        serde_json::to_vec(&manifest)
            .map_err(|error| NormalizeError::Malformed(error.to_string()))?,
    ));
    files.sort_by(|(a, _), (b, _)| a.cmp(b));
    files.dedup_by(|(a, _), (b, _)| a == b);
    Ok(NormalizedBook { manifest, files })
}

//! EPUB 2 and 3: the container, the package document, the navigation, and every document of the
//! spine in its order, so that a CFI's spine step means the same document it means in the source.

use std::collections::HashMap;

use super::zip::Archive;
use super::{Markup, decode_markup, empty_page, parse_markup};
use crate::dom::{Dom, NodeId, Ns};
use crate::package::{Contributor, Identifier, Series, SourceFormat, Title};
use crate::source::{
    Limits, NormalizeError, Parsed, Result, SourceBook, SourceDocument, SourceNav, SourceResource,
    clean_field, fold_space,
};
use crate::url::{self, Reference};
use crate::xml;

const OPF_NS: &str = "http://www.idpf.org/2007/opf";
const DC_NS: &str = "http://purl.org/dc/elements/1.1/";
const NCX_NS: &str = "http://www.daisy.org/z3986/2005/ncx/";
const IDPF_OBFUSCATION: &str = "http://www.idpf.org/2008/embedding";
const ADOBE_OBFUSCATION: &str = "http://ns.adobe.com/pdf/enc#RC";
const MAX_FIELD: usize = 4096;
const MAX_DESCRIPTION: usize = 20_000;

#[derive(Debug)]
struct Item {
    id: String,
    path: String,
    media_type: String,
    properties: Vec<String>,
    fallback: Option<String>,
}

#[derive(Debug)]
struct ItemRef {
    idref: String,
    id: Option<String>,
    linear: bool,
    properties: Vec<String>,
}

fn parse_xml_file(archive: &Archive<'_>, path: &str) -> Result<Dom> {
    let bytes = archive.read(path)?;
    let text = crate::encoding::decode_xml(&bytes).text;
    xml::parse(&text, &xml::Limits::default())
        .map_err(|error| NormalizeError::Malformed(format!("{path}: {error}")))
}

/// Elements anywhere under `node` with this local name, in one of `namespaces` (or any if empty).
fn elements<'d>(dom: &'d Dom, node: NodeId, local: &'d str) -> impl Iterator<Item = NodeId> + 'd {
    dom.descendants(node)
        .filter(move |&n| dom.name(n).is_some_and(|name| name.local == local))
}

/// An attribute by local name, whatever its namespace, which is how EPUB 2's `opf:` attributes and
/// their unprefixed cousins both read.
fn any_attr<'d>(dom: &'d Dom, node: NodeId, local: &str) -> Option<&'d str> {
    dom.attrs(node)
        .iter()
        .find(|attr| attr.name.local == local)
        .map(|attr| attr.value.as_str())
}

fn text_of(dom: &Dom, node: NodeId) -> String {
    fold_space(&dom.text_content(node))
}

/// The package document's path, from `META-INF/container.xml`.
fn package_path(archive: &Archive<'_>) -> Result<String> {
    let container_path = archive
        .find("META-INF/container.xml")
        .ok_or(NormalizeError::NotThisFormat("EPUB"))?
        .to_owned();
    let dom = parse_xml_file(archive, &container_path)?;
    let rootfiles: Vec<NodeId> = elements(&dom, Dom::DOCUMENT, "rootfile").collect();
    let chosen = rootfiles
        .iter()
        .find(|&&node| dom.attr(node, "media-type") == Some("application/oebps-package+xml"))
        .or(rootfiles.first())
        .and_then(|&node| dom.attr(node, "full-path"))
        .ok_or_else(|| NormalizeError::Malformed("the container names no package".to_owned()))?;
    Ok(url::normalize_path(&url::percent_decode(chosen)))
}

/// The files `META-INF/encryption.xml` lists, with their algorithms. A file encrypted with anything
/// other than font obfuscation means DRM.
fn encryption(archive: &Archive<'_>) -> Result<HashMap<String, String>> {
    let mut files = HashMap::new();
    let Some(path) = archive.find("META-INF/encryption.xml").map(str::to_owned) else {
        return Ok(files);
    };
    let dom = parse_xml_file(archive, &path)?;
    for data in elements(&dom, Dom::DOCUMENT, "EncryptedData") {
        let algorithm = elements(&dom, data, "EncryptionMethod")
            .next()
            .and_then(|method| dom.attr(method, "Algorithm"))
            .unwrap_or("")
            .to_owned();
        let Some(uri) = elements(&dom, data, "CipherReference")
            .next()
            .and_then(|reference| dom.attr(reference, "URI"))
        else {
            continue;
        };
        if algorithm != IDPF_OBFUSCATION && algorithm != ADOBE_OBFUSCATION {
            return Err(NormalizeError::Drm);
        }
        files.insert(url::normalize_path(&url::percent_decode(uri)), algorithm);
    }
    Ok(files)
}

/// Undoes font obfuscation: an XOR of the first bytes with a key from the book's identifier.
fn deobfuscate(
    data: &mut [u8],
    algorithm: &str,
    unique_identifier: &str,
    identifiers: &[Identifier],
) {
    if algorithm == IDPF_OBFUSCATION {
        let cleaned: String = unique_identifier
            .chars()
            .filter(|c| !matches!(c, ' ' | '\t' | '\r' | '\n'))
            .collect();
        let key = sha1_smol::Sha1::from(cleaned.as_bytes()).digest().bytes();
        for (index, byte) in data.iter_mut().take(1040).enumerate() {
            *byte ^= key[index % key.len()];
        }
    } else if algorithm == ADOBE_OBFUSCATION {
        let uuid = identifiers
            .iter()
            .map(|identifier| identifier.value.trim())
            .find(|value| value.to_ascii_lowercase().starts_with("urn:uuid:"))
            .unwrap_or(unique_identifier);
        let hex: String = uuid
            .trim_start_matches("urn:uuid:")
            .trim_start_matches("URN:UUID:")
            .chars()
            .filter(char::is_ascii_hexdigit)
            .collect();
        if hex.len() != 32 {
            return;
        }
        let key: Vec<u8> = (0..16)
            .filter_map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok())
            .collect();
        for (index, byte) in data.iter_mut().take(1024).enumerate() {
            *byte ^= key[index % key.len()];
        }
    }
}

/// What `<metadata>` says, with EPUB 3's `refines` folded into the entries they refine.
fn read_metadata(
    dom: &Dom,
    metadata: NodeId,
    book: &mut SourceBook,
    unique_id: Option<&str>,
    items: &[Item],
) {
    let mut refinements: HashMap<String, Vec<(String, String, Option<String>)>> = HashMap::new();
    let mut collections: Vec<(Option<String>, String)> = Vec::new();
    for meta in dom
        .element_children(metadata)
        .filter(|&n| dom.name(n).is_some_and(|name| name.local == "meta"))
    {
        if let (Some(refines), Some(property)) =
            (dom.attr(meta, "refines"), dom.attr(meta, "property"))
        {
            refinements
                .entry(refines.trim_start_matches('#').to_owned())
                .or_default()
                .push((
                    property.to_owned(),
                    text_of(dom, meta),
                    dom.attr(meta, "scheme").map(str::to_owned),
                ));
        }
    }
    let refined = |id: Option<&str>, property: &str| -> Option<String> {
        refinements
            .get(id?)?
            .iter()
            .find(|(p, _, _)| p == property)
            .map(|(_, value, _)| value.clone())
    };
    let m = &mut book.metadata;
    let mut calibre_series: Option<String> = None;
    let mut calibre_index: Option<String> = None;
    for node in dom.element_children(metadata) {
        let Some(name) = dom.name(node) else { continue };
        let id = dom.attr(node, "id");
        let value = text_of(dom, node);
        let is_dc = name.ns == Ns::from_uri(DC_NS) || name.prefix.as_deref() == Some("dc");
        if is_dc {
            match name.local.as_str() {
                "title" => {
                    if let Some(value) = clean_field(&value, MAX_FIELD) {
                        m.titles.push(Title {
                            value,
                            kind: refined(id, "title-type"),
                            file_as: refined(id, "file-as")
                                .or_else(|| any_attr(dom, node, "file-as").map(str::to_owned)),
                        });
                    }
                }
                "creator" | "contributor" => {
                    let Some(value) = clean_field(&value, MAX_FIELD) else {
                        continue;
                    };
                    let mut roles: Vec<String> = refinements
                        .get(id.unwrap_or(""))
                        .map(|list| {
                            list.iter()
                                .filter(|(p, _, _)| p == "role")
                                .map(|(_, v, _)| v.clone())
                                .collect()
                        })
                        .unwrap_or_default();
                    if let Some(role) = any_attr(dom, node, "role") {
                        roles.push(role.to_owned());
                    }
                    let contributor = Contributor {
                        name: value,
                        roles,
                        file_as: refined(id, "file-as")
                            .or_else(|| any_attr(dom, node, "file-as").map(str::to_owned)),
                    };
                    if name.local == "creator" {
                        m.creators.push(contributor);
                    } else {
                        m.contributors.push(contributor);
                    }
                }
                "language" => m.languages.extend(clean_field(&value, 64)),
                "identifier" => {
                    let Some(value) = clean_field(&value, MAX_FIELD) else {
                        continue;
                    };
                    let lower = value.to_ascii_lowercase();
                    let scheme = any_attr(dom, node, "scheme")
                        .map(str::to_owned)
                        .or_else(|| {
                            refined(id, "identifier-type").map(|code| match code.as_str() {
                                "15" | "02" => "ISBN".to_owned(),
                                "06" => "DOI".to_owned(),
                                other => other.to_owned(),
                            })
                        })
                        .or_else(|| {
                            if lower.starts_with("urn:isbn:") || lower.starts_with("isbn:") {
                                Some("ISBN".to_owned())
                            } else if lower.starts_with("urn:uuid:") {
                                Some("UUID".to_owned())
                            } else if lower.starts_with("doi:") || lower.starts_with("urn:doi:") {
                                Some("DOI".to_owned())
                            } else {
                                None
                            }
                        });
                    if id.is_some() && id == unique_id {
                        m.unique_identifier = Some(value.clone());
                    }
                    m.identifiers.push(Identifier { value, scheme });
                }
                "publisher" => {
                    m.publisher = m
                        .publisher
                        .take()
                        .or_else(|| clean_field(&value, MAX_FIELD))
                }
                "date" => {
                    let event = any_attr(dom, node, "event").unwrap_or("");
                    if event == "modification" {
                        m.modified = m.modified.take().or_else(|| clean_field(&value, 64));
                    } else if m.published.is_none() {
                        m.published = clean_field(&value, 64);
                    }
                }
                "description" => {
                    let raw = dom.text_content(node);
                    let plain = if raw.contains('<') {
                        crate::html::parse(&raw, 100_000)
                            .and_then(|parsed| {
                                parsed.dom.body().map(|body| parsed.dom.text_content(body))
                            })
                            .unwrap_or(raw)
                    } else {
                        raw
                    };
                    m.description = m
                        .description
                        .take()
                        .or_else(|| clean_field(&plain, MAX_DESCRIPTION));
                }
                "subject" => m.subjects.extend(clean_field(&value, MAX_FIELD)),
                "rights" => m.rights = m.rights.take().or_else(|| clean_field(&value, MAX_FIELD)),
                _ => {}
            }
            continue;
        }
        if name.local != "meta" {
            continue;
        }
        match (dom.attr(node, "property"), dom.attr(node, "name")) {
            (Some("dcterms:modified"), _) if dom.attr(node, "refines").is_none() => {
                m.modified = clean_field(&value, 64);
            }
            (Some("rendition:layout"), _) if dom.attr(node, "refines").is_none() => {
                book.fixed_layout = value == "pre-paginated";
            }
            (Some("belongs-to-collection"), _) if dom.attr(node, "refines").is_none() => {
                collections.push((id.map(str::to_owned), value));
            }
            (_, Some("cover")) => {
                if let Some(content) = dom.attr(node, "content") {
                    // An id in the manifest, or now and then a path.
                    let by_id = items.iter().find(|item| item.id == content);
                    book.cover = by_id.map(|item| item.path.clone());
                }
            }
            (_, Some("calibre:series")) => {
                calibre_series = dom
                    .attr(node, "content")
                    .and_then(|c| clean_field(c, MAX_FIELD))
            }
            (_, Some("calibre:series_index")) => {
                calibre_index = dom.attr(node, "content").and_then(|c| clean_field(c, 32))
            }
            _ => {}
        }
    }
    let m = &mut book.metadata;
    for (id, name) in collections {
        let kind = refined(id.as_deref(), "collection-type");
        if kind.as_deref().is_none_or(|kind| kind == "series") && m.series.is_none() {
            m.series = clean_field(&name, MAX_FIELD).map(|name| Series {
                name,
                position: refined(id.as_deref(), "group-position"),
            });
        }
    }
    if m.series.is_none()
        && let Some(name) = calibre_series
    {
        m.series = Some(Series {
            name,
            position: calibre_index,
        });
    }
}

/// The navigation of an EPUB 3 nav document: `toc`, `page-list` and `landmarks`.
fn read_nav(dom: &Dom, path: &str) -> (Vec<SourceNav>, Vec<SourceNav>, Vec<SourceNav>) {
    let mut toc = Vec::new();
    let mut page_list = Vec::new();
    let mut landmarks = Vec::new();
    for nav in elements(dom, Dom::DOCUMENT, "nav") {
        let kinds = dom
            .attr_ns(nav, &Ns::Ops, "type")
            .or_else(|| any_attr(dom, nav, "type"))
            .unwrap_or("");
        let Some(list) = elements(dom, nav, "ol")
            .next()
            .or_else(|| elements(dom, nav, "ul").next())
        else {
            continue;
        };
        let entries = read_nav_list(dom, list, path, 0);
        for kind in kinds.split_ascii_whitespace() {
            match kind {
                "toc" if toc.is_empty() => toc = entries.clone(),
                "page-list" if page_list.is_empty() => page_list = entries.clone(),
                "landmarks" if landmarks.is_empty() => landmarks = entries.clone(),
                _ => {}
            }
        }
    }
    (toc, page_list, landmarks)
}

fn read_nav_list(dom: &Dom, list: NodeId, base: &str, depth: usize) -> Vec<SourceNav> {
    let mut entries = Vec::new();
    if depth > 64 {
        return entries;
    }
    for item in dom
        .element_children(list)
        .filter(|&n| dom.name(n).is_some_and(|name| name.local == "li"))
    {
        let mut entry = SourceNav::default();
        for child in dom.element_children(item) {
            let local = dom
                .name(child)
                .map(|name| name.local.as_str())
                .unwrap_or("");
            match local {
                "a" | "span" if entry.label.is_empty() => {
                    entry.label = fold_space(&dom.text_content(child));
                    if entry.label.is_empty() {
                        entry.label = any_attr(dom, child, "title")
                            .map(fold_space)
                            .unwrap_or_default();
                    }
                    if let Some(href) = dom.attr(child, "href")
                        && let Reference::Internal { path, fragment } = url::resolve(base, href)
                    {
                        entry.target = Some((path, fragment));
                    }
                    entry.kind = dom.attr_ns(child, &Ns::Ops, "type").map(str::to_owned);
                }
                "ol" | "ul" => entry.children = read_nav_list(dom, child, base, depth + 1),
                _ => {}
            }
        }
        if !entry.label.is_empty() || !entry.children.is_empty() {
            entries.push(entry);
        }
    }
    entries
}

/// The navigation of an EPUB 2 NCX: `navMap` and `pageList`.
fn read_ncx(dom: &Dom, path: &str) -> (Vec<SourceNav>, Vec<SourceNav>) {
    fn points(dom: &Dom, parent: NodeId, path: &str, local: &str, depth: usize) -> Vec<SourceNav> {
        if depth > 64 {
            return Vec::new();
        }
        dom.element_children(parent)
            .filter(|&n| dom.name(n).is_some_and(|name| name.local == local))
            .map(|point| {
                let label = elements(dom, point, "navLabel")
                    .next()
                    .and_then(|label| elements(dom, label, "text").next())
                    .map(|text| fold_space(&dom.text_content(text)))
                    .unwrap_or_default();
                let target = dom
                    .element_children(point)
                    .find(|&n| dom.name(n).is_some_and(|name| name.local == "content"))
                    .and_then(|content| dom.attr(content, "src"))
                    .and_then(|src| match url::resolve(path, src) {
                        Reference::Internal { path, fragment } => Some((path, fragment)),
                        _ => None,
                    });
                SourceNav {
                    label,
                    target,
                    kind: None,
                    children: points(dom, point, path, local, depth + 1),
                }
            })
            .collect()
    }
    let toc = elements(dom, Dom::DOCUMENT, "navMap")
        .next()
        .map(|map| points(dom, map, path, "navPoint", 0))
        .unwrap_or_default();
    let pages = elements(dom, Dom::DOCUMENT, "pageList")
        .next()
        .map(|list| points(dom, list, path, "pageTarget", 0))
        .unwrap_or_default();
    (toc, pages)
}

fn is_document_type(media_type: &str) -> bool {
    matches!(
        media_type,
        "application/xhtml+xml"
            | "text/html"
            | "application/x-dtbook+xml"
            | "text/x-oeb1-document"
            | "image/svg+xml"
    )
}

/// Reads an EPUB.
pub fn read(bytes: &[u8], limits: &Limits) -> Result<SourceBook> {
    let archive = Archive::open(bytes, *limits)?;
    let package_path = package_path(&archive)?;
    let encrypted = encryption(&archive)?;
    let real_package = archive
        .find(&package_path)
        .ok_or_else(|| {
            NormalizeError::Malformed(format!("the package {package_path} is not there"))
        })?
        .to_owned();
    let package = parse_xml_file(&archive, &real_package)?;
    let root = package
        .root_element()
        .filter(|&root| {
            package
                .name(root)
                .is_some_and(|name| name.local == "package")
        })
        .ok_or_else(|| {
            NormalizeError::Malformed("the package document has no <package>".to_owned())
        })?;
    let _ = OPF_NS;
    let mut book = SourceBook::new(SourceFormat::Epub);
    book.version = package.attr(root, "version").map(str::to_owned);
    let unique_id = package.attr(root, "unique-identifier").map(str::to_owned);

    // The manifest, with paths from the archive's root.
    let mut items: Vec<Item> = Vec::new();
    if let Some(manifest) = elements(&package, root, "manifest").next() {
        for item in package
            .element_children(manifest)
            .filter(|&n| package.name(n).is_some_and(|name| name.local == "item"))
        {
            let (Some(id), Some(href)) = (package.attr(item, "id"), package.attr(item, "href"))
            else {
                continue;
            };
            let Reference::Internal { path, .. } = url::resolve(&package_path, href) else {
                continue;
            };
            items.push(Item {
                id: id.to_owned(),
                path,
                media_type: package
                    .attr(item, "media-type")
                    .unwrap_or("")
                    .trim()
                    .to_ascii_lowercase(),
                properties: package
                    .attr(item, "properties")
                    .unwrap_or("")
                    .split_ascii_whitespace()
                    .map(str::to_owned)
                    .collect(),
                fallback: package.attr(item, "fallback").map(str::to_owned),
            });
        }
    }
    let item_by_id: HashMap<&str, usize> = items
        .iter()
        .enumerate()
        .map(|(i, item)| (item.id.as_str(), i))
        .collect();

    if let Some(metadata) = elements(&package, root, "metadata").next() {
        read_metadata(&package, metadata, &mut book, unique_id.as_deref(), &items);
    }
    if book.cover.is_none() {
        book.cover = items
            .iter()
            .find(|item| item.properties.iter().any(|p| p == "cover-image"))
            .map(|item| item.path.clone());
    }

    // The spine.
    let mut itemrefs: Vec<ItemRef> = Vec::new();
    let mut ncx_id = None;
    if let Some(spine) = elements(&package, root, "spine").next() {
        ncx_id = package.attr(spine, "toc").map(str::to_owned);
        book.direction = package
            .attr(spine, "page-progression-direction")
            .filter(|direction| *direction == "rtl")
            .map(str::to_owned);
        for itemref in package
            .element_children(spine)
            .filter(|&n| package.name(n).is_some_and(|name| name.local == "itemref"))
        {
            itemrefs.push(ItemRef {
                idref: package.attr(itemref, "idref").unwrap_or("").to_owned(),
                id: package
                    .attr(itemref, "id")
                    .filter(|id| !id.is_empty())
                    .map(str::to_owned),
                linear: package.attr(itemref, "linear") != Some("no"),
                properties: package
                    .attr(itemref, "properties")
                    .unwrap_or("")
                    .split_ascii_whitespace()
                    .map(str::to_owned)
                    .collect(),
            });
        }
    }

    // Landmarks from EPUB 2's guide, replaced by the nav document's if it has them.
    if let Some(guide) = elements(&package, root, "guide").next() {
        for reference in package.element_children(guide) {
            let Some(href) = package.attr(reference, "href") else {
                continue;
            };
            if let Reference::Internal { path, fragment } = url::resolve(&package_path, href) {
                book.landmarks.push(SourceNav {
                    label: package
                        .attr(reference, "title")
                        .map(fold_space)
                        .unwrap_or_default(),
                    target: Some((path, fragment)),
                    kind: package.attr(reference, "type").map(str::to_owned),
                    children: Vec::new(),
                });
            }
        }
    }

    // Navigation: EPUB 3's nav document, or the NCX.
    let nav_item = items
        .iter()
        .position(|item| item.properties.iter().any(|p| p == "nav"));
    if let Some(index) = nav_item
        && let Some(real) = archive.find(&items[index].path).map(str::to_owned)
        && let Ok(bytes) = archive.read(&real)
    {
        let text = decode_markup(&bytes);
        let mut warning = None;
        if let Some((dom, _)) = parse_markup(&text, Markup::Xhtml, limits, &mut warning) {
            let (toc, pages, landmarks) = read_nav(&dom, &items[index].path);
            book.toc = toc;
            book.page_list = pages;
            if !landmarks.is_empty() {
                book.landmarks = landmarks;
            }
        }
    }
    if book.toc.is_empty() {
        let ncx = ncx_id
            .as_deref()
            .and_then(|id| item_by_id.get(id))
            .map(|&index| &items[index])
            .or_else(|| {
                items
                    .iter()
                    .find(|item| item.media_type == "application/x-dtbncx+xml")
            });
        if let Some(ncx) = ncx
            && let Some(real) = archive.find(&ncx.path).map(str::to_owned)
            && let Ok(dom) = parse_xml_file(&archive, &real)
        {
            let _ = NCX_NS;
            let (toc, pages) = read_ncx(&dom, &ncx.path);
            book.toc = toc;
            if book.page_list.is_empty() {
                book.page_list = pages;
            }
        }
    }

    // The documents, in spine order: one for every itemref, even a broken one, so that the
    // spine steps of CFIs keep meaning what they mean in the source.
    let mut used_as_document = vec![false; items.len()];
    for itemref in &itemrefs {
        // Follow fallbacks to something a reader can show.
        let mut current = item_by_id.get(itemref.idref.as_str()).copied();
        let mut hops = 0;
        while let Some(index) = current {
            if is_document_type(&items[index].media_type) || hops > 8 {
                break;
            }
            current = items[index]
                .fallback
                .as_deref()
                .and_then(|id| item_by_id.get(id))
                .copied();
            hops += 1;
        }
        let original = item_by_id.get(itemref.idref.as_str()).copied();
        let chosen = current.or(original);
        // A picture shown as a page is still a picture the page needs.
        if let Some(index) = chosen.filter(|&index| is_document_type(&items[index].media_type)) {
            used_as_document[index] = true;
        }
        let document = load_document(
            &archive,
            chosen.map(|index| &items[index]),
            original.map(|index| &items[index]),
            itemref,
            limits,
            &mut book.warnings,
        );
        book.documents.push(document);
    }
    // Documents the spine leaves out but links may still reach.
    for (index, item) in items.iter().enumerate() {
        let is_nav = item.properties.iter().any(|p| p == "nav");
        if used_as_document[index]
            || is_nav
            || !matches!(
                item.media_type.as_str(),
                "application/xhtml+xml" | "text/html"
            )
        {
            continue;
        }
        if archive.contains(&item.path) {
            used_as_document[index] = true;
            let itemref = ItemRef {
                idref: item.id.clone(),
                id: None,
                linear: false,
                properties: Vec::new(),
            };
            let document = load_document(
                &archive,
                Some(item),
                Some(item),
                &itemref,
                limits,
                &mut book.warnings,
            );
            book.documents.push(document);
        }
    }

    // Everything else in the manifest, and files the manifest forgot that documents may use.
    let unique = book.metadata.unique_identifier.clone().unwrap_or_default();
    let identifiers = book.metadata.identifiers.clone();
    let mut declared: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (index, item) in items.iter().enumerate() {
        declared.insert(item.path.to_lowercase());
        if used_as_document[index] || item.properties.iter().any(|p| p == "nav") {
            continue;
        }
        let Some(real) = archive.find(&item.path).map(str::to_owned) else {
            book.warnings.push(format!(
                "{} is in the manifest but not in the archive",
                item.path
            ));
            continue;
        };
        let mut data = match archive.read(&real) {
            Ok(data) => data,
            Err(NormalizeError::TooLarge(what)) => {
                book.warnings
                    .push(format!("{what} is too large and was left out"));
                book.dropped_files += 1;
                continue;
            }
            Err(other) => return Err(other),
        };
        if let Some(algorithm) = encrypted.get(&item.path).or_else(|| encrypted.get(&real)) {
            deobfuscate(&mut data, algorithm, &unique, &identifiers);
        }
        book.resources.push(SourceResource {
            path: item.path.clone(),
            media_type: item.media_type.clone(),
            data,
        });
    }
    let undeclared: Vec<String> = archive
        .names()
        .filter(|name| !declared.contains(&name.to_lowercase()))
        .filter(|name| {
            let lower = name.to_ascii_lowercase();
            [
                ".css", ".jpg", ".jpeg", ".png", ".gif", ".svg", ".webp", ".ttf", ".otf", ".woff",
                ".woff2",
            ]
            .iter()
            .any(|ext| lower.ends_with(ext))
        })
        .map(str::to_owned)
        .collect();
    for name in undeclared {
        if encrypted.contains_key(&name) {
            continue;
        }
        if let Ok(data) = archive.read(&name) {
            book.resources.push(SourceResource {
                path: name,
                media_type: String::new(),
                data,
            });
        }
    }
    Ok(book)
}

fn load_document(
    archive: &Archive<'_>,
    item: Option<&Item>,
    original: Option<&Item>,
    itemref: &ItemRef,
    limits: &Limits,
    warnings: &mut Vec<String>,
) -> SourceDocument {
    let path = original.or(item).map_or_else(
        || format!("missing-{}", itemref.idref),
        |item| item.path.clone(),
    );
    let placeholder = |path: &str, why: String, warnings: &mut Vec<String>| {
        warnings.push(format!("{path}: {why}"));
        let (mut dom, body) = empty_page(path, None);
        let p = super::add(&mut dom, body, "p", &[("class", "musaeum-unreadable")]);
        dom.append_text(p, "\u{2026}");
        SourceDocument {
            path: path.to_owned(),
            id: itemref.id.clone(),
            linear: itemref.linear,
            properties: itemref.properties.clone(),
            dom,
            parsed: Parsed::Generated,
            svg: false,
        }
    };
    let Some(item) = item else {
        return placeholder(
            &path,
            "the spine names an item the manifest does not have".to_owned(),
            warnings,
        );
    };
    let Some(real) = archive.find(&item.path).map(str::to_owned) else {
        return placeholder(&path, "missing from the archive".to_owned(), warnings);
    };
    let bytes = match archive.read(&real) {
        Ok(bytes) => bytes,
        Err(error) => return placeholder(&path, error.to_string(), warnings),
    };
    if !is_document_type(&item.media_type) {
        // A picture in the spine, with no fallback: shown as a page with the picture on it.
        let (mut dom, body) = empty_page(&item.path, None);
        let div = super::add(&mut dom, body, "div", &[("class", "musaeum-image-page")]);
        let name = item.path.rsplit('/').next().unwrap_or("");
        let source = url::encode_path(name);
        super::add(&mut dom, div, "img", &[("src", &source), ("alt", "")]);
        return SourceDocument {
            path: item.path.clone(),
            id: itemref.id.clone(),
            linear: itemref.linear,
            properties: itemref.properties.clone(),
            dom,
            parsed: Parsed::Generated,
            svg: false,
        };
    }
    let markup = match item.media_type.as_str() {
        "image/svg+xml" => Markup::Svg,
        "text/html" => Markup::Html,
        _ => Markup::Xhtml,
    };
    let text = decode_markup(&bytes);
    let mut warning = None;
    match parse_markup(&text, markup, limits, &mut warning) {
        Some((dom, parsed)) => {
            if let Some(warning) =
                warning.filter(|_| parsed == Parsed::Html && markup == Markup::Xhtml)
            {
                warnings.push(format!(
                    "{}: not well-formed XML ({warning}), read as HTML",
                    item.path
                ));
            }
            SourceDocument {
                path: item.path.clone(),
                id: itemref.id.clone(),
                linear: itemref.linear,
                properties: itemref.properties.clone(),
                dom,
                parsed,
                svg: markup == Markup::Svg,
            }
        }
        None => placeholder(
            &item.path,
            warning.unwrap_or_else(|| "unreadable".to_owned()),
            warnings,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idpf_obfuscation_is_its_own_inverse() {
        let original: Vec<u8> = (0..2000u32).map(|n| (n % 251) as u8).collect();
        let mut data = original.clone();
        deobfuscate(&mut data, IDPF_OBFUSCATION, " urn:uuid:1234 ", &[]);
        assert_ne!(data, original);
        assert_eq!(
            &data[1040..],
            &original[1040..],
            "only the first 1040 bytes change"
        );
        deobfuscate(&mut data, IDPF_OBFUSCATION, "urn:uuid:1234", &[]);
        assert_eq!(data, original, "white space does not count in the key");
    }

    #[test]
    fn adobe_obfuscation_uses_the_uuid() {
        let original = vec![0u8; 1100];
        let mut data = original.clone();
        let identifiers = [Identifier {
            value: "urn:uuid:00112233-4455-6677-8899-aabbccddeeff".to_owned(),
            scheme: None,
        }];
        deobfuscate(&mut data, ADOBE_OBFUSCATION, "isbn", &identifiers);
        assert_eq!(&data[..4], &[0x00, 0x11, 0x22, 0x33]);
        assert_eq!(&data[1024..], &original[1024..]);
    }
}

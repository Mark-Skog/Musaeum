//! The normalized book: what it holds, and the EPUB 3 package files that describe it to a reader.
//!
//! [`Manifest`] is `musaeum.json`, the part the Core reads. `package.opf`, `nav.xhtml` and
//! `META-INF/container.xml` say the same thing in EPUB's own terms, so that a reader that knows EPUB
//! (foliate-js) opens the normalized book as one.

use serde::{Deserialize, Serialize};

use crate::sanitize;
use crate::url;

pub const MANIFEST_PATH: &str = "musaeum.json";
pub const PACKAGE_PATH: &str = "package.opf";
pub const NAV_PATH: &str = "nav.xhtml";
pub const CONTAINER_PATH: &str = "META-INF/container.xml";

/// The format a book came in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceFormat {
    Epub,
    Mobi,
    Azw3,
    Fb2,
    Txt,
    Markdown,
}

/// Where the book came from, and how it asks to be shown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub format: SourceFormat,
    /// The format's own version: "3.0" for an EPUB, "6" or "8" for a Mobipocket file.
    pub version: Option<String>,
    /// `true` for a fixed-layout EPUB (`rendition:layout` `pre-paginated`).
    pub fixed_layout: bool,
    /// `rtl` when pages turn to the left.
    pub direction: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Title {
    pub value: String,
    /// `main`, `subtitle`, `short`, `collection`, `edition` or `expanded`, when the book says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_as: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Contributor {
    pub name: String,
    /// MARC relator codes (`aut`, `edt`, `trl`, `ill`…), as the book gives them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_as: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Identifier {
    pub value: String,
    /// `ISBN`, `UUID`, `DOI`, `ASIN` and the like, when the book says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Series {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<String>,
}

/// Everything a book says about itself. Text is as the book wrote it, white space folded.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Metadata {
    pub titles: Vec<Title>,
    pub creators: Vec<Contributor>,
    pub contributors: Vec<Contributor>,
    pub languages: Vec<String>,
    pub identifiers: Vec<Identifier>,
    /// The identifier the book names as its own, if it names one.
    pub unique_identifier: Option<String>,
    pub publisher: Option<String>,
    /// The publication date, as written.
    pub published: Option<String>,
    pub modified: Option<String>,
    /// As plain text, even when the book gave HTML.
    pub description: Option<String>,
    pub subjects: Vec<String>,
    pub rights: Option<String>,
    pub series: Option<Series>,
    /// The cover image, as a path in the normalized book.
    pub cover: Option<String>,
}

impl Metadata {
    /// The main title: the one marked `main`, or the first.
    pub fn title(&self) -> Option<&str> {
        self.titles
            .iter()
            .find(|title| title.kind.as_deref() == Some("main"))
            .or_else(|| self.titles.first())
            .map(|title| title.value.as_str())
    }
}

/// A part of a document, as served.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartInfo {
    pub href: String,
    /// Where the part starts in its document, as CFI steps below `<body>`; empty for the first.
    pub start: Vec<u32>,
    pub bytes: u64,
    /// Where its text starts in the document's text, in UTF-16 code units.
    pub text_start: u64,
    pub text_length: u64,
}

/// One document of the reading order: a CFI's spine step points at one of these, whatever it was
/// cut into.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Document {
    /// Its path in the source (for an EPUB, the path in the archive), which is what a Locator's
    /// `href` names.
    pub href: String,
    /// The `id` of its `itemref` in the source's spine, which a CFI asserts.
    pub id: Option<String>,
    pub media_type: String,
    pub linear: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub properties: Vec<String>,
    pub title: Option<String>,
    /// `xml` if it was well-formed, `html` if the HTML parser had to read it, `generated` if the
    /// normalizer made it (from a picture, or for a source document that could not be read).
    pub parsed: String,
    /// The CFI step of `<body>` under the root, when there is one.
    pub body_step: Option<u32>,
    /// The text under `<body>`, in UTF-16 code units.
    pub text_length: u64,
    pub parts: Vec<PartInfo>,
}

/// A file of the book other than its documents.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resource {
    pub href: String,
    pub media_type: String,
    pub bytes: u64,
    /// Its path in the source, when it had one.
    pub source: Option<String>,
}

/// An entry of the table of contents, the page list or the landmarks.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NavPoint {
    pub label: String,
    /// Where it leads in the normalized book: a part, and a fragment if it has one.
    pub href: Option<String>,
    /// The same place as a CFI, when the target could be found.
    pub cfi: Option<String>,
    /// For landmarks: `toc`, `bodymatter`, `cover`…
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<NavPoint>,
}

impl NavPoint {
    /// How many entries there are, this one and all below.
    pub fn count(points: &[NavPoint]) -> usize {
        points
            .iter()
            .map(|point| 1 + Self::count(&point.children))
            .sum()
    }
}

/// What the normalizer did, in numbers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub documents: u32,
    pub parts: u32,
    /// Documents larger than the budget, cut into parts.
    pub split_documents: u32,
    /// Parts still over the budget: one block alone is larger, and a block is never cut.
    pub oversize_parts: u32,
    pub largest_part: u64,
    /// The largest document before it was cut, as normalized XHTML.
    pub largest_document: u64,
    /// Documents that were not well-formed XML and went through the HTML parser.
    pub html_fallbacks: u32,
    pub resources: u32,
    pub resource_bytes: u64,
    /// Files of the source left out: scripts, unknown types, what nothing could use.
    pub dropped_files: u32,
    pub sanitized: sanitize::Report,
}

/// `musaeum.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub normalizer: u32,
    pub source: Source,
    pub metadata: Metadata,
    pub documents: Vec<Document>,
    pub resources: Vec<Resource>,
    pub toc: Vec<NavPoint>,
    pub page_list: Vec<NavPoint>,
    pub landmarks: Vec<NavPoint>,
    pub stats: Stats,
    pub warnings: Vec<String>,
}

impl Manifest {
    /// The parts in reading order, as the package's spine lists them: (document, part).
    pub fn spine(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.documents
            .iter()
            .enumerate()
            .flat_map(|(document, info)| (0..info.parts.len()).map(move |part| (document, part)))
    }

    /// The text of all documents before `document`, in UTF-16 code units.
    pub fn text_before(&self, document: usize) -> u64 {
        self.documents[..document]
            .iter()
            .map(|info| info.text_length)
            .sum()
    }

    pub fn total_text(&self) -> u64 {
        self.text_before(self.documents.len())
    }
}

/// The normalized book: its manifest and every file, sorted by path.
#[derive(Clone, Debug)]
pub struct NormalizedBook {
    pub manifest: Manifest,
    pub files: Vec<(String, Vec<u8>)>,
}

impl NormalizedBook {
    pub fn file(&self, path: &str) -> Option<&[u8]> {
        self.files
            .binary_search_by(|(name, _)| name.as_str().cmp(path))
            .ok()
            .map(|index| self.files[index].1.as_slice())
    }

    /// The whole book as an EPUB file, for readers and tools that want one file.
    pub fn to_epub(&self) -> Vec<u8> {
        let mut zip = crate::zipwrite::ZipWriter::new();
        // `mimetype` first and stored, as OCF requires.
        zip.add("mimetype", b"application/epub+zip", false);
        for (path, bytes) in &self.files {
            let compress = !path.ends_with(".jpg")
                && !path.ends_with(".png")
                && !path.ends_with(".gif")
                && !path.ends_with(".webp");
            zip.add(path, bytes, compress);
        }
        zip.finish()
    }
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\t' | '\n' | '\r' => out.push(' '),
            c if crate::xml::is_xml_char(c) => out.push(c),
            _ => {}
        }
    }
    out
}

pub fn container_xml() -> String {
    format!(
        "{}<container version=\"1.0\" xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\">\n\
         <rootfiles><rootfile full-path=\"{PACKAGE_PATH}\" media-type=\"application/oebps-package+xml\"/></rootfiles>\n\
         </container>\n",
        crate::serialize::XML_DECLARATION
    )
}

/// `package.opf`: the metadata a reader shows, every file, and the parts in reading order.
pub fn package_opf(manifest: &Manifest) -> String {
    let metadata = &manifest.metadata;
    let mut out = String::from(crate::serialize::XML_DECLARATION);
    out.push_str(
        "<package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\" unique-identifier=\"uid\" \
         prefix=\"rendition: http://www.idpf.org/vocab/rendition/#\">\n\
         <metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n",
    );
    let identifier = metadata
        .unique_identifier
        .clone()
        .or_else(|| metadata.identifiers.first().map(|id| id.value.clone()))
        .unwrap_or_else(|| "urn:musaeum:normalized".to_owned());
    out.push_str(&format!(
        "<dc:identifier id=\"uid\">{}</dc:identifier>\n",
        escape(&identifier)
    ));
    out.push_str(&format!(
        "<dc:title>{}</dc:title>\n",
        escape(metadata.title().unwrap_or("Untitled"))
    ));
    for creator in &metadata.creators {
        out.push_str(&format!(
            "<dc:creator>{}</dc:creator>\n",
            escape(&creator.name)
        ));
    }
    let language = metadata.languages.first().map_or("und", String::as_str);
    out.push_str(&format!(
        "<dc:language>{}</dc:language>\n",
        escape(language)
    ));
    out.push_str(&format!(
        "<meta property=\"dcterms:modified\">{}</meta>\n",
        escape(
            metadata
                .modified
                .as_deref()
                .filter(|m| m.ends_with('Z') && m.len() == 20)
                .unwrap_or("2000-01-01T00:00:00Z")
        )
    ));
    if manifest.source.fixed_layout {
        out.push_str("<meta property=\"rendition:layout\">pre-paginated</meta>\n");
    }
    if let Some(cover) = &metadata.cover
        && let Some(index) = manifest.resources.iter().position(|r| &r.href == cover)
    {
        out.push_str(&format!("<meta name=\"cover\" content=\"r{index}\"/>\n"));
    }
    out.push_str("</metadata>\n<manifest>\n");
    out.push_str(&format!(
        "<item id=\"nav\" href=\"{NAV_PATH}\" media-type=\"application/xhtml+xml\" properties=\"nav\"/>\n"
    ));
    for (document, part) in manifest.spine() {
        let info = &manifest.documents[document];
        let href = &info.parts[part].href;
        out.push_str(&format!(
            "<item id=\"d{document}p{part}\" href=\"{}\" media-type=\"{}\"/>\n",
            escape(&url::encode_path(href)),
            escape(&info.media_type)
        ));
    }
    for (index, resource) in manifest.resources.iter().enumerate() {
        let cover = metadata.cover.as_deref() == Some(resource.href.as_str());
        out.push_str(&format!(
            "<item id=\"r{index}\" href=\"{}\" media-type=\"{}\"{}/>\n",
            escape(&url::encode_path(&resource.href)),
            escape(&resource.media_type),
            if cover {
                " properties=\"cover-image\""
            } else {
                ""
            }
        ));
    }
    out.push_str("</manifest>\n<spine");
    if manifest.source.direction.as_deref() == Some("rtl") {
        out.push_str(" page-progression-direction=\"rtl\"");
    }
    out.push_str(">\n");
    for (document, part) in manifest.spine() {
        let info = &manifest.documents[document];
        let mut attributes = String::new();
        if !info.linear {
            attributes.push_str(" linear=\"no\"");
        }
        let properties: Vec<&str> = info
            .properties
            .iter()
            .map(String::as_str)
            .filter(|p| p.starts_with("page-spread-") || p.starts_with("rendition:"))
            .collect();
        if !properties.is_empty() && part == 0 {
            attributes.push_str(&format!(
                " properties=\"{}\"",
                escape(&properties.join(" "))
            ));
        }
        out.push_str(&format!(
            "<itemref idref=\"d{document}p{part}\"{attributes}/>\n"
        ));
    }
    out.push_str("</spine>\n</package>\n");
    out
}

fn nav_list(points: &[NavPoint], out: &mut String, depth: usize) {
    if points.is_empty() || depth > 64 {
        return;
    }
    out.push_str("<ol>");
    for point in points {
        out.push_str("<li>");
        match &point.href {
            Some(href) => out.push_str(&format!(
                "<a href=\"{}\">{}</a>",
                escape(href),
                escape(&point.label)
            )),
            None => out.push_str(&format!("<span>{}</span>", escape(&point.label))),
        }
        nav_list(&point.children, out, depth + 1);
        out.push_str("</li>");
    }
    out.push_str("</ol>\n");
}

/// `nav.xhtml`: the table of contents, the page list and the landmarks, as EPUB 3 navigation.
pub fn nav_xhtml(manifest: &Manifest) -> String {
    let mut out = format!(
        "{}<!DOCTYPE html>\n<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\">\n\
         <head><title>{}</title></head>\n<body>\n<nav epub:type=\"toc\" id=\"toc\">",
        crate::serialize::XML_DECLARATION,
        escape(manifest.metadata.title().unwrap_or("Contents"))
    );
    let mut toc = manifest.toc.clone();
    if toc.is_empty() {
        // A reader needs at least one entry; the first document will do.
        toc.push(NavPoint {
            label: manifest.metadata.title().unwrap_or("Start").to_owned(),
            href: manifest
                .documents
                .first()
                .and_then(|document| document.parts.first())
                .map(|part| part.href.clone()),
            ..NavPoint::default()
        });
    }
    nav_list(&toc, &mut out, 0);
    out.push_str("</nav>\n");
    if !manifest.page_list.is_empty() {
        out.push_str("<nav epub:type=\"page-list\" hidden=\"hidden\">");
        nav_list(&manifest.page_list, &mut out, 0);
        out.push_str("</nav>\n");
    }
    if !manifest.landmarks.is_empty() {
        out.push_str("<nav epub:type=\"landmarks\" hidden=\"hidden\"><ol>");
        for point in &manifest.landmarks {
            if let Some(href) = &point.href {
                out.push_str(&format!(
                    "<li><a epub:type=\"{}\" href=\"{}\">{}</a></li>",
                    escape(point.kind.as_deref().unwrap_or("other")),
                    escape(href),
                    escape(&point.label)
                ));
            }
        }
        out.push_str("</ol></nav>\n");
    }
    out.push_str("</body>\n</html>\n");
    out
}

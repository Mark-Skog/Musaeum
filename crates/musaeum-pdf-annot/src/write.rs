//! Annotations written into a PDF as an incremental update (ISO 32000-1, 7.5.6): the bytes to put
//! after the original's, which stay as they were, every one of them.
//!
//! The update holds a new object for each annotation and its appearance, a new version of each
//! page they go on (or of its `/Annots` array, when the page points to one), and a cross-reference
//! section of the kind the file's newest one is, whose `/Prev` is the original's. A new version of
//! a page is the original's own bytes with only `/Annots` changed, so whatever the page holds that
//! this does not understand stays exactly as it was.
//!
//! Each annotation carries its appearance (`/AP`), drawn as Musaeum draws the mark, so readers show
//! the same thing; markup is drawn multiplied over the page, as a highlighter's ink is.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::document::{Page, PdfFile, Result, SectionKind, broken};
use crate::model::{
    Annot, FLAG_NO_ROTATE, FLAG_NO_ZOOM, FLAG_PRINT, MAX_CONTENTS, MAX_NAME, Subtype, bounds,
    quad_points,
};
use crate::read::quad_points_from_ours;
use crate::syntax::{Object, Ref};
use crate::text::{format_date, hex_string, string_object, text_object};

/// The most annotations one update may add.
pub const MAX_NEW_ANNOTATIONS: usize = 100_000;

/// What to write besides the annotations themselves.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteOptions {
    /// `/T` for annotations that name no author.
    pub author: Option<String>,
    /// `/M` and `/CreationDate` for annotations that have none, in Unix milliseconds.
    pub now: i64,
}

/// One annotation as written.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WrittenAnnot {
    pub page: u32,
    pub name: Option<String>,
    pub number: u32,
}

/// The update, and what is in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Update {
    /// The bytes that go after the original's.
    pub tail: Vec<u8>,
    pub written: Vec<WrittenAnnot>,
}

/// A number as PDF writes it: at most five decimals, no exponent, no trailing zeros.
pub fn number(value: f64) -> String {
    if !value.is_finite() {
        return "0".to_owned();
    }
    let mut text = format!("{value:.5}");
    while text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    if text == "-0" || text.is_empty() {
        "0".to_owned()
    } else {
        text
    }
}

fn numbers(values: impl IntoIterator<Item = f64>) -> String {
    let parts: Vec<String> = values.into_iter().map(number).collect();
    format!("[{}]", parts.join(" "))
}

fn rgb(color: [f64; 3]) -> String {
    format!(
        "{} {} {}",
        number(color[0]),
        number(color[1]),
        number(color[2])
    )
}

/// The band just under a quad's text, a tenth of its height, as Musaeum draws an underline: from
/// 2 % to 12 % of each side past the bottom edge. Corners in Musaeum's order.
pub fn underline_band(quad: &[f64; 8]) -> [f64; 8] {
    let (lx, ly) = (quad[6] - quad[0], quad[7] - quad[1]);
    let (rx, ry) = (quad[4] - quad[2], quad[5] - quad[3]);
    let (blx, bly, brx, bry) = (quad[6], quad[7], quad[4], quad[5]);
    [
        blx + lx * 0.02,
        bly + ly * 0.02,
        brx + rx * 0.02,
        bry + ry * 0.02,
        brx + rx * 0.12,
        bry + ry * 0.12,
        blx + lx * 0.12,
        bly + ly * 0.12,
    ]
}

fn polygon(out: &mut String, quad: &[f64; 8]) {
    let _ = writeln!(
        out,
        "{} {} m {} {} l {} {} l {} {} l h",
        number(quad[0]),
        number(quad[1]),
        number(quad[2]),
        number(quad[3]),
        number(quad[4]),
        number(quad[5]),
        number(quad[6]),
        number(quad[7])
    );
}

/// What an annotation in user space looks like, and the box that holds it.
fn appearance(annot: &Annot, color: [f64; 3]) -> (String, [f64; 4], bool) {
    let mut content = String::new();
    let multiply = !matches!(annot.subtype, Some(Subtype::Text | Subtype::FreeText));
    if multiply {
        content.push_str("/M0 gs\n");
    }
    let rect = match annot.subtype {
        Some(Subtype::Underline | Subtype::Squiggly) => {
            let _ = writeln!(content, "{} rg", rgb(color));
            let bands: Vec<[f64; 8]> = annot.quads.iter().map(underline_band).collect();
            for band in &bands {
                polygon(&mut content, band);
            }
            content.push_str("f\n");
            bounds(quad_points(&annot.quads).chain(quad_points(&bands))).unwrap_or(annot.rect)
        }
        Some(Subtype::Highlight | Subtype::StrikeOut) => {
            let _ = writeln!(content, "{} rg", rgb(color));
            for quad in &annot.quads {
                polygon(&mut content, quad);
            }
            content.push_str("f\n");
            bounds(quad_points(&annot.quads)).unwrap_or(annot.rect)
        }
        Some(Subtype::Ink) => {
            let width = annot.width.unwrap_or(1.0);
            let _ = writeln!(content, "{} w 1 J 1 j {} RG", number(width), rgb(color));
            for stroke in &annot.ink {
                for (at, point) in stroke.as_chunks::<2>().0.iter().enumerate() {
                    let _ = write!(
                        content,
                        "{} {} {} ",
                        number(point[0]),
                        number(point[1]),
                        if at == 0 { "m" } else { "l" }
                    );
                }
                if stroke.len() == 2 {
                    let _ = write!(content, "{} {} l ", number(stroke[0]), number(stroke[1]));
                }
                content.push_str("S\n");
            }
            let half = width / 2.0 + 0.5;
            bounds(annot.ink.iter().flat_map(|stroke| {
                stroke.as_chunks::<2>().0.iter().flat_map(move |point| {
                    [
                        (point[0] - half, point[1] - half),
                        (point[0] + half, point[1] + half),
                    ]
                })
            }))
            .unwrap_or(annot.rect)
        }
        Some(Subtype::Square | Subtype::Circle) => {
            let [l, b, r, t] = annot.rect;
            let fill = annot.interior.unwrap_or(color);
            let _ = writeln!(
                content,
                "{} rg {} {} {} {} re f",
                rgb(fill),
                number(l),
                number(b),
                number(r - l),
                number(t - b)
            );
            annot.rect
        }
        Some(Subtype::Text | Subtype::FreeText) | None => {
            // A note: a square of its color, a dark edge, and three lines of writing.
            let [l, b, r, t] = annot.rect;
            let (w, h) = (r - l, t - b);
            let _ = writeln!(
                content,
                "{} rg {} {} {} {} re f",
                rgb(color),
                number(l),
                number(b),
                number(w),
                number(h)
            );
            let _ = writeln!(
                content,
                "0.25 0.25 0.25 RG {} w {} {} {} {} re S",
                number((w.min(h) / 24.0).max(0.5)),
                number(l + 0.5),
                number(b + 0.5),
                number(w - 1.0),
                number(h - 1.0)
            );
            for line in 1..=3 {
                let y = b + h * f64::from(line) / 4.0;
                let _ = writeln!(
                    content,
                    "{} {} m {} {} l S",
                    number(l + w * 0.2),
                    number(y),
                    number(r - w * 0.2),
                    number(y)
                );
            }
            annot.rect
        }
    };
    (content, rect, multiply)
}

/// The bytes after the original's that add `annots`, given with places as fractions of their
/// pages' frames.
pub fn append_annotations(
    file: &PdfFile<'_>,
    annots: &[Annot],
    options: &WriteOptions,
) -> Result<Update> {
    if annots.len() > MAX_NEW_ANNOTATIONS {
        return broken("too many annotations for one update");
    }
    let pages = file.pages()?;
    let mut tail = Tail::new(file);
    let multiply = tail.allocate();
    tail.object(
        multiply,
        b"<< /Type /ExtGState /BM /Multiply /CA 1 /ca 1 >>".to_vec(),
    );

    let mut by_page: BTreeMap<u32, Vec<Ref>> = BTreeMap::new();
    let mut written = Vec::with_capacity(annots.len());
    for annot in annots {
        let Some(page) = pages.get(annot.page as usize) else {
            return broken(format!("there is no page {}", annot.page + 1));
        };
        if !page.frame.is_usable() {
            return broken(format!(
                "page {} has no box to place anything in",
                annot.page + 1
            ));
        }
        let Some(subtype) = annot.subtype else {
            return broken("an annotation of no kind");
        };
        let user = annot.to_user(&page.frame);
        let color = annot.color.unwrap_or([1.0, 0.89, 0.42]);
        let (content, rect, multiplied) = appearance(&user, color);
        let rect_text = numbers(rect);
        let appearance_ref = tail.allocate();
        let resources = if multiplied {
            format!("/Resources << /ExtGState << /M0 {multiply} >> >> ")
        } else {
            String::new()
        };
        let mut stream = format!(
            "<< /Type /XObject /Subtype /Form /FormType 1 /BBox {rect_text} {resources}/Length {} >>\nstream\n",
            content.len()
        )
        .into_bytes();
        stream.extend_from_slice(content.as_bytes());
        stream.extend_from_slice(b"\nendstream");
        tail.object(appearance_ref, stream);

        let mut dictionary = format!(
            "<< /Type /Annot /Subtype /{} /Rect {rect_text} /P {} /F {}",
            subtype.name(),
            page.reference,
            if annot.flags != 0 {
                annot.flags
            } else if subtype == Subtype::Text {
                FLAG_PRINT | FLAG_NO_ZOOM | FLAG_NO_ROTATE
            } else {
                FLAG_PRINT
            }
        );
        let short = |text: &str| text.chars().take(MAX_NAME).collect::<String>();
        if let Some(name) = &annot.name {
            let _ = write!(dictionary, " /NM {}", text_object(&short(name)));
        }
        if let Some(author) = annot.author.as_ref().or(options.author.as_ref()) {
            let _ = write!(dictionary, " /T {}", text_object(&short(author)));
        }
        if let Some(subject) = &annot.subject {
            let _ = write!(dictionary, " /Subj {}", text_object(&short(subject)));
        }
        let modified = annot.modified.unwrap_or(options.now);
        let created = annot.created.unwrap_or(modified);
        let _ = write!(
            dictionary,
            " /M {} /CreationDate {}",
            string_object(format_date(modified).as_bytes()),
            string_object(format_date(created).as_bytes())
        );
        let _ = write!(
            dictionary,
            " /C [{}] /CA {}",
            rgb(color),
            number(annot.opacity.clamp(0.0, 1.0))
        );
        if !annot.contents.is_empty() {
            let contents: String = annot.contents.chars().take(MAX_CONTENTS).collect();
            let _ = write!(dictionary, " /Contents {}", text_object(&contents));
        }
        match subtype {
            Subtype::Highlight | Subtype::Underline | Subtype::Squiggly | Subtype::StrikeOut => {
                if user.quads.is_empty() {
                    return broken("a text markup with no quads");
                }
                let points: Vec<f64> = user.quads.iter().flat_map(quad_points_from_ours).collect();
                let _ = write!(dictionary, " /QuadPoints {}", numbers(points));
            }
            Subtype::Ink => {
                if user.ink.is_empty() {
                    return broken("ink with no strokes");
                }
                let strokes: Vec<String> = user
                    .ink
                    .iter()
                    .map(|stroke| numbers(stroke.iter().copied()))
                    .collect();
                let _ = write!(
                    dictionary,
                    " /InkList [{}] /BS << /Type /Border /W {} /S /S >>",
                    strokes.join(" "),
                    number(annot.width.unwrap_or(1.0))
                );
            }
            Subtype::Square | Subtype::Circle => {
                if let Some(interior) = annot.interior {
                    let _ = write!(dictionary, " /IC [{}]", rgb(interior));
                }
                let _ = write!(
                    dictionary,
                    " /BS << /Type /Border /W {} /S /S >>",
                    number(annot.width.unwrap_or(0.0))
                );
            }
            Subtype::Text => {
                let icon: String = annot
                    .icon
                    .as_deref()
                    .unwrap_or("Comment")
                    .chars()
                    .filter(char::is_ascii_alphanumeric)
                    .take(32)
                    .collect();
                let _ = write!(dictionary, " /Name /{icon} /Open false");
            }
            Subtype::FreeText => {
                dictionary.push_str(" /DA (0 g /Helv 10 Tf)");
            }
        }
        let _ = write!(dictionary, " /AP << /N {appearance_ref} >> >>");
        let annot_ref = tail.allocate();
        tail.object(annot_ref, dictionary.into_bytes());
        by_page.entry(annot.page).or_default().push(annot_ref);
        written.push(WrittenAnnot {
            page: annot.page,
            name: annot.name.clone(),
            number: annot_ref.number,
        });
    }

    let shared = shared_annots_arrays(file, &pages, &by_page)?;
    for (index, refs) in &by_page {
        add_to_page(file, &pages[*index as usize], refs, &shared, &mut tail)?;
    }
    Ok(Update {
        tail: tail.finish(file)?,
        written,
    })
}

/// The `/Annots` arrays that pages point to and that more than one page points to: a new version
/// of one of those would put a page's annotations on another, so those pages get arrays of their
/// own instead.
fn shared_annots_arrays(
    file: &PdfFile<'_>,
    pages: &[Page],
    touched: &BTreeMap<u32, Vec<Ref>>,
) -> Result<HashMap<u32, usize>> {
    let mut wanted = HashMap::new();
    for index in touched.keys() {
        let loaded = file.load(pages[*index as usize].reference.number)?;
        if let Some(Object::Reference(array)) = loaded
            .value
            .as_dictionary()
            .and_then(|page| page.get(b"Annots"))
        {
            wanted.insert(array.number, 0usize);
        }
    }
    if wanted.is_empty() {
        return Ok(wanted);
    }
    for page in pages {
        let loaded = file.load(page.reference.number)?;
        if let Some(Object::Reference(array)) = loaded
            .value
            .as_dictionary()
            .and_then(|page| page.get(b"Annots"))
            && let Some(count) = wanted.get_mut(&array.number)
        {
            *count += 1;
        }
    }
    Ok(wanted)
}

/// Writes the new version of a page, or of its `/Annots` array, with `refs` added.
fn add_to_page(
    file: &PdfFile<'_>,
    page: &Page,
    refs: &[Ref],
    shared: &HashMap<u32, usize>,
    tail: &mut Tail,
) -> Result<()> {
    let listed: String = refs
        .iter()
        .map(|reference| format!(" {reference}"))
        .collect();
    let loaded = file.load(page.reference.number)?;
    let Some(dictionary) = loaded.value.as_dictionary() else {
        return broken(format!(
            "page object {} is no dictionary",
            page.reference.number
        ));
    };
    let text = loaded.text.bytes();
    let whole = &text[dictionary.open..dictionary.close + 2];
    let open = dictionary.open;
    let entry = dictionary.entry(b"Annots");
    let body: Vec<u8> = match entry.map(|entry| (&entry.value, entry)) {
        // The page points to an array of its own: a new version of the array.
        Some((Object::Reference(array), _)) if shared.get(&array.number) == Some(&1) => {
            match file.entry(array.number) {
                Some(crate::document::XrefEntry::Free) | None => {
                    // A reference to nothing is null: the page gets an array of its own.
                    replace_entry(whole, open, entry, &format!("/Annots [{listed} ]"))
                }
                Some(_) => {
                    let array_loaded = file.load(array.number)?;
                    if array_loaded.value.as_array().is_none() {
                        return broken(format!(
                            "/Annots of page object {} is no array",
                            page.reference.number
                        ));
                    }
                    let raw = array_loaded.raw();
                    let mut new_array = raw[..raw.len() - 1].to_vec();
                    new_array.extend_from_slice(listed.as_bytes());
                    new_array.extend_from_slice(b" ]");
                    tail.object(
                        Ref {
                            number: array.number,
                            generation: array_loaded.reference.generation,
                        },
                        new_array,
                    );
                    return Ok(());
                }
            }
        }
        // An array shared with other pages: this page gets a copy of its own.
        Some((Object::Reference(array), entry)) => {
            let current = file.resolve(&Object::Reference(*array))?;
            let Some(items) = current.as_array() else {
                return broken(format!(
                    "/Annots of page object {} is no array",
                    page.reference.number
                ));
            };
            let mut own = String::from("/Annots [");
            for item in items {
                let Object::Reference(reference) = item else {
                    return broken("a shared /Annots array holding annotations in itself");
                };
                let _ = write!(own, " {reference}");
            }
            let _ = write!(own, "{listed} ]");
            replace_entry(whole, open, Some(entry), &own)
        }
        Some((Object::Array(_), entry)) => {
            let raw = &text[entry.value_start..entry.value_end];
            let mut value = String::from_utf8_lossy(&raw[..raw.len() - 1]).into_owned();
            // The bytes of the array are kept as they are, whatever they hold.
            let mut bytes = raw[..raw.len() - 1].to_vec();
            bytes.extend_from_slice(listed.as_bytes());
            bytes.extend_from_slice(b" ]");
            value.clear();
            let mut out = Vec::with_capacity(whole.len() + listed.len() + 2);
            out.extend_from_slice(&text[open..entry.value_start]);
            out.extend_from_slice(&bytes);
            out.extend_from_slice(&text[entry.value_end..dictionary.close + 2]);
            out
        }
        // Null, or something that is no array, which readers take as none.
        Some((_, entry)) => {
            replace_entry(whole, open, Some(entry), &format!("/Annots [{listed} ]"))
        }
        None => replace_entry(whole, open, None, &format!("/Annots [{listed} ]")),
    };
    tail.object(
        Ref {
            number: page.reference.number,
            generation: loaded.reference.generation,
        },
        body,
    );
    Ok(())
}

/// The dictionary `whole` (whose `<<` was at `open` in the bytes its spans count in) with `entry`
/// replaced by `new_entry`, or with `new_entry` added before its `>>`.
fn replace_entry(
    whole: &[u8],
    open: usize,
    entry: Option<&crate::syntax::Entry>,
    new_entry: &str,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(whole.len() + new_entry.len() + 2);
    match entry {
        Some(entry) => {
            out.extend_from_slice(&whole[..entry.key_start - open]);
            out.extend_from_slice(new_entry.as_bytes());
            out.extend_from_slice(&whole[entry.value_end - open..]);
        }
        None => {
            let close = whole.len() - 2;
            out.extend_from_slice(&whole[..close]);
            out.push(b' ');
            out.extend_from_slice(new_entry.as_bytes());
            out.push(b' ');
            out.extend_from_slice(&whole[close..]);
        }
    }
    out
}

/// The objects of an update as they are written, and where each starts in the whole file.
struct Tail {
    base: u64,
    out: Vec<u8>,
    next: u32,
    offsets: BTreeMap<u32, (u64, u16)>,
}

impl Tail {
    fn new(file: &PdfFile<'_>) -> Self {
        let mut out = Vec::new();
        // The update starts on a line of its own.
        if !matches!(file.bytes.last(), Some(b'\n' | b'\r')) {
            out.push(b'\n');
        }
        Self {
            base: file.bytes.len() as u64,
            out,
            next: file.size.max(1),
            offsets: BTreeMap::new(),
        }
    }

    fn allocate(&mut self) -> Ref {
        let number = self.next;
        self.next += 1;
        Ref {
            number,
            generation: 0,
        }
    }

    fn position(&self) -> u64 {
        self.base + self.out.len() as u64
    }

    fn object(&mut self, reference: Ref, body: Vec<u8>) {
        self.offsets
            .insert(reference.number, (self.position(), reference.generation));
        self.out.extend_from_slice(
            format!("{} {} obj\n", reference.number, reference.generation).as_bytes(),
        );
        self.out.extend_from_slice(&body);
        self.out.extend_from_slice(b"\nendobj\n");
    }

    /// Runs of consecutive object numbers: first and count.
    fn runs(numbers: impl Iterator<Item = u32>) -> Vec<(u32, u32)> {
        let mut runs: Vec<(u32, u32)> = Vec::new();
        for number in numbers {
            match runs.last_mut() {
                Some((first, count)) if *first + *count == number => *count += 1,
                _ => runs.push((number, 1)),
            }
        }
        runs
    }

    /// The trailer's entries every section must repeat.
    fn trailer_entries(&self, file: &PdfFile<'_>, size: u32) -> Result<String> {
        let Some(Object::Reference(root)) = file.trailer.get(b"Root") else {
            return broken("the trailer names no catalog");
        };
        let mut entries = format!("/Size {size} /Root {root}");
        if let Some(Object::Reference(info)) = file.trailer.get(b"Info") {
            let _ = write!(entries, " /Info {info}");
        }
        // The first identifier stays, as the file's own; the second changes with the file.
        let changed = blake3::hash(&self.out);
        let changed = &changed.as_bytes()[..16];
        let (first, _) = file
            .id()
            .unwrap_or_else(|| (changed.to_vec(), changed.to_vec()));
        let _ = write!(
            entries,
            " /ID [{} {}] /Prev {}",
            hex_string(&first),
            hex_string(changed),
            file.startxref
        );
        Ok(entries)
    }

    fn finish(mut self, file: &PdfFile<'_>) -> Result<Vec<u8>> {
        match file.newest {
            SectionKind::Table => {
                let size = self.next.max(file.size);
                let trailer = self.trailer_entries(file, size)?;
                let at = self.position();
                let mut table = String::from("xref\n");
                for (first, count) in Self::runs(self.offsets.keys().copied()) {
                    let _ = writeln!(table, "{first} {count}");
                    for number in first..first + count {
                        let (offset, generation) = self.offsets[&number];
                        let _ = writeln!(table, "{offset:010} {generation:05} n ");
                    }
                }
                let _ = write!(table, "trailer\n<< {trailer} >>\nstartxref\n{at}\n%%EOF\n");
                self.out.extend_from_slice(table.as_bytes());
            }
            SectionKind::Stream => {
                let own = self.allocate();
                let at = self.position();
                self.offsets.insert(own.number, (at, 0));
                let size = self.next.max(file.size);
                let width = (64 - at.leading_zeros()).div_ceil(8).max(4) as usize;
                let mut rows = Vec::with_capacity(self.offsets.len() * (3 + width));
                for (offset, generation) in self.offsets.values() {
                    rows.push(1u8);
                    rows.extend_from_slice(&offset.to_be_bytes()[8 - width..]);
                    rows.extend_from_slice(&generation.to_be_bytes());
                }
                let index: Vec<String> = Self::runs(self.offsets.keys().copied())
                    .iter()
                    .map(|(first, count)| format!("{first} {count}"))
                    .collect();
                let trailer = self.trailer_entries(file, size)?;
                let mut stream = format!(
                    "{} 0 obj\n<< /Type /XRef {trailer} /Index [{}] /W [1 {width} 2] /Length {} >>\nstream\n",
                    own.number,
                    index.join(" "),
                    rows.len()
                )
                .into_bytes();
                stream.extend_from_slice(&rows);
                stream.extend_from_slice(
                    format!("\nendstream\nendobj\nstartxref\n{at}\n%%EOF\n").as_bytes(),
                );
                self.out.extend_from_slice(&stream);
            }
        }
        Ok(self.out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_short_and_plain() {
        assert_eq!(number(1.0), "1");
        assert_eq!(number(0.5), "0.5");
        assert_eq!(number(-0.000001), "0");
        assert_eq!(number(595.27559), "595.27559");
        assert_eq!(number(1e-7), "0");
        assert_eq!(number(f64::NAN), "0");
        assert_eq!(number(-12.25), "-12.25");
    }

    #[test]
    fn the_underline_band_is_just_under_the_text() {
        // Ours: top left, top right, bottom right, bottom left, in user space (y up).
        let quad = [10.0, 110.0, 50.0, 110.0, 50.0, 100.0, 10.0, 100.0];
        let band = underline_band(&quad);
        assert_eq!(
            band,
            [10.0, 99.8, 50.0, 99.8, 50.0, 98.8, 10.0, 98.8].map(|v: f64| v)
        );
    }
}

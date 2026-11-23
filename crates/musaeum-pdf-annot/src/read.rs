//! The annotations a PDF holds, read from its pages' `/Annots` (ISO 32000-1, 12.5).
//!
//! What is read is what Musaeum keeps: the kind, the box, the quads of a text markup, the strokes
//! of ink, colors, opacity, the stroke's width, the note, the name (`/NM`), author, subject, dates
//! and a sticky note's icon. Kinds it does not keep are counted, not read.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::document::{PdfFile, Result};
use crate::model::{
    Annot, Frame, MAX_CONTENTS, MAX_NAME, MAX_QUADS, MAX_STROKE_POINTS, MAX_STROKES, Subtype,
};
use crate::syntax::{Dictionary, Object};
use crate::text::{decode_text, parse_date};

/// The most annotations read from one page.
pub const MAX_PAGE_ANNOTATIONS: usize = 20_000;

/// What a PDF holds.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reading {
    /// Every page's frame, in order.
    pub frames: Vec<Frame>,
    /// The annotations of the kinds Musaeum keeps, with places as fractions of their page.
    pub annots: Vec<Annot>,
    /// How many of each other kind there were, by its PDF name, and `"(broken)"` for those that
    /// could not be read.
    pub skipped: BTreeMap<String, u32>,
}

/// Reads every page's annotations.
pub fn read_annotations(file: &PdfFile<'_>) -> Result<Reading> {
    let pages = file.pages()?;
    let mut reading = Reading {
        frames: pages.iter().map(|page| page.frame).collect(),
        ..Reading::default()
    };
    for (index, page) in pages.iter().enumerate() {
        let loaded = file.load(page.reference.number)?;
        let Some(dictionary) = loaded.value.as_dictionary() else {
            continue;
        };
        let items = match file.get(dictionary, b"Annots")? {
            Some(Object::Array(items)) => items,
            _ => continue,
        };
        for item in items.iter().take(MAX_PAGE_ANNOTATIONS) {
            let Ok(Object::Dictionary(annot)) = file.resolve(item) else {
                *reading.skipped.entry("(broken)".to_owned()).or_default() += 1;
                continue;
            };
            let subtype = annot.name(b"Subtype").unwrap_or(b"(none)");
            let Some(known) = Subtype::from_name(subtype) else {
                *reading
                    .skipped
                    .entry(String::from_utf8_lossy(subtype).into_owned())
                    .or_default() += 1;
                continue;
            };
            match read_one(file, &annot, known, index as u32) {
                Ok(Some(found)) if page.frame.is_usable() => {
                    reading.annots.push(found.to_uv(&page.frame));
                }
                _ => *reading.skipped.entry("(broken)".to_owned()).or_default() += 1,
            }
        }
    }
    Ok(reading)
}

fn text_of(
    file: &PdfFile<'_>,
    annot: &Dictionary,
    key: &[u8],
    limit: usize,
) -> Result<Option<String>> {
    Ok(match file.get(annot, key)? {
        Some(Object::String(bytes)) => {
            let text: String = decode_text(&bytes).chars().take(limit).collect();
            Some(text)
        }
        _ => None,
    })
}

fn numbers_of(file: &PdfFile<'_>, annot: &Dictionary, key: &[u8]) -> Result<Option<Vec<f64>>> {
    Ok(match file.get(annot, key)? {
        Some(Object::Array(items)) => {
            let mut numbers = Vec::with_capacity(items.len());
            for item in &items {
                match file.resolve(item)?.as_number() {
                    Some(number) => numbers.push(number),
                    None => return Ok(None),
                }
            }
            Some(numbers)
        }
        _ => None,
    })
}

/// A color array: gray, RGB or CMYK; none when it is empty, which means transparent.
fn color_of(file: &PdfFile<'_>, annot: &Dictionary, key: &[u8]) -> Result<Option<[f64; 3]>> {
    let Some(values) = numbers_of(file, annot, key)? else {
        return Ok(None);
    };
    let clamp = |value: f64| value.clamp(0.0, 1.0);
    Ok(match values.as_slice() {
        [gray] => Some([clamp(*gray); 3]),
        [r, g, b] => Some([clamp(*r), clamp(*g), clamp(*b)]),
        [c, m, y, k] => {
            let k = clamp(*k);
            Some([
                (1.0 - clamp(*c)) * (1.0 - k),
                (1.0 - clamp(*m)) * (1.0 - k),
                (1.0 - clamp(*y)) * (1.0 - k),
            ])
        }
        _ => None,
    })
}

/// One annotation in user space, or `None` when it has no box or its geometry does not hold.
fn read_one(
    file: &PdfFile<'_>,
    annot: &Dictionary,
    subtype: Subtype,
    page: u32,
) -> Result<Option<Annot>> {
    let Some(rect) = numbers_of(file, annot, b"Rect")?.and_then(|r| <[f64; 4]>::try_from(r).ok())
    else {
        return Ok(None);
    };
    let rect = [
        rect[0].min(rect[2]),
        rect[1].min(rect[3]),
        rect[0].max(rect[2]),
        rect[1].max(rect[3]),
    ];
    let mut found = Annot {
        page,
        subtype: Some(subtype),
        rect,
        opacity: file
            .get(annot, b"CA")?
            .and_then(|value| value.as_number())
            .map_or(1.0, |value| value.clamp(0.0, 1.0)),
        color: color_of(file, annot, b"C")?,
        interior: color_of(file, annot, b"IC")?,
        contents: text_of(file, annot, b"Contents", MAX_CONTENTS)?.unwrap_or_default(),
        name: text_of(file, annot, b"NM", MAX_NAME)?,
        author: text_of(file, annot, b"T", MAX_NAME)?,
        subject: text_of(file, annot, b"Subj", MAX_NAME)?,
        created: text_of(file, annot, b"CreationDate", 64)?.and_then(|date| parse_date(&date)),
        modified: text_of(file, annot, b"M", 64)?.and_then(|date| parse_date(&date)),
        flags: file
            .get(annot, b"F")?
            .and_then(|value| value.as_integer())
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(0),
        reply: annot.get(b"IRT").is_some(),
        ..Annot::default()
    };
    if subtype == Subtype::Text {
        found.icon = match file.get(annot, b"Name")? {
            Some(Object::Name(name)) => Some(
                String::from_utf8_lossy(&name)
                    .chars()
                    .take(MAX_NAME)
                    .collect(),
            ),
            _ => None,
        };
    }
    found.width = match file.get(annot, b"BS")? {
        Some(Object::Dictionary(border)) => file.get(&border, b"W")?.and_then(|w| w.as_number()),
        _ => None,
    }
    .or(numbers_of(file, annot, b"Border")?.and_then(|border| border.get(2).copied()))
    .filter(|width| width.is_finite() && *width >= 0.0);

    if subtype.is_markup() {
        let Some(points) = numbers_of(file, annot, b"QuadPoints")? else {
            return Ok(None);
        };
        if points.is_empty() || points.len() % 8 != 0 || points.len() / 8 > MAX_QUADS {
            return Ok(None);
        }
        found.quads = points
            .as_chunks::<8>()
            .0
            .iter()
            .map(|chunk| {
                let mut quad = [0.0; 8];
                quad.copy_from_slice(chunk);
                ours_from_quad_points(quad)
            })
            .collect();
    }
    if subtype == Subtype::Ink {
        let Some(Object::Array(strokes)) = file.get(annot, b"InkList")? else {
            return Ok(None);
        };
        if strokes.is_empty() || strokes.len() > MAX_STROKES {
            return Ok(None);
        }
        for stroke in &strokes {
            let Some(points) = file.resolve(stroke)?.as_numbers() else {
                return Ok(None);
            };
            if points.len() < 2 || points.len() % 2 != 0 || points.len() / 2 > MAX_STROKE_POINTS {
                return Ok(None);
            }
            found.ink.push(points);
        }
    }
    Ok(Some(found))
}

/// Whether segments `a`–`b` and `c`–`d` cross each other.
fn cross(a: (f64, f64), b: (f64, f64), c: (f64, f64), d: (f64, f64)) -> bool {
    let side = |p: (f64, f64), q: (f64, f64), r: (f64, f64)| {
        (q.0 - p.0) * (r.1 - p.1) - (q.1 - p.1) * (r.0 - p.0)
    };
    let (d1, d2) = (side(c, d, a), side(c, d, b));
    let (d3, d4) = (side(a, b, c), side(a, b, d));
    d1 * d2 < 0.0 && d3 * d4 < 0.0
}

/// One `/QuadPoints` quad in Musaeum's order: top left, top right, bottom right, bottom left.
///
/// Acrobat writes upper left, upper right, lower left, lower right, which PDFium and pdf.js read
/// too, although the specification's text speaks of counterclockwise order. Taken in Acrobat's
/// order, a quad is a simple polygon; written counterclockwise from the lower left, it would cross
/// itself, and the other order is then the one meant.
pub fn ours_from_quad_points(quad: [f64; 8]) -> [f64; 8] {
    let p = |at: usize| (quad[at * 2], quad[at * 2 + 1]);
    let (p0, p1, p2, p3) = (p(0), p(1), p(2), p(3));
    let acrobat_crosses = cross(p0, p1, p3, p2) || cross(p1, p3, p2, p0);
    let listed_crosses = cross(p0, p1, p2, p3) || cross(p1, p2, p3, p0);
    let [tl, tr, br, bl] = if acrobat_crosses && !listed_crosses {
        // Counterclockwise from the lower left.
        [p3, p2, p1, p0]
    } else {
        [p0, p1, p3, p2]
    };
    [tl.0, tl.1, tr.0, tr.1, br.0, br.1, bl.0, bl.1]
}

/// A quad in Musaeum's order as `/QuadPoints` has it, in Acrobat's order.
pub fn quad_points_from_ours(quad: &[f64; 8]) -> [f64; 8] {
    [
        quad[0], quad[1], quad[2], quad[3], quad[6], quad[7], quad[4], quad[5],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_orders_of_quad_points_come_out_as_ours() {
        // Text from x 10 to 50, between y 100 and 112, in user space.
        let ours = [10.0, 112.0, 50.0, 112.0, 50.0, 100.0, 10.0, 100.0];
        let acrobat = [10.0, 112.0, 50.0, 112.0, 10.0, 100.0, 50.0, 100.0];
        assert_eq!(ours_from_quad_points(acrobat), ours);
        assert_eq!(quad_points_from_ours(&ours), acrobat);
        let counterclockwise = [10.0, 100.0, 50.0, 100.0, 50.0, 112.0, 10.0, 112.0];
        assert_eq!(ours_from_quad_points(counterclockwise), ours);
        // Text running up the page, as on a page turned a quarter: the top is to the left.
        let up_ours = [88.0, 10.0, 88.0, 50.0, 100.0, 50.0, 100.0, 10.0];
        assert_eq!(
            ours_from_quad_points(quad_points_from_ours(&up_ours)),
            up_ours
        );
        // Nothing to tell from a quad with no area: Acrobat's order.
        let flat = [1.0, 1.0, 2.0, 1.0, 1.0, 1.0, 2.0, 1.0];
        assert_eq!(
            ours_from_quad_points(flat),
            [1.0, 1.0, 2.0, 1.0, 2.0, 1.0, 1.0, 1.0]
        );
    }
}

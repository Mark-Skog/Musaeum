//! A PDF's own annotations, in and out of the reader: a copy of the book with Musaeum's
//! annotations in it, the annotations of a PDF brought into the library, and XFDF both ways.
//!
//! The book is only ever read. An export writes a new file the person chose, never one the
//! library has: the book's bytes, unchanged and checked against the hash the library knows, then
//! the update the annotations worker made, into a temporary file beside the one chosen; PDFium
//! reads it back and must find every annotation where it was put before the file is renamed into
//! place. Every PDF and XFDF file is parsed in the worker's sandbox, never here.
//!
//! An annotation goes to a PDF as the mark it is: text marked by highlight or underline becomes
//! `/Highlight` or `/Underline` with its quads; a box `/Square`; a stroke `/Ink`; a pinned note
//! `/Text`. Its UUID is its `/NM`, so it comes back as itself; one that covers several pages is one
//! PDF annotation per page, the first named by the UUID and the others `UUID#2`, `UUID#3`. Its kind
//! goes as `/Subj` in the interface's language (Realce, Sublinhado, Nota; Highlight, Underline,
//! Note), read back in either, and its color as `/C`, as drawn. Each is signed (`/T`, an XFDF
//! `title`) with the name chosen in the preferences, or the system's user name.

use std::collections::{BTreeMap, HashMap};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use musaeum_core::FixedFormat;
use musaeum_library::anchoring::fixed;
use musaeum_library::anchoring::region::{self, Region, RegionTool};
use musaeum_library::{
    AnchorState, Annotation, AnnotationKind, ImportedAnnotation, Library, NewAnnotation,
    QuoteSource, Segment, SelectorKind,
};
use musaeum_pdf_annot::client::{AnnotClient, AnnotWorkerConfig};
use musaeum_pdf_annot::model::{Annot, Frame, Subtype};
use musaeum_pdf_annot::protocol::{Found, Reader};
use musaeum_pdf_annot::write::WriteOptions;
use musaeum_shm::chars::{chars_in_quads, line_quads};
use musaeum_shm::{CHARS_VERSION, CharLayer};
use musaeum_vision::LINES_VERSION;
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, State, WebviewWindow};

use musaeum_i18n::{Locale, T, text, text_in, tr};

use crate::annotations::{CharCache, awaits_quote, fixed_annotations};
use crate::exchange::check_outside_library;
use crate::picker::{FileKind, pick_file, pick_save};
use crate::session::{AppState, Session};
use crate::shelf::owner_of;

/// A stroke's width as the reader draws it, in points (`compositor/annotator.ts`).
const STROKE_PT: f64 = 9.0;
/// The largest XFDF file read.
const MAX_XFDF: u64 = 64 << 20;
const PDF: FileKind = FileKind {
    name: T::FileKindPdf,
    pattern: "*.pdf",
};
const XFDF: FileKind = FileKind {
    name: T::FileKindXfdf,
    pattern: "*.xfdf",
};

/// Highlighter colors over white paper, as `compositor/annotator.ts` has them.
const PALETTE: [(&str, [f64; 3]); 5] = [
    ("yellow", [1.0, 0.89, 0.42]),
    ("green", [0.66, 0.9, 0.63]),
    ("blue", [0.6, 0.79, 0.99]),
    ("pink", [0.98, 0.68, 0.84]),
    ("purple", [0.78, 0.71, 0.99]),
];
/// What an underline in each color is drawn with.
const INK: [(&str, [f64; 3]); 5] = [
    ("yellow", [0.85, 0.6, 0.0]),
    ("green", [0.13, 0.6, 0.2]),
    ("blue", [0.1, 0.4, 0.85]),
    ("pink", [0.85, 0.2, 0.5]),
    ("purple", [0.45, 0.25, 0.85]),
];

/// The message each kind goes as, in `/Subj`.
fn subject_message(kind: AnnotationKind) -> T {
    match kind {
        AnnotationKind::Highlight => T::SubjectHighlight,
        AnnotationKind::Underline => T::SubjectUnderline,
        AnnotationKind::Note => T::SubjectNote,
    }
}

/// The name each kind goes as, in `/Subj`, in the interface's language.
fn subject_of(kind: AnnotationKind) -> String {
    text(subject_message(kind), &[])
}

/// The kind a `/Subj` names, in any language the app writes it in.
fn kind_of_subject(subject: Option<&str>) -> Option<AnnotationKind> {
    let subject = subject?.trim();
    [
        AnnotationKind::Highlight,
        AnnotationKind::Underline,
        AnnotationKind::Note,
    ]
    .into_iter()
    .find(|kind| {
        Locale::ALL
            .iter()
            .any(|locale| text_in(*locale, subject_message(*kind), &[]) == subject)
    })
}

/// A color as the reader keeps it: a name of the palette, or `#rrggbb`, or `#rrggbbaa` with an
/// opacity. `underline` is whether it is the ink of a line rather than the fill of a mark.
pub fn color_of_name(name: &str, underline: bool) -> ([f64; 3], f64) {
    let table = if underline { &INK } else { &PALETTE };
    if let Some((_, rgb)) = table.iter().find(|(known, _)| *known == name) {
        return (*rgb, 1.0);
    }
    let hex = name.strip_prefix('#').unwrap_or("");
    let channel = |at: usize| {
        hex.get(at..at + 2)
            .and_then(|digits| u8::from_str_radix(digits, 16).ok())
            .map(|value| f64::from(value) / 255.0)
    };
    match (hex.len(), channel(0), channel(2), channel(4)) {
        (6 | 8, Some(r), Some(g), Some(b)) => (
            ([r, g, b]),
            if hex.len() == 8 {
                channel(6).unwrap_or(1.0)
            } else {
                1.0
            },
        ),
        _ => (table[0].1, 1.0),
    }
}

/// The reader's name for a color a PDF gives: the palette's when it is one of its colors, to
/// within a step of 255, and `#rrggbb` (`aa` after it, for an opacity) otherwise.
pub fn name_of_color(rgb: [f64; 3], opacity: f64, underline: bool) -> String {
    let near = |a: &[f64; 3]| {
        a.iter()
            .zip(&rgb)
            .all(|(x, y)| (x - y).abs() <= 1.5 / 255.0)
    };
    let (first, second) = if underline {
        (&INK, &PALETTE)
    } else {
        (&PALETTE, &INK)
    };
    if opacity >= 0.995
        && let Some((name, _)) = first
            .iter()
            .chain(second.iter())
            .find(|(_, known)| near(known))
    {
        return (*name).to_owned();
    }
    let byte = |value: f64| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    let mut name = format!(
        "#{:02x}{:02x}{:02x}",
        byte(rgb[0]),
        byte(rgb[1]),
        byte(rgb[2])
    );
    if opacity < 0.995 {
        name.push_str(&format!("{:02x}", byte(opacity)));
    }
    name
}

fn selector_data(segment: &Segment, kind: SelectorKind) -> Option<&Value> {
    segment
        .selectors
        .iter()
        .find(|selector| selector.kind == kind)
        .map(|selector| &selector.data)
}

fn quads_of(data: &Value) -> Vec<[f64; 8]> {
    data.get("quads")
        .and_then(Value::as_array)
        .map(|quads| {
            quads
                .iter()
                .filter_map(|quad| {
                    let values: Vec<f64> =
                        quad.as_array()?.iter().filter_map(Value::as_f64).collect();
                    values.try_into().ok()
                })
                .collect()
        })
        .unwrap_or_default()
}

fn bounds(quads: &[[f64; 8]]) -> [f64; 4] {
    let mut out = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for quad in quads {
        for corner in 0..4 {
            out[0] = out[0].min(quad[corner * 2]);
            out[1] = out[1].min(quad[corner * 2 + 1]);
            out[2] = out[2].max(quad[corner * 2]);
            out[3] = out[3].max(quad[corner * 2 + 1]);
        }
    }
    out
}

/// One mark of an annotation on one page, as it goes to a PDF.
enum Piece {
    /// Text marked: its quads.
    Quads(Vec<[f64; 8]>),
    Rect([f64; 4]),
    Stroke(Vec<f64>, f64),
    Point([f64; 4]),
}

/// What of a segment goes to a PDF, where it is now; nothing for an orphan.
fn piece_of(segment: &Segment) -> Option<(u32, Piece)> {
    if segment.anchor_state == AnchorState::Orphan {
        return None;
    }
    if let Some(found) = region::region_of(segment) {
        let quads: Vec<[f64; 8]> = found.quads.iter().map(|quad| quad.map(f64::from)).collect();
        let clip = found.clip.map(f64::from);
        let piece = match found.tool {
            RegionTool::Lines | RegionTool::Quads => Piece::Quads(quads),
            RegionTool::Rect => Piece::Rect(clip),
            RegionTool::Free => Piece::Stroke(
                found.path.iter().map(|value| f64::from(*value)).collect(),
                found.width.map_or(STROKE_PT, f64::from),
            ),
            RegionTool::Point => Piece::Point(clip),
        };
        return Some((found.page, piece));
    }
    let data = selector_data(segment, SelectorKind::PageQuads)?;
    let page = u32::try_from(data.get("page")?.as_u64()?).ok()?;
    let quads = quads_of(data);
    (!quads.is_empty()).then_some((page, Piece::Quads(quads)))
}

/// The PDF annotations an annotation becomes, and whether it had nothing to give (an orphan, a
/// note about the whole document).
pub fn pdf_annots_of(annotation: &Annotation, name: &str) -> Vec<Annot> {
    let mut pages: Vec<(u32, Vec<Piece>)> = Vec::new();
    for (page, piece) in annotation.segments.iter().filter_map(piece_of) {
        match pages.iter_mut().find(|(at, _)| *at == page) {
            Some((_, pieces)) => pieces.push(piece),
            None => pages.push((page, vec![piece])),
        }
    }
    let underline = annotation.kind == AnnotationKind::Underline;
    let (fill, opacity) = color_of_name(&annotation.color, false);
    let (ink, _) = color_of_name(&annotation.color, true);
    let mut out: Vec<Annot> = Vec::new();
    for (page, pieces) in pages {
        let mut quads = Vec::new();
        let mut strokes: Vec<(Vec<f64>, f64)> = Vec::new();
        let mut boxes = Vec::new();
        for piece in pieces {
            match piece {
                Piece::Quads(more) => quads.extend(more),
                Piece::Stroke(path, width) => strokes.push((path, width)),
                Piece::Rect(clip) => boxes.push((Subtype::Square, clip)),
                Piece::Point(clip) => boxes.push((Subtype::Text, clip)),
            }
        }
        let base = |subtype: Subtype, rect: [f64; 4]| Annot {
            page,
            subtype: Some(subtype),
            rect,
            color: Some(if subtype == Subtype::Underline {
                ink
            } else {
                fill
            }),
            opacity,
            subject: Some(subject_of(annotation.kind)),
            created: Some(annotation.created_at),
            modified: Some(annotation.updated_at),
            ..Annot::default()
        };
        if !quads.is_empty() {
            let subtype = if underline {
                Subtype::Underline
            } else {
                Subtype::Highlight
            };
            let rect = bounds(&quads);
            out.push(Annot {
                quads,
                ..base(subtype, rect)
            });
        }
        if !strokes.is_empty() {
            let width = strokes.iter().map(|(_, width)| *width).fold(0.0, f64::max);
            let ink: Vec<Vec<f64>> = strokes.into_iter().map(|(path, _)| path).collect();
            let all: Vec<[f64; 8]> = ink
                .iter()
                .flat_map(|path| {
                    path.as_chunks::<2>()
                        .0
                        .iter()
                        .map(|p| [p[0], p[1], p[0], p[1], p[0], p[1], p[0], p[1]])
                })
                .collect();
            out.push(Annot {
                ink,
                width: Some(width),
                ..base(Subtype::Ink, bounds(&all))
            });
        }
        for (subtype, clip) in boxes {
            let mut made = base(subtype, clip);
            if subtype == Subtype::Square {
                made.interior = made.color;
                made.width = Some(0.0);
            } else {
                made.icon = Some("Comment".to_owned());
            }
            out.push(made);
        }
    }
    for (index, made) in out.iter_mut().enumerate() {
        made.name = Some(if index == 0 {
            name.to_owned()
        } else {
            format!("{name}#{}", index + 1)
        });
        if index == 0 {
            made.contents = annotation.note.clone();
        }
    }
    out
}

/// The name an annotation goes to a PDF by: the `/NM` it came with from another program, so it
/// keeps its identity there too, or else its UUID.
pub fn export_name(annotation: &Annotation) -> &str {
    match annotation.pdf_nm.as_deref() {
        Some(name) if !name.starts_with("musaeum:") && !name.trim().is_empty() => name,
        _ => &annotation.uuid,
    }
}

/// The PDF annotations of the open document's annotations, and how many had none to give.
fn exported_annots(annotations: &[&Annotation]) -> (Vec<Annot>, usize, usize) {
    let mut annots = Vec::new();
    let mut left_out = 0;
    let mut exported = 0;
    for annotation in annotations {
        let made = pdf_annots_of(annotation, export_name(annotation));
        if made.is_empty() {
            left_out += 1;
        } else {
            exported += 1;
            annots.extend(made);
        }
    }
    (annots, exported, left_out)
}

/// A PDF annotation's name split into the UUID of the annotation it is part of and which part.
fn part_of(name: &str) -> Option<(&str, u32)> {
    let (base, part) = match name.split_once('#') {
        Some((base, part)) => (base, part.parse::<u32>().ok().filter(|part| *part >= 2)?),
        None => (name, 1),
    };
    uuid::Uuid::try_parse(base).ok().map(|_| (base, part))
}

/// The quads of a stroke, as `compositor/lines.ts` makes them: one band per step, `width` points
/// wide and squared off past each point. The path is in fractions of a frame of `w` by `h` points.
pub fn stroke_quads(path: &[f64], width: f64, w: f64, h: f64) -> Vec<[f32; 8]> {
    let half = width / 2.0;
    let points: Vec<(f64, f64)> = path
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| (p[0] * w, p[1] * h))
        .collect();
    let band = |(ax, ay): (f64, f64), (bx, by): (f64, f64)| -> [f32; 8] {
        let length = (bx - ax).hypot(by - ay);
        let (dx, dy) = if length > 0.0 {
            ((bx - ax) / length, (by - ay) / length)
        } else {
            (1.0, 0.0)
        };
        let (nx, ny) = (dy * half, -dx * half);
        let (sx, sy) = (ax - dx * half, ay - dy * half);
        let (ex, ey) = (bx + dx * half, by + dy * half);
        [
            sx + nx,
            sy + ny,
            ex + nx,
            ey + ny,
            ex - nx,
            ey - ny,
            sx - nx,
            sy - ny,
        ]
        .iter()
        .enumerate()
        .map(|(index, value)| (if index % 2 == 0 { value / w } else { value / h }) as f32)
        .collect::<Vec<f32>>()
        .try_into()
        .unwrap_or([0.0; 8])
    };
    match points.as_slice() {
        [] => Vec::new(),
        [only] => vec![band(*only, *only)],
        _ => {
            let quads: Vec<[f32; 8]> = points
                .windows(2)
                .filter(|pair| pair[0] != pair[1])
                .map(|pair| band(pair[0], pair[1]))
                .collect();
            if quads.is_empty() {
                vec![band(points[0], points[0])]
            } else {
                quads
            }
        }
    }
}

fn clip_of(quads: &[[f32; 8]]) -> [f32; 4] {
    let mut out = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for quad in quads {
        for corner in 0..4 {
            out[0] = out[0].min(quad[corner * 2]);
            out[1] = out[1].min(quad[corner * 2 + 1]);
            out[2] = out[2].max(quad[corner * 2]);
            out[3] = out[3].max(quad[corner * 2 + 1]);
        }
    }
    out.map(|value| value.clamp(0.0, 1.0))
}

/// The quad of a box on a page shown turned `rotation` quarter turns, as the reader draws one: from
/// its corner at the top left as shown, round as shown (`compositor/lines.ts`, `boxQuad`).
fn box_quad([left, top, right, bottom]: [f32; 4], rotation: u8) -> [f32; 8] {
    let corners = [[left, top], [right, top], [right, bottom], [left, bottom]];
    let start = usize::from((4 - rotation % 4) % 4);
    let mut quad = [0.0; 8];
    for (at, corner) in quad.as_chunks_mut::<2>().0.iter_mut().enumerate() {
        *corner = corners[(start + at) % 4];
    }
    quad
}

/// A point of a page as stored, as shown turned `rotation` quarter turns (`compositor/chars.ts`).
fn to_shown(u: f64, v: f64, rotation: u8) -> (f64, f64) {
    match rotation % 4 {
        1 => (1.0 - v, u),
        2 => (1.0 - u, 1.0 - v),
        3 => (v, 1.0 - u),
        _ => (u, v),
    }
}

/// A point of a page as shown turned `rotation` quarter turns, as stored (`compositor/lines.ts`).
fn to_stored(x: f64, y: f64, rotation: u8) -> (f64, f64) {
    match rotation % 4 {
        1 => (y, 1.0 - x),
        2 => (1.0 - x, 1.0 - y),
        3 => (1.0 - y, x),
        _ => (x, y),
    }
}

/// The quads of a stroke on a page of `frame`, as the reader makes them: banded as shown, on the
/// page as wide and as tall as it is shown, then stored. The path is as stored.
fn stroke_quads_on(path: &[f64], width: f64, frame: &Frame) -> Vec<[f32; 8]> {
    let rotation = frame.rotation;
    let shown: Vec<f64> = path
        .as_chunks::<2>()
        .0
        .iter()
        .flat_map(|point| {
            let (x, y) = to_shown(point[0], point[1], rotation);
            [x, y]
        })
        .collect();
    let (w, h) = if rotation % 2 == 1 {
        (frame.height(), frame.width())
    } else {
        (frame.width(), frame.height())
    };
    stroke_quads(&shown, width, w, h)
        .into_iter()
        .map(|quad| {
            let mut out = [0.0_f32; 8];
            for (corner, at) in out
                .as_chunks_mut::<2>()
                .0
                .iter_mut()
                .zip(quad.as_chunks::<2>().0)
            {
                let (u, v) = to_stored(f64::from(at[0]), f64::from(at[1]), rotation);
                *corner = [u as f32, v as f32];
            }
            out
        })
        .collect()
}

/// Whether quads a PDF gives are the quads the text layer makes of the characters under them,
/// near enough: then the mark is on that text, and anchored to it.
fn same_quads(layer: &[[f32; 8]], given: &[[f64; 8]]) -> bool {
    layer.len() == given.len()
        && layer.iter().zip(given).all(|(a, b)| {
            let height = f64::from((a[7] - a[1]).hypot(a[6] - a[0])).max(0.002);
            a.iter()
                .zip(b.iter())
                .all(|(x, y)| (f64::from(*x) - y).abs() <= height * 0.35)
        })
}

/// The pages of the open document, as the import needs them.
pub trait ImportPages {
    fn count(&self) -> u32;
    /// The page's characters, when it has a text layer.
    fn layer(&mut self, page: u32) -> Option<Arc<CharLayer>>;
    /// Whether the page is a picture, whose text the OCR reads rather than its layer.
    fn picture(&self, page: u32) -> bool;
    /// The hash of the page's picture, when the background has traced it.
    fn page_hash(&self, page: u32) -> Option<u64>;
}

/// What an import leaves out, by why.
pub type Skipped = BTreeMap<String, u32>;

fn skip(skipped: &mut Skipped, why: &str) {
    *skipped.entry(why.to_owned()).or_default() += 1;
}

/// A name for an annotation a PDF gives none: made from what it is, so the same one gets the same
/// name each time it is read.
fn made_name(annot: &Annot) -> String {
    let rounded = |value: f64| (value * 10_000.0).round() as i64;
    let mut hasher = blake3::Hasher::new();
    hasher.update(annot.subtype.map_or("", Subtype::name).as_bytes());
    hasher.update(&annot.page.to_le_bytes());
    for value in annot
        .rect
        .iter()
        .chain(annot.quads.iter().flatten())
        .chain(annot.ink.iter().flatten())
    {
        hasher.update(&rounded(*value).to_le_bytes());
    }
    hasher.update(annot.contents.as_bytes());
    let hash = hasher.finalize();
    format!("musaeum:{}", &hash.to_hex()[..32])
}

/// The parts of one annotation a PDF holds: what tells it apart, the UUID it came with, and each
/// part with its place among them.
type Group<'a> = (String, Option<String>, Vec<(u32, &'a Annot)>);

/// The annotations a PDF (or an XFDF file) holds, as annotations of the open document.
pub fn imported_from(
    annots: &[Annot],
    frames: &[Frame],
    pages: &mut dyn ImportPages,
    document: i64,
    revision: i64,
    format: FixedFormat,
    skipped: &mut Skipped,
) -> Vec<ImportedAnnotation> {
    // The parts of one of Musaeum's annotations go back together, in order.
    let mut groups: Vec<Group<'_>> = Vec::new();
    for annot in annots {
        if annot.reply {
            skip(skipped, "resposta");
            continue;
        }
        if annot.flags
            & (musaeum_pdf_annot::model::FLAG_HIDDEN | musaeum_pdf_annot::model::FLAG_NO_VIEW)
            != 0
        {
            skip(skipped, "oculta");
            continue;
        }
        if matches!(
            annot.subtype,
            Some(Subtype::StrikeOut | Subtype::Circle) | None
        ) {
            skip(
                skipped,
                annot.subtype.map_or("(desconhecida)", Subtype::name),
            );
            continue;
        }
        if annot.page >= pages.count() {
            skip(skipped, "página que o livro não tem");
            continue;
        }
        let (key, uuid, part) = match annot.name.as_deref().and_then(part_of) {
            Some((uuid, part)) => (uuid.to_owned(), Some(uuid.to_owned()), part),
            None => (
                annot.name.clone().unwrap_or_else(|| made_name(annot)),
                None,
                1,
            ),
        };
        match groups.iter_mut().find(|(known, _, _)| *known == key) {
            Some((_, _, parts)) => parts.push((part, annot)),
            None => groups.push((key, uuid, vec![(part, annot)])),
        }
    }
    let count = pages.count().max(1);
    let mut out = Vec::new();
    for (key, uuid, mut parts) in groups {
        parts.sort_by_key(|(part, _)| *part);
        let first = parts[0].1;
        let underline_subtype =
            matches!(first.subtype, Some(Subtype::Underline | Subtype::Squiggly));
        let kind = kind_of_subject(first.subject.as_deref()).unwrap_or(match first.subtype {
            Some(Subtype::Underline | Subtype::Squiggly) => AnnotationKind::Underline,
            Some(Subtype::Text | Subtype::FreeText) => AnnotationKind::Note,
            _ => AnnotationKind::Highlight,
        });
        let as_ink = underline_subtype || kind == AnnotationKind::Underline;
        let rgb = match first.subtype {
            Some(Subtype::Square) => first.interior.or(first.color),
            _ => first.color,
        };
        let color = rgb.map_or_else(
            || "yellow".to_owned(),
            |rgb| {
                name_of_color(
                    rgb,
                    first.opacity,
                    as_ink && first.subtype != Some(Subtype::Square),
                )
            },
        );
        let mut segments: Vec<Segment> = Vec::new();
        let mut quotes: Vec<String> = Vec::new();
        let mut broken = false;
        for (_, annot) in &parts {
            let page = annot.page;
            let frame = frames
                .get(page as usize)
                .copied()
                .unwrap_or_else(|| Frame::of(None, None, 0));
            let ordinal = segments.len() as u32;
            let region_segment = |tool: RegionTool,
                                  quads: Vec<[f32; 8]>,
                                  clip: [f32; 4],
                                  path: Vec<f32>,
                                  width: Option<f32>| Region {
                page,
                tool,
                quads,
                clip,
                lines: Vec::new(),
                path,
                width,
                page_hash: None,
                region_hash: None,
            };
            let mut regions: Vec<Region> = Vec::new();
            match annot.subtype {
                Some(Subtype::Highlight | Subtype::Underline | Subtype::Squiggly) => {
                    let layer = pages.layer(page).filter(|_| !pages.picture(page));
                    let quads32: Vec<[f32; 8]> = annot
                        .quads
                        .iter()
                        .map(|quad| quad.map(|v| v as f32))
                        .collect();
                    let on_text =
                        layer.as_ref().and_then(|layer| {
                            let (start, end) = chars_in_quads(&layer.chars, &quads32)?;
                            same_quads(&line_quads(&layer.chars, start, end), &annot.quads)
                                .then_some((Arc::clone(layer), start, end))
                        });
                    match on_text {
                        Some((layer, start, end)) => {
                            let (segment, quote) = fixed::segment_for(
                                ordinal,
                                fixed::PageRange { page, start, end },
                                &layer,
                                revision,
                            );
                            quotes.push(quote.exact);
                            segments.push(segment);
                        }
                        None => regions.push(region_segment(
                            RegionTool::Quads,
                            quads32.clone(),
                            clip_of(&quads32),
                            Vec::new(),
                            None,
                        )),
                    }
                }
                Some(Subtype::Square | Subtype::FreeText) => {
                    let clip = annot.rect.map(|v| v as f32);
                    regions.push(region_segment(
                        RegionTool::Rect,
                        vec![box_quad(clip, frame.rotation)],
                        clip,
                        Vec::new(),
                        None,
                    ));
                }
                Some(Subtype::Text) => {
                    let clip = annot.rect.map(|v| v as f32);
                    regions.push(region_segment(
                        RegionTool::Point,
                        vec![box_quad(clip, frame.rotation)],
                        clip,
                        Vec::new(),
                        None,
                    ));
                }
                Some(Subtype::Ink) => {
                    let width = annot.width.filter(|width| *width > 0.0).unwrap_or(1.0);
                    for stroke in &annot.ink {
                        let quads = stroke_quads_on(stroke, width, &frame);
                        let clip = clip_of(&quads);
                        regions.push(region_segment(
                            RegionTool::Free,
                            quads,
                            clip,
                            stroke.iter().map(|value| *value as f32).collect(),
                            (width != STROKE_PT).then_some(width as f32),
                        ));
                    }
                }
                _ => broken = true,
            }
            for mut found in regions {
                found.page_hash = pages.page_hash(page).map(region::hash_text);
                if found.problem().is_some() {
                    broken = true;
                    continue;
                }
                // The text under a mark on a page that is not a picture is its quote; on a
                // picture the OCR reads it, in the background.
                if !pages.picture(page)
                    && let Some(layer) = pages.layer(page)
                    && let Some((start, end)) = chars_in_quads(&layer.chars, &found.quads)
                {
                    let quote = fixed::quote_at(&layer, start, end).exact;
                    if !quote.trim().is_empty() {
                        quotes.push(quote);
                    }
                }
                segments.push(found.segment(segments.len() as u32, revision, LINES_VERSION));
            }
        }
        if broken || segments.is_empty() {
            skip(skipped, "(ilegível)");
            continue;
        }
        let progression = segments.first().and_then(|segment| {
            let (page, quads) = match region::region_of(segment) {
                Some(found) => (found.page, found.quads),
                None => {
                    let data = selector_data(segment, SelectorKind::PageQuads)?;
                    (
                        u32::try_from(data.get("page")?.as_u64()?).ok()?,
                        quads_of(data)
                            .iter()
                            .map(|quad| quad.map(|v| v as f32))
                            .collect(),
                    )
                }
            };
            let top = quads
                .first()
                .map_or(0.0, |quad| f64::from(quad[1]).clamp(0.0, 1.0));
            Some(((f64::from(page) + top) / f64::from(count)).clamp(0.0, 1.0))
        });
        let quote = (!quotes.is_empty()).then(|| quotes.join(" "));
        let quote_source = match (&quote, format) {
            (None, _) => QuoteSource::None,
            (Some(_), FixedFormat::Djvu) => QuoteSource::Ocr,
            (Some(_), _) => QuoteSource::TextLayer,
        };
        out.push(ImportedAnnotation {
            new: NewAnnotation {
                document_id: document,
                revision_id: revision,
                kind,
                color,
                note: first.contents.clone(),
                quote,
                quote_source,
                progression,
                anchor_state: AnchorState::Exact,
                resolved_version: Some(CHARS_VERSION),
                pdf_nm: Some(key),
                segments,
            },
            uuid,
            created_at: first.created,
            updated_at: first.modified.or(first.created),
        });
    }
    out
}

/// Whether an annotation the document has is, mark for mark, the one a PDF gives without a name:
/// a copy written by something that dropped the names.
fn same_as_existing(new: &ImportedAnnotation, existing: &[Annotation]) -> bool {
    let marks = |segments: &[Segment]| -> Vec<(u32, Vec<[f64; 8]>)> {
        segments
            .iter()
            .filter_map(|segment| {
                let (page, piece) = piece_of(segment)?;
                Some((
                    page,
                    match piece {
                        Piece::Quads(quads) => quads,
                        Piece::Rect(clip) | Piece::Point(clip) => {
                            let [l, t, r, b] = clip;
                            vec![[l, t, r, t, r, b, l, b]]
                        }
                        Piece::Stroke(path, _) => {
                            let points: Vec<[f64; 8]> = path
                                .as_chunks::<2>()
                                .0
                                .iter()
                                .map(|p| [p[0], p[1], p[0], p[1], p[0], p[1], p[0], p[1]])
                                .collect();
                            let [l, t, r, b] = bounds(&points);
                            vec![[l, t, r, t, r, b, l, b]]
                        }
                    },
                ))
            })
            .collect()
    };
    let wanted = marks(&new.new.segments);
    existing.iter().any(|annotation| {
        annotation.kind == new.new.kind && annotation.note == new.new.note && {
            let have = marks(&annotation.segments);
            have.len() == wanted.len()
                && have.iter().zip(&wanted).all(|((pa, qa), (pb, qb))| {
                    pa == pb
                        && qa.len() == qb.len()
                        && qa
                            .iter()
                            .flatten()
                            .zip(qb.iter().flatten())
                            .all(|(a, b)| (a - b).abs() <= 0.002)
                })
        }
    })
}

/// A file opened only to be read, as the library opens books: others may read, write and delete
/// it meanwhile, and nothing here ever writes to it.
pub(crate) fn open_to_read(path: &Path) -> io::Result<File> {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .open(path)
}

/// The volume and the file's index on it, which two paths to the same file share.
fn file_identity(path: &Path) -> Option<(u32, u64)> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let file = open_to_read(path).ok()?;
    // SAFETY: all zeroes is a valid value of this plain structure, filled in below.
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: the handle is open for the call and `info` is a valid out-pointer.
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } != 0;
    ok.then(|| {
        (
            info.dwVolumeSerialNumber,
            (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        )
    })
}

/// Refuses to write over the book, or over any file the library has.
fn check_target(library: &Library, source: &Path, target: &Path) -> Result<(), String> {
    let same_path = |a: &Path, b: &Path| {
        let canonical =
            |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        canonical(a).to_string_lossy().to_lowercase()
            == canonical(b).to_string_lossy().to_lowercase()
    };
    if same_path(source, target)
        || (target.exists()
            && file_identity(source).is_some()
            && file_identity(source) == file_identity(target))
    {
        return Err(tr!(CopyIsTheBook));
    }
    if target.exists() && matches!(library.opened_file(target), Ok(Some(_))) {
        return Err(tr!(CopyOverLibraryBook));
    }
    Ok(())
}

/// Writes `parts` into a new temporary file beside `target`, flushed to the disk, and returns it.
fn temporary_beside(target: &Path) -> Result<(PathBuf, File), String> {
    let folder = target.parent().ok_or_else(|| tr!(TargetHasNoFolder))?;
    let stem = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    for attempt in 0..16 {
        let name = format!(".{stem}.musaeum-{}-{attempt}.tmp", std::process::id());
        let path = folder.join(name);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(tr!(
                    CopyNotCreated,
                    folder = folder.display(),
                    error = error
                ));
            }
        }
    }
    Err(tr!(CopyTemporaryNamesInUse))
}

/// Copies `source` into `out` and returns its BLAKE3 and length.
fn copy_hashing(source: &mut File, out: &mut File) -> io::Result<(blake3::Hash, u64)> {
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0u8; 1 << 20];
    let mut total = 0u64;
    loop {
        let read = source.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        out.write_all(&buffer[..read])?;
        total += read as u64;
    }
    Ok((hasher.finalize(), total))
}

fn library(state: &AppState) -> Result<Arc<Library>, String> {
    state
        .shelf
        .library()
        .ok_or_else(|| "there is no library open".to_owned())
}

fn pdf_session(state: &AppState, session: u32) -> Result<Arc<Session>, String> {
    let session = state
        .session(session)
        .ok_or("that document is no longer open")?;
    if session.format != FixedFormat::Pdf {
        return Err(tr!(NativeAnnotationsPdfOnly));
    }
    Ok(session)
}

fn client() -> AnnotClient {
    AnnotClient::new(AnnotWorkerConfig::default())
}

/// The author written in `/T`: the person's name on this computer.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

/// The open document's annotations as they are now, found again if they need to be.
fn current_annotations(
    app: &AppHandle,
    state: &State<'_, AppState>,
    cache: &State<'_, CharCache>,
    session: &Session,
) -> Result<Vec<Annotation>, String> {
    let (document, _) = session.document.ok_or_else(|| tr!(DocumentNotInLibrary))?;
    // Listing them anchors them on the document as it is, and records where.
    fixed_annotations(app.clone(), state.clone(), cache.clone(), session.id)?;
    library(state)?
        .annotations(document, false)
        .map_err(|error| error.to_string())
}

/// Where a file the person chose goes, or comes from: the dialog's answer, or under a test the
/// path it gave.
pub(crate) fn chosen(
    state: &AppState,
    given: Option<String>,
    ask: impl FnOnce() -> Result<Option<PathBuf>, String>,
) -> Result<Option<PathBuf>, String> {
    match given {
        Some(path) if state.is_e2e() => Ok(Some(PathBuf::from(path))),
        Some(_) => Err("a path is only given under a test".to_owned()),
        None => ask(),
    }
}

/// What an export wrote.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportReport {
    path: String,
    /// The annotations of Musaeum written, and the PDF annotations they became.
    annotations: usize,
    written: usize,
    /// Annotations left out: orphans, and notes about the whole document.
    left_out: usize,
    /// Annotations the PDF already holds, which came from it: left as they are in the copy.
    already_in_pdf: usize,
    bytes: u64,
}

/// Writes a copy of the open PDF with its annotations in it, where the person chooses.
#[tauri::command(async)]
pub fn pdf_annotations_export(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    cache: State<'_, CharCache>,
    session: u32,
    target: Option<String>,
) -> Result<Option<ExportReport>, String> {
    let session = pdf_session(&state, session)?;
    let library = library(&state)?;
    let annotations = current_annotations(&app, &state, &cache, &session)?;
    if annotations.is_empty() {
        return Err(tr!(NoAnnotationsForPdf));
    }
    let source = session.path.clone();
    let suggested = tr!(
        NamePdfCopy,
        name = source
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default()
    );
    let Some(target) = chosen(&state, target, || {
        // Not the book's own folder: the library follows it, and nothing is written there.
        pick_save(
            owner_of(&window),
            &tr!(PickExportPdfCopy),
            PDF,
            None,
            &suggested,
        )
    })?
    else {
        return Ok(None);
    };
    check_target(&library, &source, &target)?;
    check_outside_library(&library, &target)?;

    let mut client = client();
    let mut book = open_to_read(&source).map_err(|error| tr!(BookNotRead, error = error))?;
    let before = client.read(&book, Reader::Auto)?;
    // What came from the book itself is in it already: it is not written again.
    let in_pdf: std::collections::HashSet<String> = before
        .reading
        .annots
        .iter()
        .map(|annot| annot.name.clone().unwrap_or_else(|| made_name(annot)))
        .collect();
    let (from_pdf, ours): (Vec<&Annotation>, Vec<&Annotation>) =
        annotations.iter().partition(|annotation| {
            annotation
                .pdf_nm
                .as_ref()
                .is_some_and(|name| in_pdf.contains(name))
                || in_pdf.contains(&annotation.uuid)
        });
    let (annots, exported, left_out) = exported_annots(&ours);
    if annots.is_empty() {
        return Err(if from_pdf.is_empty() {
            tr!(NoAnnotationOnAPage)
        } else {
            tr!(AnnotationsAlreadyInPdf)
        });
    }
    let update = client.export(
        &book,
        annots.clone(),
        WriteOptions {
            author: state.preferences.author(),
            now: now_ms(),
        },
    )?;
    let (temporary, mut out) = temporary_beside(&target)?;
    let written = (|| -> Result<u64, String> {
        let (hash, length) = copy_hashing(&mut book, &mut out)
            .map_err(|error| tr!(CopyNotWritten, error = error))?;
        if session
            .blake3
            .is_some_and(|known| known != *hash.as_bytes())
            || length != update.base
        {
            return Err(tr!(BookChangedWhileCopied));
        }
        out.write_all(&update.tail)
            .and_then(|()| out.sync_all())
            .map_err(|error| tr!(CopyNotWritten, error = error))?;
        drop(out);
        // PDFium reads the copy back: every annotation where it was put, and what was there before.
        let copy = open_to_read(&temporary).map_err(|error| error.to_string())?;
        let after = client.read(&copy, Reader::Pdfium)?;
        verify(&before, &after, &annots)?;
        Ok(length + update.tail.len() as u64)
    })();
    let bytes = match written {
        Ok(bytes) => bytes,
        Err(reason) => {
            let _ = std::fs::remove_file(&temporary);
            return Err(reason);
        }
    };
    if let Err(error) = std::fs::rename(&temporary, &target) {
        let _ = std::fs::remove_file(&temporary);
        return Err(tr!(CopyNotRenamed, error = error));
    }
    Ok(Some(ExportReport {
        path: target.to_string_lossy().into_owned(),
        annotations: exported,
        written: annots.len(),
        left_out,
        already_in_pdf: from_pdf.len(),
        bytes,
    }))
}

/// Whether a copy holds every annotation written, on its page, of its kind and where it was put,
/// and everything the book had.
fn verify(before: &Found, after: &Found, written: &[Annot]) -> Result<(), String> {
    if after.reading.frames.len() != before.reading.frames.len() {
        return Err(tr!(CopyLacksPages));
    }
    let mut by_name: HashMap<&str, Vec<&Annot>> = HashMap::new();
    for annot in &after.reading.annots {
        if let Some(name) = annot.name.as_deref() {
            by_name.entry(name).or_default().push(annot);
        }
    }
    for want in written {
        let name = want.name.as_deref().unwrap_or_default();
        let found = by_name.get(name).map(Vec::as_slice).unwrap_or_default();
        let [got] = found else {
            return Err(tr!(CopyLacksAnnotation, name = name));
        };
        let near = |a: &[f64], b: &[f64]| {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() <= 1e-3)
        };
        let same_place = got.page == want.page
            && got.subtype == want.subtype
            && near(&got.quads.concat(), &want.quads.concat())
            && near(&got.ink.concat(), &want.ink.concat())
            && (!matches!(want.subtype, Some(Subtype::Square | Subtype::Text))
                || near(&got.rect, &want.rect));
        if !same_place {
            return Err(tr!(CopyMovedAnnotation, name = name));
        }
    }
    for old in &before.reading.annots {
        let kept = after.reading.annots.iter().any(|annot| {
            annot.page == old.page
                && annot.subtype == old.subtype
                && annot.name == old.name
                && annot.contents == old.contents
        });
        if !kept {
            return Err(tr!(CopyLostAnnotation));
        }
    }
    Ok(())
}

/// What an import brought.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    path: String,
    added: usize,
    already: usize,
    skipped: Skipped,
    /// When an XFDF file says it is about another PDF.
    other_document: bool,
}

struct SessionImportPages<'a> {
    state: &'a AppState,
    library: &'a Library,
    session: &'a Session,
    cache: &'a CharCache,
}

impl ImportPages for SessionImportPages<'_> {
    fn count(&self) -> u32 {
        self.session.pool.pages().len() as u32
    }

    fn layer(&mut self, page: u32) -> Option<Arc<CharLayer>> {
        self.cache.get(self.session, page)
    }

    fn picture(&self, page: u32) -> bool {
        crate::ocr::picture_page(self.state, self.session, page) == Some(true)
    }

    fn page_hash(&self, page: u32) -> Option<u64> {
        let blake3 = self.session.blake3?;
        self.library
            .line_model(&blake3, page, LINES_VERSION)
            .ok()
            .flatten()
            .map(|record| record.page_hash)
    }
}

/// Whether the PDF a file's annotations were read from has the open book's pages: as many, each as
/// large, so the places mean the same.
fn same_pages(found: &Found, session: &Session) -> bool {
    let pages = session.pool.pages();
    found.reading.frames.len() == pages.len()
        && found.reading.frames.iter().zip(pages).all(|(frame, size)| {
            let (w, h) = if frame.rotation % 2 == 1 {
                (frame.height(), frame.width())
            } else {
                (frame.width(), frame.height())
            };
            (w - f64::from(size.width_pt)).abs() <= 1.0
                && (h - f64::from(size.height_pt)).abs() <= 1.0
        })
}

/// Brings what `found` holds into the open document.
fn import_found(
    app: &AppHandle,
    state: &State<'_, AppState>,
    cache: &State<'_, CharCache>,
    session: &Arc<Session>,
    found: &Found,
    path: &Path,
    other_document: bool,
) -> Result<ImportReport, String> {
    let (document, revision) = session.document.ok_or_else(|| tr!(DocumentNotInLibrary))?;
    let library = library(state)?;
    let mut skipped = found.reading.skipped.clone();
    for quiet in ["Popup", "Link", "Widget"] {
        skipped.remove(quiet);
    }
    let mut pages = SessionImportPages {
        state,
        library: &library,
        session,
        cache,
    };
    let imported = imported_from(
        &found.reading.annots,
        &found.reading.frames,
        &mut pages,
        document,
        revision,
        session.format,
        &mut skipped,
    );
    let existing = library
        .annotations(document, true)
        .map_err(|error| error.to_string())?;
    let mut already = 0;
    let fresh: Vec<ImportedAnnotation> = imported
        .into_iter()
        .filter(|item| {
            let named = item.uuid.is_some()
                || !item
                    .new
                    .pdf_nm
                    .as_deref()
                    .unwrap_or("")
                    .starts_with("musaeum:");
            if !named && same_as_existing(item, &existing) {
                already += 1;
                return false;
            }
            true
        })
        .collect();
    let outcome = library
        .import_annotations(document, revision, fresh)
        .map_err(|error| error.to_string())?;
    for annotation in &outcome.added {
        if awaits_quote(annotation) {
            state
                .ocr
                .enqueue(app, Arc::clone(session), annotation.uuid.clone());
        }
    }
    Ok(ImportReport {
        path: path.to_string_lossy().into_owned(),
        added: outcome.added.len(),
        already: already + outcome.already,
        skipped,
        other_document,
    })
}

/// Brings the annotations of a PDF the person chooses into the open book: the book itself, or a
/// copy of it annotated elsewhere. Those it already has, it keeps as they are.
#[tauri::command(async)]
pub fn pdf_annotations_import(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    cache: State<'_, CharCache>,
    session: u32,
    source: Option<String>,
) -> Result<Option<ImportReport>, String> {
    let session = pdf_session(&state, session)?;
    let Some(source) = chosen(&state, source, || {
        pick_file(
            owner_of(&window),
            &tr!(PickImportFromPdf),
            PDF,
            session.path.parent(),
        )
    })?
    else {
        return Ok(None);
    };
    let file = open_to_read(&source).map_err(|error| tr!(PdfNotRead, error = error))?;
    let found = client().read(&file, Reader::Auto)?;
    if !same_pages(&found, &session) {
        return Err(tr!(PdfOtherPages));
    }
    import_found(&app, &state, &cache, &session, &found, &source, false).map(Some)
}

/// Writes the open PDF's annotations as an XFDF file, where the person chooses.
#[tauri::command(async)]
pub fn xfdf_export(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    cache: State<'_, CharCache>,
    session: u32,
    target: Option<String>,
) -> Result<Option<ExportReport>, String> {
    let session = pdf_session(&state, session)?;
    let library = library(&state)?;
    let annotations = current_annotations(&app, &state, &cache, &session)?;
    let (mut annots, exported, left_out) = exported_annots(&annotations.iter().collect::<Vec<_>>());
    if annots.is_empty() {
        return Err(tr!(NoAnnotationsForXfdf));
    }
    // Signed as in a PDF copy: another program's keep their own author.
    let author = state.preferences.author();
    for annot in &mut annots {
        if annot.author.is_none() {
            annot.author.clone_from(&author);
        }
    }
    let source = session.path.clone();
    let href = source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned());
    let suggested = format!(
        "{}.xfdf",
        source
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default()
    );
    let Some(target) = chosen(&state, target, || {
        pick_save(
            owner_of(&window),
            &tr!(PickExportXfdf),
            XFDF,
            None,
            &suggested,
        )
    })?
    else {
        return Ok(None);
    };
    check_outside_library(&library, &target)?;
    let book = open_to_read(&source).map_err(|error| tr!(BookNotRead, error = error))?;
    let bytes = client().write_xfdf(&book, annots.clone(), href)?;
    let (temporary, mut out) = temporary_beside(&target)?;
    let done = out
        .write_all(&bytes)
        .and_then(|()| out.sync_all())
        .map_err(|error| tr!(XfdfNotWritten, error = error));
    drop(out);
    if let Err(reason) = done.and_then(|()| {
        std::fs::rename(&temporary, &target).map_err(|error| tr!(XfdfNotRenamed, error = error))
    }) {
        let _ = std::fs::remove_file(&temporary);
        return Err(reason);
    }
    Ok(Some(ExportReport {
        path: target.to_string_lossy().into_owned(),
        annotations: exported,
        written: annots.len(),
        left_out,
        already_in_pdf: 0,
        bytes: bytes.len() as u64,
    }))
}

/// Brings the annotations of an XFDF file the person chooses into the open PDF.
#[tauri::command(async)]
pub fn xfdf_import(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    cache: State<'_, CharCache>,
    session: u32,
    source: Option<String>,
) -> Result<Option<ImportReport>, String> {
    let session = pdf_session(&state, session)?;
    let Some(source) = chosen(&state, source, || {
        pick_file(
            owner_of(&window),
            &tr!(PickImportXfdf),
            XFDF,
            session.path.parent(),
        )
    })?
    else {
        return Ok(None);
    };
    let mut xfdf = Vec::new();
    open_to_read(&source)
        .and_then(|file| file.take(MAX_XFDF + 1).read_to_end(&mut xfdf))
        .map_err(|error| tr!(XfdfNotRead, error = error))?;
    if xfdf.len() as u64 > MAX_XFDF {
        return Err(tr!(XfdfTooLarge));
    }
    let book = open_to_read(&session.path).map_err(|error| tr!(BookNotRead, error = error))?;
    let found = client().read_xfdf(&book, &xfdf)?;
    let other_document = match (&found.xfdf_ids, &found.id) {
        (Some((theirs, _)), Some((ours, _))) => !theirs.eq_ignore_ascii_case(ours),
        _ => false,
    };
    import_found(
        &app,
        &state,
        &cache,
        &session,
        &found,
        &source,
        other_document,
    )
    .map(Some)
}

/// What PDFium reads in a file, for the window tests: the objects of a copy as another reader sees
/// them.
#[tauri::command(async)]
pub fn e2e_pdf_annotations(state: State<'_, AppState>, path: String) -> Result<Found, String> {
    if !state.is_e2e() {
        return Err("only under a test".to_owned());
    }
    let file = open_to_read(Path::new(&path)).map_err(|error| error.to_string())?;
    client().read(&file, Reader::Pdfium)
}

#[cfg(test)]
mod tests {
    use super::*;
    use musaeum_library::Selector;
    use serde_json::json;

    #[test]
    fn colors_go_out_as_drawn_and_come_back_as_named() {
        for (name, fill) in PALETTE {
            assert_eq!(color_of_name(name, false), (fill, 1.0));
            assert_eq!(name_of_color(fill, 1.0, false), name);
        }
        for (name, ink) in INK {
            assert_eq!(color_of_name(name, true).0, ink);
            assert_eq!(name_of_color(ink, 1.0, true), name);
        }
        // Through XFDF's #RRGGBB, a step of 255 away at most.
        let stepped = PALETTE[0].1.map(|value| (value * 255.0).round() / 255.0);
        assert_eq!(name_of_color(stepped, 1.0, false), "yellow");
        assert_eq!(name_of_color([1.0, 0.0, 0.0], 1.0, false), "#ff0000");
        assert_eq!(name_of_color([1.0, 0.0, 0.0], 0.5, false), "#ff000080");
        assert_eq!(
            color_of_name("#ff000080", false),
            ([1.0, 0.0, 0.0], 128.0 / 255.0)
        );
        assert_eq!(color_of_name("nonsense", false), (PALETTE[0].1, 1.0));
    }

    fn annotation(kind: AnnotationKind, segments: Vec<Segment>) -> Annotation {
        Annotation {
            id: 1,
            uuid: "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b".into(),
            document_id: 1,
            revision_id: 1,
            kind,
            color: "blue".into(),
            note: "uma nota".into(),
            quote: None,
            quote_source: QuoteSource::None,
            progression: None,
            anchor_state: AnchorState::Exact,
            resolved_revision_id: Some(1),
            resolved_version: Some(CHARS_VERSION),
            pdf_nm: None,
            created_at: 1_700_000_000_000,
            updated_at: 1_700_000_100_000,
            deleted_at: None,
            segments,
        }
    }

    fn text_segment(ordinal: u32, page: u32, quads: Vec<[f64; 8]>) -> Segment {
        Segment {
            ordinal,
            page: Some(page),
            href: None,
            clip: None,
            anchor_state: AnchorState::Exact,
            selectors: vec![Selector {
                kind: SelectorKind::PageQuads,
                data: json!({ "page": page, "quads": quads }),
                priority: 1,
                normalizer_version: Some(CHARS_VERSION),
                revision_id: Some(1),
            }],
        }
    }

    #[test]
    fn an_annotation_over_two_pages_goes_as_two_named_parts() {
        let quad = [0.1, 0.1, 0.4, 0.1, 0.4, 0.12, 0.1, 0.12];
        let two = annotation(
            AnnotationKind::Underline,
            vec![
                text_segment(0, 3, vec![quad]),
                text_segment(1, 4, vec![quad, quad]),
            ],
        );
        let made = pdf_annots_of(&two, export_name(&two));
        assert_eq!(made.len(), 2);
        assert_eq!(
            made[0].name.as_deref(),
            Some("0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b")
        );
        assert_eq!(
            made[1].name.as_deref(),
            Some("0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b#2")
        );
        assert_eq!(made[0].subtype, Some(Subtype::Underline));
        assert_eq!(
            made[0].color,
            Some(INK[2].1),
            "an underline goes as its ink"
        );
        assert_eq!(made[0].subject.as_deref(), Some("Sublinhado"));
        assert_eq!(made[0].contents, "uma nota");
        assert_eq!(made[1].contents, "", "the note goes once");
        assert_eq!(made[1].quads.len(), 2);
        assert_eq!(
            part_of(made[1].name.as_deref().unwrap()),
            Some(("0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b", 2))
        );
        assert_eq!(part_of("acrobat-1#2"), None);

        let mut orphan = text_segment(0, 3, vec![quad]);
        orphan.anchor_state = AnchorState::Orphan;
        assert!(
            pdf_annots_of(&annotation(AnnotationKind::Highlight, vec![orphan]), "x").is_empty()
        );
        assert!(pdf_annots_of(&annotation(AnnotationKind::Note, Vec::new()), "x").is_empty());
        // One that came from another program goes back by the name it came with.
        let mut foreign = annotation(AnnotationKind::Highlight, Vec::new());
        foreign.pdf_nm = Some("acrobat-0001".into());
        assert_eq!(export_name(&foreign), "acrobat-0001");
        foreign.pdf_nm = Some("musaeum:00ff".into());
        assert_eq!(export_name(&foreign), foreign.uuid);
    }

    #[test]
    fn a_stroke_has_the_quads_the_reader_draws() {
        let quads = stroke_quads(&[0.1, 0.1, 0.2, 0.1], 9.0, 600.0, 800.0);
        assert_eq!(quads.len(), 1);
        let q = quads[0];
        // From 4.5 points before the first point to 4.5 after the last, 9 points across.
        assert!((f64::from(q[0]) - (60.0 - 4.5) / 600.0).abs() < 1e-6);
        assert!((f64::from(q[2]) - (120.0 + 4.5) / 600.0).abs() < 1e-6);
        assert!((f64::from(q[5] - q[3]) * 800.0 - 9.0).abs() < 1e-3);
        assert_eq!(
            stroke_quads(&[0.5, 0.5], 9.0, 600.0, 800.0).len(),
            1,
            "a dot"
        );
    }

    /// A box or a stroke read from a PDF has the quads the reader would have stored had it been
    /// drawn in the window on that page as shown, whichever way the page is turned: corner for
    /// corner, so a round trip gives back the very region.
    #[test]
    fn a_box_and_a_stroke_come_back_as_drawn_on_a_turned_page() {
        let close = |a: &[f32], b: &[f32]| {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)
        };
        for rotation in 0..4u8 {
            let frame = Frame::of(
                Some([0.0, 0.0, 595.0, 842.0]),
                None,
                i64::from(rotation) * 90,
            );
            // As `boxQuad` and `storedQuads` make it in the window.
            let shown = [0.2, 0.3, 0.6, 0.45];
            let drawn: Vec<f32> = [
                (shown[0], shown[1]),
                (shown[2], shown[1]),
                (shown[2], shown[3]),
                (shown[0], shown[3]),
            ]
            .into_iter()
            .flat_map(|(x, y)| {
                let (u, v) = to_stored(x, y, rotation);
                [u as f32, v as f32]
            })
            .collect();
            let clip = clip_of(&[drawn.clone().try_into().unwrap()]);
            assert!(
                close(&box_quad(clip, rotation), &drawn),
                "a box turned {rotation}"
            );

            // As `strokeQuads` on the page as shown, then stored.
            let path_shown = [0.1, 0.2, 0.3, 0.25, 0.35, 0.5];
            let (w, h) = if rotation % 2 == 1 {
                (842.0, 595.0)
            } else {
                (595.0, 842.0)
            };
            let made: Vec<f32> = stroke_quads(&path_shown, 9.0, w, h)
                .iter()
                .flat_map(|quad| {
                    quad.as_chunks::<2>().0.iter().flat_map(|at| {
                        let (u, v) = to_stored(f64::from(at[0]), f64::from(at[1]), rotation);
                        [u as f32, v as f32]
                    })
                })
                .collect();
            let path: Vec<f64> = path_shown
                .as_chunks::<2>()
                .0
                .iter()
                .flat_map(|at| {
                    let (u, v) = to_stored(at[0], at[1], rotation);
                    [u, v]
                })
                .collect();
            let read: Vec<f32> = stroke_quads_on(&path, 9.0, &frame)
                .iter()
                .flatten()
                .copied()
                .collect();
            assert!(close(&read, &made), "a stroke turned {rotation}");
        }
    }
}

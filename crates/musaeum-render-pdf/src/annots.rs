//! A PDF's annotations as PDFium reads them, for the annotations worker.
//!
//! PDFium reads every file the reader shows, encrypted ones included, so it reads the annotations
//! of a file whose objects this crate's own reader cannot, and it is what checks a copy Musaeum
//! wrote: an annotation is there when PDFium finds it. Each page's frame is the box PDFium shows
//! (`FPDF_GetPageBoundingBox`), the one the render workers measure characters in.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{c_float, c_uint, c_ulong};
use std::ptr;

use musaeum_pdf_annot::model::{
    Annot, Frame, MAX_CONTENTS, MAX_NAME, MAX_QUADS, MAX_STROKE_POINTS, MAX_STROKES, Subtype,
};
use musaeum_pdf_annot::read::{MAX_PAGE_ANNOTATIONS, Reading, ours_from_quad_points};
use musaeum_pdf_annot::text::parse_date;
use pdfium_render::prelude::{
    FPDF_ANNOTATION, FPDF_DOCUMENT, FPDF_PAGE, FS_POINTF, FS_QUADPOINTSF, FS_RECTF,
    PdfiumLibraryBindings,
};

use crate::engine::Pdfium;

/// `FPDF_ANNOT_APPEARANCEMODE_NORMAL` and the two `FPDFANNOT_COLORTYPE`s.
const APPEARANCE_NORMAL: i32 = 0;
const COLOR: u32 = 0;
const INTERIOR_COLOR: u32 = 1;
/// `FPDF_FILEIDTYPE`s.
const FILE_ID_PERMANENT: u32 = 0;
const FILE_ID_CHANGING: u32 = 1;

/// The PDF names of `FPDF_ANNOTATION_SUBTYPE`s, in their order.
const SUBTYPE_NAMES: [&str; 29] = [
    "(unknown)",
    "Text",
    "Link",
    "FreeText",
    "Line",
    "Square",
    "Circle",
    "Polygon",
    "PolyLine",
    "Highlight",
    "Underline",
    "Squiggly",
    "StrikeOut",
    "Stamp",
    "Caret",
    "Ink",
    "Popup",
    "FileAttachment",
    "Sound",
    "Movie",
    "Widget",
    "Screen",
    "PrinterMark",
    "TrapNet",
    "Watermark",
    "3D",
    "RichMedia",
    "XFAWidget",
    "Redact",
];

/// A document PDFium opened from bytes in memory.
pub struct AnnotDocument<'lib, 'data> {
    pdfium: &'lib Pdfium,
    handle: FPDF_DOCUMENT,
    _data: &'data [u8],
}

impl<'lib, 'data> AnnotDocument<'lib, 'data> {
    /// Opens `data`, or says why PDFium would not.
    pub fn open(pdfium: &'lib Pdfium, data: &'data [u8]) -> Result<Self, String> {
        let bindings = pdfium.bindings();
        // SAFETY: `data` outlives the document thanks to `'data`.
        let handle = unsafe { bindings.FPDF_LoadMemDocument64(data, None) };
        if handle.is_null() {
            // SAFETY: the library is initialized.
            let error = unsafe { bindings.FPDF_GetLastError() };
            return Err(match error {
                3 => "o PDFium não consegue abrir o arquivo: ele está danificado".to_owned(),
                4 => "o PDF pede uma senha para ser aberto".to_owned(),
                other => format!("o PDFium não consegue abrir o arquivo (erro {other})"),
            });
        }
        Ok(Self {
            pdfium,
            handle,
            _data: data,
        })
    }

    fn bindings(&self) -> &dyn PdfiumLibraryBindings {
        self.pdfium.bindings()
    }

    pub fn page_count(&self) -> u32 {
        // SAFETY: the document is open.
        unsafe { self.bindings().FPDF_GetPageCount(self.handle) }.max(0) as u32
    }

    /// The file's identifiers, as hex: the permanent one and the changing one.
    pub fn file_id(&self) -> Option<(String, String)> {
        let read = |kind: u32| -> Option<String> {
            let bindings = self.bindings();
            // SAFETY: the document is open; a null buffer asks for the length.
            let length = unsafe {
                bindings.FPDF_GetFileIdentifier(self.handle, kind as _, ptr::null_mut(), 0)
            };
            if length <= 1 || length > 1024 {
                return None;
            }
            let mut buffer = vec![0u8; length as usize];
            // SAFETY: `buffer` holds `length` bytes, as PDFium asked.
            unsafe {
                bindings.FPDF_GetFileIdentifier(
                    self.handle,
                    kind as _,
                    buffer.as_mut_ptr().cast(),
                    length,
                )
            };
            // The identifier is a byte string, followed by a terminating zero.
            buffer.pop();
            Some(musaeum_pdf_annot::protocol::hex(&buffer))
        };
        Some((read(FILE_ID_PERMANENT)?, read(FILE_ID_CHANGING)?))
    }

    /// Runs `visit` on page `index`, loaded for it alone.
    fn with_page<T>(&self, index: u32, visit: impl FnOnce(FPDF_PAGE) -> T) -> Option<T> {
        // SAFETY: the document is open; an index past the end gives null.
        let page = unsafe { self.bindings().FPDF_LoadPage(self.handle, index as i32) };
        if page.is_null() {
            return None;
        }
        let result = visit(page);
        // SAFETY: loaded above, used by nothing else.
        unsafe { self.bindings().FPDF_ClosePage(page) };
        Some(result)
    }

    fn frame_of(&self, page: FPDF_PAGE) -> Frame {
        let bindings = self.bindings();
        let mut rect = FS_RECTF {
            left: 0.0,
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
        };
        // SAFETY: the page is open and `rect` is a valid out-pointer.
        let ok = unsafe { bindings.FPDF_GetPageBoundingBox(page, &mut rect) } != 0;
        // SAFETY: the page is open.
        let rotation = unsafe { bindings.FPDFPage_GetRotation(page) }.rem_euclid(4) as u8;
        if !ok {
            return Frame::of(None, Some([0.0; 4]), 0);
        }
        Frame {
            left: f64::from(rect.left.min(rect.right)),
            bottom: f64::from(rect.bottom.min(rect.top)),
            right: f64::from(rect.left.max(rect.right)),
            top: f64::from(rect.bottom.max(rect.top)),
            rotation,
        }
    }

    /// The frames of `pages`.
    pub fn frames(&self, pages: &BTreeSet<u32>) -> BTreeMap<u32, Frame> {
        pages
            .iter()
            .filter_map(|&index| Some((index, self.with_page(index, |page| self.frame_of(page))?)))
            .collect()
    }

    /// Every page's frame and annotations, as PDFium reads them, and how many annotations of every
    /// kind each page has. `only` limits the pages loaded; the frames of the others are
    /// `fallback`'s, and their counts empty.
    pub fn read(
        &self,
        only: Option<&BTreeSet<u32>>,
        fallback: &[Frame],
    ) -> (Reading, Vec<BTreeMap<String, u32>>) {
        let count = self.page_count();
        let mut reading = Reading::default();
        let mut counts = Vec::with_capacity(count as usize);
        for index in 0..count {
            let wanted = only.is_none_or(|only| only.contains(&index));
            let looked = wanted.then(|| {
                self.with_page(index, |page| {
                    let frame = self.frame_of(page);
                    (
                        frame,
                        self.page_annotations(page, index, &frame, &mut reading.skipped),
                    )
                })
            });
            match looked.flatten() {
                Some((frame, (annots, kinds))) => {
                    reading.frames.push(frame);
                    reading.annots.extend(annots);
                    counts.push(kinds);
                }
                None => {
                    reading.frames.push(
                        fallback
                            .get(index as usize)
                            .copied()
                            .unwrap_or_else(|| Frame::of(None, None, 0)),
                    );
                    counts.push(BTreeMap::new());
                }
            }
        }
        reading.skipped.retain(|_, count| *count > 0);
        (reading, counts)
    }

    /// The annotations of a page, and how many of every kind it has.
    fn page_annotations(
        &self,
        page: FPDF_PAGE,
        index: u32,
        frame: &Frame,
        skipped: &mut BTreeMap<String, u32>,
    ) -> (Vec<Annot>, BTreeMap<String, u32>) {
        let bindings = self.bindings();
        // SAFETY: the page is open.
        let count = unsafe { bindings.FPDFPage_GetAnnotCount(page) }.max(0) as usize;
        let mut annots = Vec::new();
        let mut kinds: BTreeMap<String, u32> = BTreeMap::new();
        for at in 0..count.min(MAX_PAGE_ANNOTATIONS) {
            // SAFETY: `at` is below the count PDFium gave.
            let annot = unsafe { bindings.FPDFPage_GetAnnot(page, at as i32) };
            if annot.is_null() {
                *skipped.entry("(broken)".to_owned()).or_default() += 1;
                continue;
            }
            // SAFETY: the annotation is open.
            let code = unsafe { bindings.FPDFAnnot_GetSubtype(annot) };
            let name = SUBTYPE_NAMES
                .get(usize::try_from(code).unwrap_or(0))
                .copied()
                .unwrap_or("(unknown)");
            *kinds.entry(name.to_owned()).or_default() += 1;
            match Subtype::from_name(name.as_bytes()) {
                Some(subtype) => match self.annotation(annot, subtype, index) {
                    Some(found) if frame.is_usable() => annots.push(found.to_uv(frame)),
                    _ => *skipped.entry("(broken)".to_owned()).or_default() += 1,
                },
                None => *skipped.entry(name.to_owned()).or_default() += 1,
            }
            // SAFETY: opened above, used by nothing else.
            unsafe { bindings.FPDFPage_CloseAnnot(annot) };
        }
        (annots, kinds)
    }

    fn string(&self, annot: FPDF_ANNOTATION, key: &str, limit: usize) -> Option<String> {
        let bindings = self.bindings();
        // SAFETY: the annotation is open; a null buffer asks for the length, in bytes.
        let has = unsafe { bindings.FPDFAnnot_HasKey(annot, key) } != 0;
        if !has {
            return None;
        }
        // SAFETY: as above.
        let length = unsafe { bindings.FPDFAnnot_GetStringValue(annot, key, ptr::null_mut(), 0) };
        if length < 2 || length as usize > (MAX_CONTENTS + 1) * 4 {
            return None;
        }
        let mut buffer = vec![0u16; (length as usize).div_ceil(2)];
        // SAFETY: `buffer` holds at least `length` bytes.
        unsafe { bindings.FPDFAnnot_GetStringValue(annot, key, buffer.as_mut_ptr(), length) };
        while buffer.last() == Some(&0) {
            buffer.pop();
        }
        Some(
            String::from_utf16_lossy(&buffer)
                .chars()
                .take(limit)
                .collect(),
        )
    }

    fn number(&self, annot: FPDF_ANNOTATION, key: &str) -> Option<f64> {
        let mut value: c_float = 0.0;
        // SAFETY: the annotation is open and `value` a valid out-pointer.
        let ok = unsafe {
            self.bindings()
                .FPDFAnnot_GetNumberValue(annot, key, &mut value)
        } != 0;
        (ok && value.is_finite()).then_some(f64::from(value))
    }

    /// `/C` or `/IC`. PDFium gives no color for an annotation with an appearance, so the
    /// appearance is dropped from this copy of the document first, which nothing ever writes.
    fn color(&self, annot: FPDF_ANNOTATION, key: &str, kind: u32) -> Option<[f64; 3]> {
        let bindings = self.bindings();
        // SAFETY: the annotation is open.
        if unsafe { bindings.FPDFAnnot_HasKey(annot, key) } == 0 {
            return None;
        }
        // SAFETY: the annotation is open; a null value removes its appearance in memory.
        unsafe { bindings.FPDFAnnot_SetAP(annot, APPEARANCE_NORMAL, ptr::null()) };
        let (mut r, mut g, mut b, mut a): (c_uint, c_uint, c_uint, c_uint) = (0, 0, 0, 0);
        // SAFETY: the annotation is open and the four are valid out-pointers.
        let ok = unsafe {
            bindings.FPDFAnnot_GetColor(annot, kind as _, &mut r, &mut g, &mut b, &mut a)
        } != 0;
        ok.then(|| [r, g, b].map(|channel| f64::from(channel.min(255)) / 255.0))
    }

    fn annotation(&self, annot: FPDF_ANNOTATION, subtype: Subtype, page: u32) -> Option<Annot> {
        let bindings = self.bindings();
        let mut rect = FS_RECTF {
            left: 0.0,
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
        };
        // SAFETY: the annotation is open and `rect` a valid out-pointer.
        if unsafe { bindings.FPDFAnnot_GetRect(annot, &mut rect) } == 0 {
            return None;
        }
        let (l, r) = (f64::from(rect.left), f64::from(rect.right));
        let (b, t) = (f64::from(rect.bottom), f64::from(rect.top));
        let mut found = Annot {
            page,
            subtype: Some(subtype),
            rect: [l.min(r), b.min(t), l.max(r), b.max(t)],
            contents: self
                .string(annot, "Contents", MAX_CONTENTS)
                .unwrap_or_default(),
            name: self.string(annot, "NM", MAX_NAME),
            author: self.string(annot, "T", MAX_NAME),
            subject: self.string(annot, "Subj", MAX_NAME),
            created: self
                .string(annot, "CreationDate", 64)
                .and_then(|date| parse_date(&date)),
            modified: self
                .string(annot, "M", 64)
                .and_then(|date| parse_date(&date)),
            opacity: self
                .number(annot, "CA")
                .map_or(1.0, |value| value.clamp(0.0, 1.0)),
            // SAFETY: the annotation is open.
            flags: unsafe { bindings.FPDFAnnot_GetFlags(annot) }.max(0) as u32,
            // SAFETY: as above.
            reply: unsafe { bindings.FPDFAnnot_HasKey(annot, "IRT") } != 0,
            icon: if subtype == Subtype::Text {
                self.string(annot, "Name", MAX_NAME)
            } else {
                None
            },
            ..Annot::default()
        };
        let (mut horizontal, mut vertical, mut width): (c_float, c_float, c_float) =
            (0.0, 0.0, 0.0);
        // SAFETY: the annotation is open and the three are valid out-pointers. PDFium reads only
        // `/Border`; `/BS` is not within its reach.
        if unsafe {
            bindings.FPDFAnnot_GetBorder(annot, &mut horizontal, &mut vertical, &mut width)
        } != 0
            && width.is_finite()
            && width >= 0.0
        {
            found.width = Some(f64::from(width));
        }
        if subtype.is_markup() {
            // SAFETY: the annotation is open.
            let count = unsafe { bindings.FPDFAnnot_CountAttachmentPoints(annot) };
            if count == 0 || count > MAX_QUADS {
                return None;
            }
            for at in 0..count {
                let mut points = FS_QUADPOINTSF {
                    x1: 0.0,
                    y1: 0.0,
                    x2: 0.0,
                    y2: 0.0,
                    x3: 0.0,
                    y3: 0.0,
                    x4: 0.0,
                    y4: 0.0,
                };
                // SAFETY: `at` is below the count, and `points` a valid out-pointer.
                if unsafe { bindings.FPDFAnnot_GetAttachmentPoints(annot, at as _, &mut points) }
                    == 0
                {
                    return None;
                }
                let raw = [
                    points.x1, points.y1, points.x2, points.y2, points.x3, points.y3, points.x4,
                    points.y4,
                ]
                .map(f64::from);
                found.quads.push(ours_from_quad_points(raw));
            }
        }
        if subtype == Subtype::Ink {
            // SAFETY: the annotation is open.
            let strokes = unsafe { bindings.FPDFAnnot_GetInkListCount(annot) } as usize;
            if strokes == 0 || strokes > MAX_STROKES {
                return None;
            }
            for stroke in 0..strokes {
                // SAFETY: `stroke` is below the count; a null buffer asks for the length.
                let length = unsafe {
                    bindings.FPDFAnnot_GetInkListPath(annot, stroke as c_ulong, ptr::null_mut(), 0)
                } as usize;
                if length == 0 || length > MAX_STROKE_POINTS {
                    return None;
                }
                let mut points = vec![FS_POINTF { x: 0.0, y: 0.0 }; length];
                // SAFETY: `points` holds `length` points, as PDFium asked.
                unsafe {
                    bindings.FPDFAnnot_GetInkListPath(
                        annot,
                        stroke as c_ulong,
                        points.as_mut_ptr(),
                        length as c_ulong,
                    )
                };
                found.ink.push(
                    points
                        .iter()
                        .flat_map(|point| [f64::from(point.x), f64::from(point.y)])
                        .collect(),
                );
            }
        }
        // Last: reading a color drops the appearance from PDFium's copy in memory.
        found.color = self.color(annot, "C", COLOR);
        found.interior = self.color(annot, "IC", INTERIOR_COLOR);
        Some(found)
    }
}

impl Drop for AnnotDocument<'_, '_> {
    fn drop(&mut self) {
        // SAFETY: every page and annotation was closed after use, so the document can go.
        unsafe { self.pdfium.bindings().FPDF_CloseDocument(self.handle) };
    }
}

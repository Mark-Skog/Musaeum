//! What an annotation of a PDF is, as it travels between the host and the worker, and the frame
//! its places are measured in.
//!
//! Places are fractions of the page's visible box before its rotation (the CropBox clipped to the
//! MediaBox, as PDFium has it), `u` from its left edge and `v` from its top: the frame in which the
//! render worker measures characters and the library keeps annotations, so a place goes from one to
//! the other unchanged. A quad's corners go top left, top right, bottom right, bottom left, in the
//! order of the text it covers, whatever the page's `/Rotate`.

use serde::{Deserialize, Serialize};

/// The page as a reader shows it: the CropBox clipped to the MediaBox, in default user space, and
/// the quarter turns clockwise it is shown at.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub left: f64,
    pub bottom: f64,
    pub right: f64,
    pub top: f64,
    pub rotation: u8,
}

fn normalized([a, b, c, d]: [f64; 4]) -> [f64; 4] {
    [a.min(c), b.min(d), a.max(c), b.max(d)]
}

fn is_empty([left, bottom, right, top]: [f64; 4]) -> bool {
    left >= right || bottom >= top
}

impl Frame {
    /// The frame PDFium works out (`CPDF_Page::UpdateDimensions`): the MediaBox, or US Letter when
    /// it has none; the CropBox clipped to it when there is one; `/Rotate` in quarter turns, the
    /// rest of a division by 90 dropped.
    pub fn of(media_box: Option<[f64; 4]>, crop_box: Option<[f64; 4]>, rotate: i64) -> Self {
        let media = media_box
            .map(normalized)
            .filter(|rect| !is_empty(*rect))
            .unwrap_or([0.0, 0.0, 612.0, 792.0]);
        let shown = match crop_box.map(normalized).filter(|rect| !is_empty(*rect)) {
            Some(crop) => {
                let clipped = [
                    crop[0].max(media[0]),
                    crop[1].max(media[1]),
                    crop[2].min(media[2]),
                    crop[3].min(media[3]),
                ];
                if clipped[0] > clipped[2] || clipped[1] > clipped[3] {
                    [0.0; 4]
                } else {
                    clipped
                }
            }
            None => media,
        };
        Self {
            left: shown[0],
            bottom: shown[1],
            right: shown[2],
            top: shown[3],
            rotation: ((rotate / 90) % 4 + 4) as u8 % 4,
        }
    }

    pub fn width(&self) -> f64 {
        self.right - self.left
    }

    pub fn height(&self) -> f64 {
        self.top - self.bottom
    }

    /// Whether places can be measured in it.
    pub fn is_usable(&self) -> bool {
        self.width() > 0.0 && self.height() > 0.0
    }

    /// A point of user space as fractions of the frame.
    pub fn to_uv(&self, x: f64, y: f64) -> (f64, f64) {
        (
            (x - self.left) / self.width(),
            (self.top - y) / self.height(),
        )
    }

    /// Fractions of the frame as a point of user space.
    pub fn to_user(&self, u: f64, v: f64) -> (f64, f64) {
        (self.left + u * self.width(), self.top - v * self.height())
    }

    /// Whether two frames are the same box, near enough for the places measured in one to be those
    /// of the other.
    pub fn same_as(&self, other: &Frame) -> bool {
        let close = |a: f64, b: f64| (a - b).abs() <= 0.01;
        close(self.left, other.left)
            && close(self.bottom, other.bottom)
            && close(self.right, other.right)
            && close(self.top, other.top)
            && self.rotation == other.rotation
    }
}

/// The kinds of annotation this reads and writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Subtype {
    Highlight,
    Underline,
    Squiggly,
    StrikeOut,
    Square,
    Circle,
    Ink,
    Text,
    FreeText,
}

impl Subtype {
    /// The PDF name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Highlight => "Highlight",
            Self::Underline => "Underline",
            Self::Squiggly => "Squiggly",
            Self::StrikeOut => "StrikeOut",
            Self::Square => "Square",
            Self::Circle => "Circle",
            Self::Ink => "Ink",
            Self::Text => "Text",
            Self::FreeText => "FreeText",
        }
    }

    pub fn from_name(name: &[u8]) -> Option<Self> {
        Some(match name {
            b"Highlight" => Self::Highlight,
            b"Underline" => Self::Underline,
            b"Squiggly" => Self::Squiggly,
            b"StrikeOut" => Self::StrikeOut,
            b"Square" => Self::Square,
            b"Circle" => Self::Circle,
            b"Ink" => Self::Ink,
            b"Text" => Self::Text,
            b"FreeText" => Self::FreeText,
            _ => return None,
        })
    }

    /// The XFDF element.
    pub fn xfdf(self) -> &'static str {
        match self {
            Self::Highlight => "highlight",
            Self::Underline => "underline",
            Self::Squiggly => "squiggly",
            Self::StrikeOut => "strikeout",
            Self::Square => "square",
            Self::Circle => "circle",
            Self::Ink => "ink",
            Self::Text => "text",
            Self::FreeText => "freetext",
        }
    }

    pub fn from_xfdf(element: &str) -> Option<Self> {
        Some(match element {
            "highlight" => Self::Highlight,
            "underline" => Self::Underline,
            "squiggly" => Self::Squiggly,
            "strikeout" => Self::StrikeOut,
            "square" => Self::Square,
            "circle" => Self::Circle,
            "ink" => Self::Ink,
            "text" => Self::Text,
            "freetext" => Self::FreeText,
            _ => return None,
        })
    }

    /// Whether it marks text by quads.
    pub fn is_markup(self) -> bool {
        matches!(
            self,
            Self::Highlight | Self::Underline | Self::Squiggly | Self::StrikeOut
        )
    }
}

/// The `/F` bit of an annotation that is not shown.
pub const FLAG_HIDDEN: u32 = 1 << 1;
/// The `/F` bit of an annotation that is printed.
pub const FLAG_PRINT: u32 = 1 << 2;
/// The `/F` bits that keep an icon at its size and upright, as sticky notes have.
pub const FLAG_NO_ZOOM: u32 = 1 << 3;
pub const FLAG_NO_ROTATE: u32 = 1 << 4;
/// The `/F` bit of an annotation shown nowhere.
pub const FLAG_NO_VIEW: u32 = 1 << 5;

/// The most quads, strokes and points one annotation may carry, and the longest texts.
pub const MAX_QUADS: usize = 4096;
pub const MAX_STROKES: usize = 256;
pub const MAX_STROKE_POINTS: usize = 8192;
pub const MAX_CONTENTS: usize = 1 << 20;
pub const MAX_NAME: usize = 256;

/// One annotation. Its places are fractions of its page's [`Frame`] (see the module), except
/// inside the readers and writers of this crate, which turn them to and from user space.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Annot {
    /// The page, from 0.
    pub page: u32,
    pub subtype: Option<Subtype>,
    /// Its box: left, top, right, bottom.
    pub rect: [f64; 4],
    /// The text it covers, for highlights and the like.
    #[serde(default)]
    pub quads: Vec<[f64; 8]>,
    /// The strokes of an ink annotation, each its points' `u` and `v` in turn.
    #[serde(default)]
    pub ink: Vec<Vec<f64>>,
    /// `/C`, from 0 to 1.
    pub color: Option<[f64; 3]>,
    /// `/IC`.
    pub interior: Option<[f64; 3]>,
    /// `/CA`.
    pub opacity: f64,
    /// The border's or the stroke's width, in points.
    pub width: Option<f64>,
    /// `/Contents`.
    #[serde(default)]
    pub contents: String,
    /// `/NM`.
    pub name: Option<String>,
    /// `/T`.
    pub author: Option<String>,
    /// `/Subj`.
    pub subject: Option<String>,
    /// `/CreationDate` and `/M`, in Unix milliseconds.
    pub created: Option<i64>,
    pub modified: Option<i64>,
    /// A sticky note's `/Name`.
    pub icon: Option<String>,
    /// `/F`.
    #[serde(default)]
    pub flags: u32,
    /// Whether it answers another (`/IRT`).
    #[serde(default)]
    pub reply: bool,
}

impl Annot {
    /// The same annotation with its places in user space of `frame`: the box as left, bottom,
    /// right, top.
    pub fn to_user(&self, frame: &Frame) -> Annot {
        let mut out = self.clone();
        let (l, t) = frame.to_user(self.rect[0], self.rect[1]);
        let (r, b) = frame.to_user(self.rect[2], self.rect[3]);
        out.rect = [l.min(r), b.min(t), l.max(r), b.max(t)];
        out.quads = self
            .quads
            .iter()
            .map(|quad| map_quad(quad, |u, v| frame.to_user(u, v)))
            .collect();
        out.ink = self
            .ink
            .iter()
            .map(|stroke| map_path(stroke, |u, v| frame.to_user(u, v)))
            .collect();
        out
    }

    /// The same annotation with its places, given in user space, as fractions of `frame`.
    pub fn to_uv(&self, frame: &Frame) -> Annot {
        let mut out = self.clone();
        let (l, t) = frame.to_uv(self.rect[0], self.rect[3]);
        let (r, b) = frame.to_uv(self.rect[2], self.rect[1]);
        out.rect = [l.min(r), t.min(b), l.max(r), t.max(b)];
        out.quads = self
            .quads
            .iter()
            .map(|quad| map_quad(quad, |x, y| frame.to_uv(x, y)))
            .collect();
        out.ink = self
            .ink
            .iter()
            .map(|stroke| map_path(stroke, |x, y| frame.to_uv(x, y)))
            .collect();
        out
    }
}

fn map_quad(quad: &[f64; 8], map: impl Fn(f64, f64) -> (f64, f64)) -> [f64; 8] {
    let mut out = [0.0; 8];
    for corner in 0..4 {
        let (x, y) = map(quad[corner * 2], quad[corner * 2 + 1]);
        out[corner * 2] = x;
        out[corner * 2 + 1] = y;
    }
    out
}

fn map_path(path: &[f64], map: impl Fn(f64, f64) -> (f64, f64)) -> Vec<f64> {
    path.as_chunks::<2>()
        .0
        .iter()
        .flat_map(|point| {
            let (x, y) = map(point[0], point[1]);
            [x, y]
        })
        .collect()
}

/// The box around a set of points, `[min x, min y, max x, max y]`.
pub fn bounds(points: impl IntoIterator<Item = (f64, f64)>) -> Option<[f64; 4]> {
    let mut bounds: Option<[f64; 4]> = None;
    for (x, y) in points {
        bounds = Some(match bounds {
            None => [x, y, x, y],
            Some([l, b, r, t]) => [l.min(x), b.min(y), r.max(x), t.max(y)],
        });
    }
    bounds
}

/// The corners of quads as points.
pub fn quad_points(quads: &[[f64; 8]]) -> impl Iterator<Item = (f64, f64)> + '_ {
    quads
        .iter()
        .flat_map(|quad| (0..4).map(move |corner| (quad[corner * 2], quad[corner * 2 + 1])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_frame_is_the_crop_box_clipped_to_the_media_box() {
        let frame = Frame::of(
            Some([0.0, 0.0, 595.0, 842.0]),
            Some([50.0, 60.0, 700.0, 782.0]),
            0,
        );
        assert_eq!(
            [frame.left, frame.bottom, frame.right, frame.top],
            [50.0, 60.0, 595.0, 782.0]
        );
        let upside = Frame::of(Some([595.0, 842.0, 0.0, 0.0]), None, -90);
        assert_eq!(
            [upside.left, upside.bottom, upside.right, upside.top],
            [0.0, 0.0, 595.0, 842.0]
        );
        assert_eq!(upside.rotation, 3);
        assert_eq!(Frame::of(None, None, 45).rotation, 0);
        assert_eq!(Frame::of(None, None, 450).rotation, 1);
        assert_eq!(Frame::of(None, None, 0).width(), 612.0);
        // A crop box outside the media box leaves nothing to show.
        assert!(
            !Frame::of(
                Some([0.0, 0.0, 100.0, 100.0]),
                Some([200.0, 200.0, 300.0, 300.0]),
                0
            )
            .is_usable()
        );
    }

    #[test]
    fn places_go_to_user_space_and_back() {
        let frame = Frame::of(
            Some([100.0, 100.0, 695.0, 942.0]),
            Some([150.0, 160.0, 645.0, 882.0]),
            90,
        );
        let annot = Annot {
            rect: [0.1, 0.2, 0.3, 0.25],
            quads: vec![[0.1, 0.2, 0.3, 0.2, 0.3, 0.25, 0.1, 0.25]],
            ink: vec![vec![0.5, 0.5, 0.6, 0.7]],
            ..Annot::default()
        };
        let user = annot.to_user(&frame);
        assert!((user.rect[0] - (150.0 + 0.1 * 495.0)).abs() < 1e-9);
        assert!((user.rect[3] - (882.0 - 0.2 * 722.0)).abs() < 1e-9);
        let back = user.to_uv(&frame);
        for (a, b) in annot.rect.iter().zip(back.rect.iter()) {
            assert!((a - b).abs() < 1e-12);
        }
        for (a, b) in annot.quads[0].iter().zip(back.quads[0].iter()) {
            assert!((a - b).abs() < 1e-12);
        }
        for (a, b) in annot.ink[0].iter().zip(back.ink[0].iter()) {
            assert!((a - b).abs() < 1e-12);
        }
    }
}

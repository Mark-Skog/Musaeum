//! Page orientation and the binary geometry table sent to the compositor.

use musaeum_shm::PageGeometry;
use serde::Serialize;

/// `"MGEO"`, the first word of the geometry table.
pub const GEOMETRY_MAGIC: u32 = u32::from_le_bytes(*b"MGEO");

/// Pages whose sides differ by less than this fraction count as square.
const SQUARE_TOLERANCE: f32 = 0.03;

/// Orientation of a single page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Orientation {
    /// Taller than wide.
    Portrait,
    /// Wider than tall.
    Landscape,
    /// Roughly equal sides.
    Square,
}

impl Orientation {
    /// Classifies a page from its size, which already includes the file's `/Rotate`.
    pub fn of(page: PageGeometry) -> Self {
        let longest = page.width_pt.max(page.height_pt);
        if longest <= 0.0 || (page.width_pt - page.height_pt).abs() < longest * SQUARE_TOLERANCE {
            Self::Square
        } else if page.width_pt > page.height_pt {
            Self::Landscape
        } else {
            Self::Portrait
        }
    }
}

/// Orientation of a whole document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DocumentOrientation {
    /// The document has no pages.
    Empty,
    /// Every page is portrait.
    Portrait,
    /// Every page is landscape.
    Landscape,
    /// Every page is square.
    Square,
    /// Pages disagree, as in a PDF with fold-out plates.
    Mixed,
}

/// Summarizes a document's orientation without looking at any pixel.
pub fn summarize_orientation(pages: &[PageGeometry]) -> DocumentOrientation {
    let mut orientations = pages.iter().map(|&page| Orientation::of(page));
    let Some(first) = orientations.next() else {
        return DocumentOrientation::Empty;
    };
    if orientations.all(|orientation| orientation == first) {
        match first {
            Orientation::Portrait => DocumentOrientation::Portrait,
            Orientation::Landscape => DocumentOrientation::Landscape,
            Orientation::Square => DocumentOrientation::Square,
        }
    } else {
        DocumentOrientation::Mixed
    }
}

/// Encodes page sizes as `[magic, count]` followed by `[width_pt, height_pt]` pairs of
/// little-endian `f32`, which JavaScript can view directly as a `Float32Array`.
pub fn geometry_bytes(pages: &[PageGeometry]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(8 + pages.len() * 8);
    bytes.extend_from_slice(&GEOMETRY_MAGIC.to_le_bytes());
    bytes.extend_from_slice(&(pages.len() as u32).to_le_bytes());
    for page in pages {
        bytes.extend_from_slice(&page.width_pt.to_le_bytes());
        bytes.extend_from_slice(&page.height_pt.to_le_bytes());
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(width_pt: f32, height_pt: f32) -> PageGeometry {
        PageGeometry {
            width_pt,
            height_pt,
        }
    }

    #[test]
    fn classifies_pages_with_a_square_tolerance() {
        assert_eq!(Orientation::of(page(612.0, 792.0)), Orientation::Portrait);
        assert_eq!(Orientation::of(page(842.0, 595.0)), Orientation::Landscape);
        assert_eq!(Orientation::of(page(500.0, 500.0)), Orientation::Square);
        assert_eq!(Orientation::of(page(500.0, 510.0)), Orientation::Square);
        assert_eq!(Orientation::of(page(0.0, 0.0)), Orientation::Square);
    }

    #[test]
    fn summarizes_documents() {
        let portrait = page(612.0, 792.0);
        let landscape = page(792.0, 612.0);
        assert_eq!(summarize_orientation(&[]), DocumentOrientation::Empty);
        assert_eq!(
            summarize_orientation(&[portrait, portrait]),
            DocumentOrientation::Portrait
        );
        assert_eq!(
            summarize_orientation(&[portrait, landscape]),
            DocumentOrientation::Mixed
        );
    }

    #[test]
    fn geometry_table_is_a_header_plus_float_pairs() {
        let bytes = geometry_bytes(&[page(612.0, 792.0)]);
        assert_eq!(bytes.len(), 16);
        assert_eq!(&bytes[0..4], b"MGEO");
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 1);
        assert_eq!(f32::from_le_bytes(bytes[8..12].try_into().unwrap()), 612.0);
        assert_eq!(f32::from_le_bytes(bytes[12..16].try_into().unwrap()), 792.0);
    }
}

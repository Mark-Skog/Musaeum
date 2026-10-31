//! Tile arithmetic shared by the host and the render workers.
//!
//! A level of detail `lod` means `2^lod` device pixels per PDF point. Tiles are
//! always [`TILE_SIZE`] square and are laid out on a grid anchored at the page's
//! top-left corner. `apps/desktop/src/compositor/layout.ts` mirrors these rules.

/// Edge of a tile in pixels.
pub const TILE_SIZE: u32 = 512;
/// Bytes per tile row (RGBA, tightly packed).
pub const TILE_STRIDE: usize = TILE_SIZE as usize * 4;
/// Bytes per tile.
pub const TILE_BYTES: usize = TILE_STRIDE * TILE_SIZE as usize;

/// Coarsest level of detail: 1/64 pixel per point.
pub const MIN_LOD: i8 = -6;
/// Finest level of detail: 32 pixels per point (2304 dpi).
pub const MAX_LOD: i8 = 5;

/// Identifies one tile of one page at one level of detail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TileKey {
    /// Zero-based page index.
    pub page: u32,
    /// Level of detail, in `MIN_LOD..=MAX_LOD`.
    pub lod: i8,
    /// Tile column.
    pub x: u16,
    /// Tile row.
    pub y: u16,
}

/// Page size in PDF points, after the file's `/Rotate` has been applied.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageGeometry {
    /// Width in points.
    pub width_pt: f32,
    /// Height in points.
    pub height_pt: f32,
}

/// Device pixels per point at `lod`.
pub fn lod_scale(lod: i8) -> f64 {
    2f64.powi(i32::from(lod))
}

/// Length in pixels of a page edge at `lod`. Never less than one pixel, so a
/// degenerate page still occupies one tile instead of dividing by zero downstream.
pub fn page_pixels(length_pt: f32, lod: i8) -> u64 {
    let pixels = (f64::from(length_pt) * lod_scale(lod)).round();
    if pixels.is_finite() && pixels >= 1.0 {
        pixels as u64
    } else {
        1
    }
}

/// Number of tile columns and rows that cover a page at `lod`.
pub fn tile_grid(page: PageGeometry, lod: i8) -> (u64, u64) {
    let size = u64::from(TILE_SIZE);
    (
        page_pixels(page.width_pt, lod).div_ceil(size),
        page_pixels(page.height_pt, lod).div_ceil(size),
    )
}

/// Whether `key` addresses an existing tile of `page`.
pub fn tile_in_page(key: TileKey, page: PageGeometry) -> bool {
    if !(MIN_LOD..=MAX_LOD).contains(&key.lod) {
        return false;
    }
    let (columns, rows) = tile_grid(page, key.lod);
    u64::from(key.x) < columns && u64::from(key.y) < rows
}

#[cfg(test)]
mod tests {
    use super::*;

    const LETTER: PageGeometry = PageGeometry {
        width_pt: 612.0,
        height_pt: 792.0,
    };

    #[test]
    fn grid_rounds_partial_tiles_up() {
        // 612 pt at 2 px/pt is 1224 px: two full tiles and a partial third.
        assert_eq!(tile_grid(LETTER, 1), (3, 4));
        // At 1/2 px/pt the whole page fits in a single tile.
        assert_eq!(tile_grid(LETTER, -1), (1, 1));
    }

    #[test]
    fn tile_bounds_are_enforced() {
        let key = |x, y, lod| TileKey { page: 0, lod, x, y };
        assert!(tile_in_page(key(2, 3, 1), LETTER));
        assert!(!tile_in_page(key(3, 0, 1), LETTER));
        assert!(!tile_in_page(key(0, 4, 1), LETTER));
        assert!(!tile_in_page(key(0, 0, MAX_LOD + 1), LETTER));
        assert!(!tile_in_page(key(0, 0, MIN_LOD - 1), LETTER));
    }

    #[test]
    fn degenerate_pages_still_have_one_tile() {
        let empty = PageGeometry {
            width_pt: 0.0,
            height_pt: f32::NAN,
        };
        assert_eq!(tile_grid(empty, 0), (1, 1));
    }
}

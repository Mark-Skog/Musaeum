//! Tile arithmetic shared by the host and the render workers.
//!
//! A tile's `scale` is in device pixels per PDF point, counted in units of 1/[`SCALE_ONE`]. Any
//! scale the viewport needs can be rendered, so text is rasterized at exactly the size it is shown
//! instead of being resampled from the nearest power of two. Rounding to a step of 1/4096 pixel per
//! point moves the edge of an ordinary page by about a tenth of a pixel, and of the largest page PDF
//! allows by under two. The compositor draws a page at the quantized scale itself, so every texel
//! still lands on one pixel. Tiles are always [`TILE_SIZE`] square, on a grid anchored at the page's
//! top-left corner. `apps/desktop/src/compositor/tiles.ts` mirrors these rules.

/// Edge of a tile in pixels.
pub const TILE_SIZE: u32 = 512;
/// Bytes per tile row (RGBA, tightly packed).
pub const TILE_STRIDE: usize = TILE_SIZE as usize * 4;
/// Bytes per tile.
pub const TILE_BYTES: usize = TILE_STRIDE * TILE_SIZE as usize;

/// One device pixel per point, in the units of a tile's scale.
pub const SCALE_ONE: u32 = 4096;
/// Coarsest scale: 1/64 pixel per point.
pub const MIN_SCALE: u32 = SCALE_ONE / 64;
/// Finest scale: 32 pixels per point (2304 dpi).
pub const MAX_SCALE: u32 = SCALE_ONE * 32;
/// Longest page edge a tile grid may have, in pixels. It bounds a grid to 1024 tiles a side, which
/// keeps the compositor's tile keys exact.
pub const MAX_PAGE_PIXELS: u64 = 1 << 19;

/// Identifies one tile of one page at one scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TileKey {
    /// Zero-based page index.
    pub page: u32,
    /// Device pixels per point, in units of 1/[`SCALE_ONE`], in `MIN_SCALE..=MAX_SCALE`.
    pub scale: u32,
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

/// Device pixels per point at `scale`.
pub fn scale_factor(scale: u32) -> f64 {
    f64::from(scale) / f64::from(SCALE_ONE)
}

/// The scale of a power-of-two level of detail, `2^lod` pixels per point.
pub fn scale_of_lod(lod: i8) -> u32 {
    let exponent = i32::from(lod).clamp(-6, 5);
    if exponent >= 0 {
        SCALE_ONE << exponent
    } else {
        SCALE_ONE >> -exponent
    }
}

/// Length in pixels of a page edge at `scale`. Never less than one pixel, so a degenerate page
/// still occupies one tile instead of dividing by zero downstream.
pub fn page_pixels(length_pt: f32, scale: u32) -> u64 {
    let pixels = (f64::from(length_pt) * f64::from(scale) / f64::from(SCALE_ONE)).round();
    if pixels.is_finite() && pixels >= 1.0 {
        pixels as u64
    } else {
        1
    }
}

/// Number of tile columns and rows that cover a page at `scale`.
pub fn tile_grid(page: PageGeometry, scale: u32) -> (u64, u64) {
    let size = u64::from(TILE_SIZE);
    (
        page_pixels(page.width_pt, scale).div_ceil(size),
        page_pixels(page.height_pt, scale).div_ceil(size),
    )
}

/// Longest edge of a page thumbnail, in pixels.
pub const THUMBNAIL_EDGE: u32 = 256;

/// The finest scale at which the whole page fits in a square of `edge` pixels, so that its
/// thumbnail is the single tile at `x = 0, y = 0`. `edge` is at most [`TILE_SIZE`]. Only a page
/// longer than `edge` × 64 points, which PDF without `/UserUnit` does not allow, still overflows it.
pub fn thumbnail_scale(page: PageGeometry, edge: u32) -> u32 {
    let edge = edge.clamp(1, TILE_SIZE);
    let longest = page.width_pt.max(page.height_pt);
    if !longest.is_finite() || longest <= 0.0 {
        return MIN_SCALE;
    }
    // An edge rounds to `edge` pixels up to just under `edge + 0.5`, so the largest scale is the
    // last one below that.
    let fits = ((f64::from(edge) + 0.5) * f64::from(SCALE_ONE) / f64::from(longest)).ceil() - 1.0;
    let mut scale = fits.clamp(f64::from(MIN_SCALE), f64::from(MAX_SCALE)) as u32;
    // Floating-point error can still land one pixel over.
    while scale > MIN_SCALE && page_pixels(longest, scale) > u64::from(edge) {
        scale -= 1;
    }
    scale
}

/// Whether `key` addresses an existing tile of `page`.
pub fn tile_in_page(key: TileKey, page: PageGeometry) -> bool {
    if !(MIN_SCALE..=MAX_SCALE).contains(&key.scale)
        || page_pixels(page.width_pt, key.scale) > MAX_PAGE_PIXELS
        || page_pixels(page.height_pt, key.scale) > MAX_PAGE_PIXELS
    {
        return false;
    }
    let (columns, rows) = tile_grid(page, key.scale);
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
        assert_eq!(tile_grid(LETTER, scale_of_lod(1)), (3, 4));
        // At 1/2 px/pt the whole page fits in a single tile.
        assert_eq!(tile_grid(LETTER, scale_of_lod(-1)), (1, 1));
    }

    #[test]
    fn any_scale_is_a_grid_of_its_own() {
        // 1.37 px/pt, as a zoom of 137% on a plain display asks for: 839 by 1085 pixels.
        let scale = (1.37 * f64::from(SCALE_ONE)).round() as u32;
        assert_eq!(page_pixels(612.0, scale), 839);
        assert_eq!(page_pixels(792.0, scale), 1085);
        assert_eq!(tile_grid(LETTER, scale), (2, 3));
        assert!((scale_factor(scale) - 1.37).abs() < 1.0 / f64::from(SCALE_ONE));
    }

    #[test]
    fn levels_of_detail_are_scales_too() {
        assert_eq!(scale_of_lod(0), SCALE_ONE);
        assert_eq!(scale_of_lod(-6), MIN_SCALE);
        assert_eq!(scale_of_lod(5), MAX_SCALE);
        assert_eq!(scale_of_lod(-2), SCALE_ONE / 4);
    }

    #[test]
    fn tile_bounds_are_enforced() {
        let key = |x, y, scale| TileKey {
            page: 0,
            scale,
            x,
            y,
        };
        let two = scale_of_lod(1);
        assert!(tile_in_page(key(2, 3, two), LETTER));
        assert!(!tile_in_page(key(3, 0, two), LETTER));
        assert!(!tile_in_page(key(0, 4, two), LETTER));
        assert!(!tile_in_page(key(0, 0, MAX_SCALE + 1), LETTER));
        assert!(!tile_in_page(key(0, 0, MIN_SCALE - 1), LETTER));
        assert!(!tile_in_page(key(0, 0, 0), LETTER));
    }

    #[test]
    fn a_page_too_large_for_a_grid_at_some_scale_is_refused_at_that_scale() {
        // A banner larger than PDF's own 200 inches, as /UserUnit allows: fine at 1 px/pt, beyond
        // the grid's limit at 32.
        let banner = PageGeometry {
            width_pt: 20_000.0,
            height_pt: 100.0,
        };
        let key = |scale| TileKey {
            page: 0,
            scale,
            x: 0,
            y: 0,
        };
        assert!(tile_in_page(key(SCALE_ONE), banner));
        assert!(!tile_in_page(key(MAX_SCALE), banner));
    }

    #[test]
    fn a_thumbnail_is_one_tile_as_large_as_the_box_allows() {
        let scale = thumbnail_scale(LETTER, THUMBNAIL_EDGE);
        assert_eq!(page_pixels(792.0, scale), 256);
        assert!(page_pixels(612.0, scale) <= 256);
        assert_eq!(tile_grid(LETTER, scale), (1, 1));
        assert!(
            page_pixels(792.0, scale + 1) > 256,
            "the next step up would not fit"
        );

        let landscape = PageGeometry {
            width_pt: 1190.0,
            height_pt: 842.0,
        };
        assert_eq!(
            page_pixels(1190.0, thumbnail_scale(landscape, THUMBNAIL_EDGE)),
            256
        );
        let empty = PageGeometry {
            width_pt: 0.0,
            height_pt: 0.0,
        };
        assert_eq!(thumbnail_scale(empty, THUMBNAIL_EDGE), MIN_SCALE);
    }

    #[test]
    fn degenerate_pages_still_have_one_tile() {
        let empty = PageGeometry {
            width_pt: 0.0,
            height_pt: f32::NAN,
        };
        assert_eq!(tile_grid(empty, SCALE_ONE), (1, 1));
    }
}

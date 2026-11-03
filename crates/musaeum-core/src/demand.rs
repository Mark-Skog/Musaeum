//! Binary demand lists sent by the compositor: the tiles it wants, best first.
//!
//! ```text
//! u32 magic "MDMD"   u32 epoch   u32 count   u32 reserved
//! count × { u32 page, u32 scale, u32 x, u32 y }
//! ```
//!
//! The list comes from a WebView, which later phases will share with untrusted book
//! content, so every field is validated against the document before it can reach a
//! worker.

use musaeum_shm::tile::tile_in_page;
use musaeum_shm::{PageGeometry, TileKey};
use thiserror::Error;

/// `"MDMD"`, the first word of a demand list.
pub const DEMAND_MAGIC: u32 = u32::from_le_bytes(*b"MDMD");
/// Most tiles a single demand may ask for.
pub const MAX_DEMAND_TILES: usize = 4096;

const HEADER_LEN: usize = 16;
const ENTRY_LEN: usize = 16;

/// A validated demand list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Demand {
    /// Increases with every list the compositor sends.
    pub epoch: u32,
    /// Wanted tiles, most important first.
    pub tiles: Vec<TileKey>,
}

/// Why a demand list was rejected.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum DemandError {
    /// Shorter than its header, or its length disagrees with its count.
    #[error("demand has an inconsistent length of {0} bytes")]
    BadLength(usize),
    /// The first word is wrong.
    #[error("demand has a bad magic number")]
    BadMagic,
    /// More tiles than [`MAX_DEMAND_TILES`].
    #[error("demand asks for {0} tiles, more than the limit of {MAX_DEMAND_TILES}")]
    TooManyTiles(usize),
    /// A tile that does not exist in this document.
    #[error("tile {index} of the demand does not exist in the document")]
    BadTile {
        /// Position in the list.
        index: usize,
    },
}

fn word(bytes: &[u8], offset: usize) -> u32 {
    let mut raw = [0; 4];
    raw.copy_from_slice(&bytes[offset..offset + 4]);
    u32::from_le_bytes(raw)
}

/// Parses and validates a demand list against the document's pages.
pub fn parse_demand(bytes: &[u8], pages: &[PageGeometry]) -> Result<Demand, DemandError> {
    if bytes.len() < HEADER_LEN {
        return Err(DemandError::BadLength(bytes.len()));
    }
    if word(bytes, 0) != DEMAND_MAGIC {
        return Err(DemandError::BadMagic);
    }
    let epoch = word(bytes, 4);
    let count = word(bytes, 8) as usize;
    if count > MAX_DEMAND_TILES {
        return Err(DemandError::TooManyTiles(count));
    }
    if bytes.len() != HEADER_LEN + count * ENTRY_LEN {
        return Err(DemandError::BadLength(bytes.len()));
    }

    let mut tiles = Vec::with_capacity(count);
    for index in 0..count {
        let at = HEADER_LEN + index * ENTRY_LEN;
        let tile = decode_tile(
            word(bytes, at),
            word(bytes, at + 4),
            word(bytes, at + 8),
            word(bytes, at + 12),
        )
        .filter(|tile| {
            pages
                .get(tile.page as usize)
                .is_some_and(|&page| tile_in_page(*tile, page))
        })
        .ok_or(DemandError::BadTile { index })?;
        tiles.push(tile);
    }
    Ok(Demand { epoch, tiles })
}

/// Encodes a demand list. The compositor does this in TypeScript; this version exists for
/// tests and headless tools.
pub fn encode_demand(epoch: u32, tiles: &[TileKey]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(HEADER_LEN + tiles.len() * ENTRY_LEN);
    for word in [DEMAND_MAGIC, epoch, tiles.len() as u32, 0] {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    for tile in tiles {
        let words = [tile.page, tile.scale, u32::from(tile.x), u32::from(tile.y)];
        for word in words {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
    }
    bytes
}

fn decode_tile(page: u32, scale: u32, x: u32, y: u32) -> Option<TileKey> {
    Some(TileKey {
        page,
        scale,
        x: u16::try_from(x).ok()?,
        y: u16::try_from(y).ok()?,
    })
}

#[cfg(test)]
mod tests {
    use musaeum_shm::tile::{MAX_SCALE, SCALE_ONE, scale_of_lod};

    use super::*;

    const PAGES: &[PageGeometry] = &[PageGeometry {
        width_pt: 612.0,
        height_pt: 792.0,
    }];

    fn encode(epoch: u32, tiles: &[(u32, u32, u32, u32)]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for value in [DEMAND_MAGIC, epoch, tiles.len() as u32, 0] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for &(page, scale, x, y) in tiles {
            for value in [page, scale, x, y] {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        bytes
    }

    #[test]
    fn parses_a_valid_list_in_order() {
        let two = scale_of_lod(1);
        let tiles = [(0, two, 2, 3), (0, SCALE_ONE * 3 / 2, 0, 0)];
        let demand = parse_demand(&encode(9, &tiles), PAGES).unwrap();
        assert_eq!(demand.epoch, 9);
        assert_eq!(demand.tiles.len(), 2);
        assert_eq!(
            demand.tiles[0],
            TileKey {
                page: 0,
                scale: two,
                x: 2,
                y: 3
            }
        );
        assert_eq!(
            demand.tiles[1].scale, 6144,
            "any scale, not just powers of two"
        );
    }

    /// The same words are asserted by `apps/desktop/src/compositor/demand.test.ts`.
    #[test]
    fn parses_the_exact_bytes_the_typescript_encoder_produces() {
        let words: [u32; 12] = [0x444D_444D, 7, 2, 0, 0, 5611, 1, 0, 0, 8192, 2, 3];
        let bytes: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();

        let demand = parse_demand(&bytes, PAGES).unwrap();
        assert_eq!(demand.epoch, 7);
        assert_eq!(
            demand.tiles,
            [
                TileKey {
                    page: 0,
                    scale: 5611,
                    x: 1,
                    y: 0
                },
                TileKey {
                    page: 0,
                    scale: 8192,
                    x: 2,
                    y: 3
                },
            ]
        );
        assert_eq!(encode_demand(7, &demand.tiles), bytes);
    }

    #[test]
    fn an_empty_list_is_valid_and_means_nothing_is_wanted() {
        assert!(
            parse_demand(&encode(1, &[]), PAGES)
                .unwrap()
                .tiles
                .is_empty()
        );
    }

    #[test]
    fn rejects_structural_damage() {
        assert_eq!(parse_demand(&[0; 3], PAGES), Err(DemandError::BadLength(3)));

        let mut wrong_magic = encode(1, &[]);
        wrong_magic[0] ^= 0xFF;
        assert_eq!(
            parse_demand(&wrong_magic, PAGES),
            Err(DemandError::BadMagic)
        );

        let mut truncated = encode(1, &[(0, SCALE_ONE, 0, 0)]);
        truncated.pop();
        assert_eq!(
            parse_demand(&truncated, PAGES),
            Err(DemandError::BadLength(31))
        );

        let mut oversized = encode(1, &[]);
        oversized[8..12].copy_from_slice(&(MAX_DEMAND_TILES as u32 + 1).to_le_bytes());
        assert_eq!(
            parse_demand(&oversized, PAGES),
            Err(DemandError::TooManyTiles(MAX_DEMAND_TILES + 1))
        );
    }

    #[test]
    fn rejects_tiles_outside_the_document() {
        let two = scale_of_lod(1);
        for bad in [
            (1, SCALE_ONE, 0, 0),        // no such page
            (0, two, 3, 0),              // past the last column
            (0, two, 0, 4),              // past the last row
            (0, 0, 0, 0),                // no scale at all
            (0, MAX_SCALE + 1, 0, 0),    // finer than any scale allowed
            (0, two, 70_000, 0),         // does not fit the wire format
            (0, u32::MAX, 0, 0),         // absurd scale
            (u32::MAX, SCALE_ONE, 0, 0), // absurd page
        ] {
            assert_eq!(
                parse_demand(&encode(1, &[bad]), PAGES),
                Err(DemandError::BadTile { index: 0 }),
                "{bad:?} must be rejected"
            );
        }
    }
}

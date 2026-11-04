//! Pixel slab: fixed-size tile slots plus per-worker cancellation cells.
//!
//! ```text
//! 0x0000  header (64 bytes): magic, version, slot count, tile size, pixels offset
//! 0x0040  cancel cells: one AtomicU64 per worker, each on its own cache line
//! 0x1000  pixels: slot_count × TILE_BYTES, page-aligned
//! ```
//!
//! Slots carry no state in shared memory. Ownership is tracked privately by the
//! host's scheduler and transferred with control-plane messages, so nothing a worker
//! scribbles over the slab can change which slot the host believes it may read.
//!
//! The invariants are:
//!
//! 1. A slot is written by at most one worker: from the moment it is dispatched
//!    until that worker reports a terminal message for the job, or dies.
//! 2. A slot is read only after that terminal message, and is not reused until the
//!    reader has released it.

use std::mem::MaybeUninit;
use std::ptr;
use std::sync::atomic::AtomicU64;

use thiserror::Error;

use crate::region::ShmRegion;
use crate::tile::{TILE_BYTES, TILE_SIZE, TILE_STRIDE};

/// `"MSLB"`, the first word of every slab.
pub const SLAB_MAGIC: u32 = u32::from_le_bytes(*b"MSLB");
/// Version of this layout.
pub const SLAB_VERSION: u32 = 1;
/// Offset of the first pixel of slot zero.
pub const PIXELS_OFFSET: usize = 4096;
/// Upper bound on concurrent render workers, fixed by the header area's size.
pub const MAX_WORKERS: usize = 16;

const CANCEL_OFFSET: usize = 64;
const CANCEL_STRIDE: usize = 64;

// The cancel cells must fit between the header and the pixels.
const _: () = assert!(CANCEL_OFFSET + MAX_WORKERS * CANCEL_STRIDE <= PIXELS_OFFSET);

/// Errors from creating or addressing a slab.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SlabError {
    /// The region cannot hold the requested slots.
    #[error("region of {actual} bytes is too small for a slab of {required} bytes")]
    RegionTooSmall {
        /// Bytes needed.
        required: usize,
        /// Bytes available.
        actual: usize,
    },
    /// The region does not start with a compatible slab header.
    #[error("region does not contain a compatible pixel slab")]
    BadHeader,
    /// A slab must have at least one slot.
    #[error("a slab needs at least one slot")]
    NoSlots,
    /// A slot number outside the slab.
    #[error("slot {slot} is out of range (slab has {slot_count})")]
    SlotOutOfRange {
        /// Requested slot.
        slot: u32,
        /// Slots in this slab.
        slot_count: u32,
    },
    /// A worker index outside the cancel-cell table.
    #[error("worker index {0} exceeds the maximum of {MAX_WORKERS} workers")]
    WorkerOutOfRange(usize),
}

/// Bytes needed for a slab of `slot_count` slots.
pub fn slab_len(slot_count: u32) -> usize {
    PIXELS_OFFSET + slot_count as usize * TILE_BYTES
}

/// A pixel slab mapped into this process.
#[derive(Debug)]
pub struct PixelSlab {
    region: ShmRegion,
    slot_count: u32,
}

impl PixelSlab {
    /// Formats `region` as a new slab. Must finish before anyone else can see it.
    pub fn init(region: ShmRegion, slot_count: u32) -> Result<Self, SlabError> {
        if slot_count == 0 {
            return Err(SlabError::NoSlots);
        }
        let required = slab_len(slot_count);
        if region.len() < required {
            return Err(SlabError::RegionTooSmall {
                required,
                actual: region.len(),
            });
        }
        let header = [
            SLAB_MAGIC,
            SLAB_VERSION,
            slot_count,
            TILE_SIZE,
            PIXELS_OFFSET as u32,
        ];
        // SAFETY: the region is at least `PIXELS_OFFSET` bytes, mapped views are
        // page-aligned, and the caller has not shared the region yet.
        unsafe {
            ptr::write_bytes(region.as_ptr(), 0, PIXELS_OFFSET);
            region
                .as_ptr()
                .cast::<[u32; 5]>()
                .write(header.map(u32::to_le));
        }
        Ok(Self { region, slot_count })
    }

    /// Opens a slab that the host has already formatted.
    pub fn attach(region: ShmRegion) -> Result<Self, SlabError> {
        if region.len() < PIXELS_OFFSET {
            return Err(SlabError::RegionTooSmall {
                required: PIXELS_OFFSET,
                actual: region.len(),
            });
        }
        // SAFETY: the region holds at least the header and the view is page-aligned.
        let [magic, version, slot_count, tile_size, pixels_offset] =
            unsafe { region.as_ptr().cast::<[u32; 5]>().read_volatile() }.map(u32::from_le);
        if magic != SLAB_MAGIC
            || version != SLAB_VERSION
            || tile_size != TILE_SIZE
            || pixels_offset != PIXELS_OFFSET as u32
            || slot_count == 0
        {
            return Err(SlabError::BadHeader);
        }
        let required = slab_len(slot_count);
        if region.len() < required {
            return Err(SlabError::RegionTooSmall {
                required,
                actual: region.len(),
            });
        }
        Ok(Self { region, slot_count })
    }

    /// Number of tile slots.
    pub fn slot_count(&self) -> u32 {
        self.slot_count
    }

    /// The shared-memory region behind this slab.
    pub fn region(&self) -> &ShmRegion {
        &self.region
    }

    /// The cell a host uses to cancel `worker`'s job: storing a job sequence number
    /// cancels that job and every earlier one. Sequence numbers are never reused, so
    /// a stale value cannot cancel a later job.
    pub fn cancel_cell(&self, worker: usize) -> Result<&AtomicU64, SlabError> {
        if worker >= MAX_WORKERS {
            return Err(SlabError::WorkerOutOfRange(worker));
        }
        // SAFETY: the offset is inside the header area (checked at compile time), is
        // 8-byte aligned because the view is page-aligned and both constants are
        // multiples of 8, and the cell is only ever accessed atomically by every
        // party. `AtomicU64` has no invalid bit patterns.
        Ok(unsafe {
            &*self
                .region
                .as_ptr()
                .add(CANCEL_OFFSET + worker * CANCEL_STRIDE)
                .cast::<AtomicU64>()
        })
    }

    /// Start of a slot's pixels, for handing to PDFium as an external buffer.
    pub fn slot_ptr(&self, slot: u32) -> Result<*mut u8, SlabError> {
        if slot >= self.slot_count {
            return Err(SlabError::SlotOutOfRange {
                slot,
                slot_count: self.slot_count,
            });
        }
        // SAFETY: `slot < slot_count` and the region was checked to hold every slot.
        Ok(unsafe {
            self.region
                .as_ptr()
                .add(PIXELS_OFFSET + slot as usize * TILE_BYTES)
        })
    }

    /// Copies a slot out of shared memory.
    ///
    /// The bytes are not exposed as `&[u8]` on purpose: another process can map this
    /// memory, so handing out a reference would promise an immutability this process
    /// cannot enforce. The worst a misbehaving worker can cause here is wrong pixels.
    pub fn copy_slot(&self, slot: u32, dst: &mut [MaybeUninit<u8>]) -> Result<(), SlabError> {
        assert_eq!(
            dst.len(),
            TILE_BYTES,
            "destination must hold exactly one tile"
        );
        let src = self.slot_ptr(slot)?;
        // SAFETY: `src` addresses `TILE_BYTES` readable bytes, `dst` is that long, and
        // shared memory cannot overlap a process-local buffer.
        unsafe { ptr::copy_nonoverlapping(src, dst.as_mut_ptr().cast::<u8>(), TILE_BYTES) };
        Ok(())
    }

    /// Copies the first `len` bytes of a slot, such as a text layer, clamped to the slot.
    pub fn copy_slot_prefix(&self, slot: u32, len: usize) -> Result<Vec<u8>, SlabError> {
        let src = self.slot_ptr(slot)?;
        let len = len.min(TILE_BYTES);
        let mut out: Vec<u8> = Vec::with_capacity(len);
        // SAFETY: `src` addresses `TILE_BYTES` readable bytes and `len` is no more than that;
        // `out` has room for `len` bytes and cannot overlap shared memory.
        unsafe {
            ptr::copy_nonoverlapping(src, out.as_mut_ptr(), len);
            out.set_len(len);
        }
        Ok(out)
    }

    /// Copies the `width` × `height` pixels at a slot's top-left corner, rows packed tightly, as
    /// for a thumbnail drawn into one corner of a tile. Both sides are clamped to the tile.
    pub fn copy_slot_rect(&self, slot: u32, width: u32, height: u32) -> Result<Vec<u8>, SlabError> {
        let src = self.slot_ptr(slot)?;
        let row = width.min(TILE_SIZE) as usize * 4;
        let rows = height.min(TILE_SIZE) as usize;
        let mut out: Vec<u8> = Vec::with_capacity(row * rows);
        for y in 0..rows {
            // SAFETY: row `y` of the slot starts `y * TILE_STRIDE` bytes in and holds at least
            // `row` readable bytes, since both are clamped to the tile; `out` has room for them.
            unsafe {
                ptr::copy_nonoverlapping(
                    src.add(y * TILE_STRIDE),
                    out.as_mut_ptr().add(y * row),
                    row,
                );
            }
        }
        // SAFETY: the loop initialized `rows` rows of `row` bytes.
        unsafe { out.set_len(row * rows) };
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use super::*;

    fn slab(slots: u32) -> PixelSlab {
        PixelSlab::init(ShmRegion::create(slab_len(slots)).unwrap(), slots).unwrap()
    }

    #[test]
    fn init_rejects_undersized_regions_and_zero_slots() {
        let region = ShmRegion::create(PIXELS_OFFSET).unwrap();
        assert!(matches!(
            PixelSlab::init(region, 1),
            Err(SlabError::RegionTooSmall { .. })
        ));
        let region = ShmRegion::create(PIXELS_OFFSET).unwrap();
        assert!(matches!(
            PixelSlab::init(region, 0),
            Err(SlabError::NoSlots)
        ));
    }

    #[test]
    fn attach_rejects_memory_that_is_not_a_slab() {
        let region = ShmRegion::create(slab_len(1)).unwrap();
        assert!(matches!(
            PixelSlab::attach(region),
            Err(SlabError::BadHeader)
        ));
    }

    #[test]
    fn slots_are_bounds_checked_and_page_aligned() {
        let slab = slab(3);
        assert!(slab.slot_ptr(2).is_ok());
        assert_eq!(
            slab.slot_ptr(3),
            Err(SlabError::SlotOutOfRange {
                slot: 3,
                slot_count: 3
            })
        );
        assert_eq!(slab.slot_ptr(1).unwrap() as usize % 4096, 0);
    }

    #[test]
    fn cancel_cells_do_not_share_a_cache_line_or_touch_pixels() {
        let slab = slab(1);
        for worker in 0..MAX_WORKERS {
            slab.cancel_cell(worker)
                .unwrap()
                .store(worker as u64 + 1, Ordering::Relaxed);
        }
        for worker in 0..MAX_WORKERS {
            assert_eq!(
                slab.cancel_cell(worker).unwrap().load(Ordering::Relaxed),
                worker as u64 + 1
            );
        }
        assert!(slab.cancel_cell(MAX_WORKERS).is_err());
        // SAFETY: slot zero exists and nothing else is using it in this test.
        assert_eq!(unsafe { slab.slot_ptr(0).unwrap().read() }, 0);
    }

    #[test]
    fn copy_slot_returns_exactly_the_slots_bytes() {
        let slab = slab(2);
        // SAFETY: both slots exist and this test is single-threaded.
        unsafe {
            ptr::write_bytes(slab.slot_ptr(0).unwrap(), 0x11, TILE_BYTES);
            ptr::write_bytes(slab.slot_ptr(1).unwrap(), 0x22, TILE_BYTES);
        }
        let mut out = Vec::with_capacity(TILE_BYTES);
        slab.copy_slot(1, &mut out.spare_capacity_mut()[..TILE_BYTES])
            .unwrap();
        // SAFETY: `copy_slot` initialized exactly `TILE_BYTES` bytes.
        unsafe { out.set_len(TILE_BYTES) };
        assert!(out.iter().all(|&byte| byte == 0x22));
    }

    #[test]
    fn partial_copies_take_exactly_the_bytes_asked_for() {
        let slab = slab(1);
        let base = slab.slot_ptr(0).unwrap();
        // SAFETY: slot zero exists and this test is single-threaded. Each row holds its index.
        unsafe {
            for y in 0..TILE_SIZE as usize {
                ptr::write_bytes(base.add(y * TILE_STRIDE), y as u8, TILE_STRIDE);
            }
        }
        assert_eq!(slab.copy_slot_prefix(0, 10).unwrap(), vec![0u8; 10]);
        assert_eq!(
            slab.copy_slot_prefix(0, usize::MAX).unwrap().len(),
            TILE_BYTES
        );

        let rect = slab.copy_slot_rect(0, 3, 2).unwrap();
        assert_eq!(rect, [vec![0u8; 12], vec![1u8; 12]].concat());
        assert_eq!(
            slab.copy_slot_rect(0, 10_000, 1).unwrap().len(),
            TILE_STRIDE
        );
        assert!(slab.copy_slot_rect(1, 1, 1).is_err());
    }
}

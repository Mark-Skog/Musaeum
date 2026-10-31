//! Single-producer, single-consumer ring that announces finished tiles.
//!
//! On Path B the producer is the host's pool thread and the consumer is the WebView's
//! main thread, reading through a WebView2 shared buffer with `Atomics`. The layout is
//! mirrored in `apps/desktop/src/protocol.ts`.
//!
//! ```text
//! 0x00  header: magic, version, capacity, entry length (u32 each)
//! 0x40  head: AtomicU32, written only by the producer
//! 0x80  consumed: AtomicU32, written only by the consumer
//! 0xC0  entries: capacity × 8 u32 words
//! ```
//!
//! `head` and `consumed` are free-running counters that wrap at `u32::MAX`, and a
//! power-of-two capacity keeps masking correct across the wrap. Each counter sits on
//! its own cache line so the two sides do not slow each other down.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use thiserror::Error;

use crate::region::ShmRegion;

/// `"MDRG"`, the first word of every delivery ring.
pub const RING_MAGIC: u32 = u32::from_le_bytes(*b"MDRG");
/// Version of this layout.
pub const RING_VERSION: u32 = 1;
/// Words per entry.
pub const ENTRY_WORDS: usize = 8;

const HEAD_OFFSET: usize = 0x40;
const CONSUMED_OFFSET: usize = 0x80;
const ENTRIES_OFFSET: usize = 0xC0;
const ENTRY_LEN: usize = ENTRY_WORDS * 4;

/// One finished tile. Host-validated, so the consumer never has to read anything a
/// render worker could have written, apart from the pixels themselves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileAnnouncement {
    /// Slab slot holding the pixels.
    pub slot: u32,
    /// Page index.
    pub page: u32,
    /// Level of detail.
    pub lod: i32,
    /// Tile column.
    pub x: u32,
    /// Tile row.
    pub y: u32,
    /// Demand epoch the tile was rendered for.
    pub epoch: u32,
    /// Microseconds the worker spent on it, including page load.
    pub render_us: u32,
    /// Host clock when the tile finished, in microseconds, wrapping.
    pub done_us: u32,
}

impl TileAnnouncement {
    /// The tile as the eight words stored in the ring and in Path A batches.
    pub fn to_words(self) -> [u32; ENTRY_WORDS] {
        [
            self.slot,
            self.page,
            self.lod as u32,
            self.x,
            self.y,
            self.epoch,
            self.render_us,
            self.done_us,
        ]
    }

    /// Rebuilds a tile from its words.
    pub fn from_words(words: [u32; ENTRY_WORDS]) -> Self {
        Self {
            slot: words[0],
            page: words[1],
            lod: words[2] as i32,
            x: words[3],
            y: words[4],
            epoch: words[5],
            render_us: words[6],
            done_us: words[7],
        }
    }
}

/// Errors from formatting a ring.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum RingError {
    /// Capacity was zero or not a power of two.
    #[error("ring capacity must be a non-zero power of two, got {0}")]
    BadCapacity(u32),
    /// The region cannot hold the requested entries.
    #[error("region of {actual} bytes is too small for a ring of {required} bytes")]
    RegionTooSmall {
        /// Bytes needed.
        required: usize,
        /// Bytes available.
        actual: usize,
    },
}

/// The ring is full; the consumer has to catch up first.
#[derive(Debug, Error, PartialEq, Eq)]
#[error("delivery ring is full")]
pub struct RingFull;

/// Bytes needed for a ring of `capacity` entries.
pub fn ring_len(capacity: u32) -> usize {
    ENTRIES_OFFSET + capacity as usize * ENTRY_LEN
}

/// Producer side of a delivery ring.
#[derive(Debug)]
pub struct DeliveryRing {
    region: Arc<ShmRegion>,
    capacity: u32,
    head: u32,
}

impl DeliveryRing {
    /// Formats `region` as an empty ring. Must finish before the consumer can see it.
    pub fn init(region: ShmRegion, capacity: u32) -> Result<Self, RingError> {
        if capacity == 0 || !capacity.is_power_of_two() {
            return Err(RingError::BadCapacity(capacity));
        }
        let required = ring_len(capacity);
        if region.len() < required {
            return Err(RingError::RegionTooSmall {
                required,
                actual: region.len(),
            });
        }
        let header = [RING_MAGIC, RING_VERSION, capacity, ENTRY_LEN as u32];
        // SAFETY: the region is large enough, mapped views are page-aligned, and the
        // caller has not shared the region yet.
        unsafe {
            std::ptr::write_bytes(region.as_ptr(), 0, required);
            region.as_ptr().cast::<[u32; 4]>().write(header);
        }
        Ok(Self {
            region: Arc::new(region),
            capacity,
            head: 0,
        })
    }

    /// Number of entries the ring can hold.
    pub fn capacity(&self) -> u32 {
        self.capacity
    }

    /// Publishes one tile.
    pub fn publish(&mut self, tile: TileAnnouncement) -> Result<(), RingFull> {
        let consumed = counter(&self.region, CONSUMED_OFFSET).load(Ordering::Acquire);
        if self.head.wrapping_sub(consumed) >= self.capacity {
            return Err(RingFull);
        }
        // SAFETY: the masked index is within the entries checked by `init`, the slot
        // is outside the consumer's readable window because the ring is not full, and
        // the `Release` store below publishes the write.
        unsafe {
            entry_ptr(&self.region, self.head & (self.capacity - 1)).write(tile.to_words());
        }
        self.head = self.head.wrapping_add(1);
        counter(&self.region, HEAD_OFFSET).store(self.head, Ordering::Release);
        Ok(())
    }

    /// The consumer's position. Slots behind it may be reused.
    pub fn consumed(&self) -> u32 {
        counter(&self.region, CONSUMED_OFFSET).load(Ordering::Acquire)
    }

    /// Entries published so far, wrapping.
    pub fn head(&self) -> u32 {
        self.head
    }

    /// A consumer for tests and headless tools. In the app, the consumer is JavaScript.
    pub fn consumer(&self) -> RingConsumer {
        RingConsumer {
            region: Arc::clone(&self.region),
            capacity: self.capacity,
            tail: self.consumed(),
        }
    }
}

/// Consumer side of a delivery ring.
#[derive(Debug)]
pub struct RingConsumer {
    region: Arc<ShmRegion>,
    capacity: u32,
    tail: u32,
}

impl RingConsumer {
    /// Reads the next entry without releasing it.
    pub fn peek(&self) -> Option<TileAnnouncement> {
        let head = counter(&self.region, HEAD_OFFSET).load(Ordering::Acquire);
        if head == self.tail {
            return None;
        }
        // SAFETY: the masked index is within the entries, and the `Acquire` load above
        // synchronizes with the producer's `Release` store for this entry.
        let words = unsafe { entry_ptr(&self.region, self.tail & (self.capacity - 1)).read() };
        Some(TileAnnouncement::from_words(words))
    }

    /// Releases the entry returned by the last [`peek`](Self::peek).
    pub fn advance(&mut self) {
        self.tail = self.tail.wrapping_add(1);
        counter(&self.region, CONSUMED_OFFSET).store(self.tail, Ordering::Release);
    }
}

fn counter(region: &ShmRegion, offset: usize) -> &AtomicU32 {
    // SAFETY: both offsets lie in the header area checked by `init`, are 4-byte aligned
    // within a page-aligned view, and every party only accesses them atomically.
    unsafe { &*region.as_ptr().add(offset).cast::<AtomicU32>() }
}

/// # Safety
///
/// `index` must be below the capacity that `region` was formatted with.
unsafe fn entry_ptr(region: &ShmRegion, index: u32) -> *mut [u32; ENTRY_WORDS] {
    // SAFETY: guaranteed by the caller; `ENTRIES_OFFSET` and `ENTRY_LEN` keep it aligned.
    unsafe {
        region
            .as_ptr()
            .add(ENTRIES_OFFSET + index as usize * ENTRY_LEN)
            .cast()
    }
}

#[cfg(test)]
mod tests {
    use std::thread;

    use super::*;

    fn ring(capacity: u32) -> DeliveryRing {
        DeliveryRing::init(ShmRegion::create(ring_len(capacity)).unwrap(), capacity).unwrap()
    }

    fn tile(n: u32) -> TileAnnouncement {
        TileAnnouncement {
            slot: n,
            page: n.wrapping_mul(3),
            lod: -2,
            x: n ^ 5,
            y: n ^ 9,
            epoch: n / 2,
            render_us: n.rotate_left(3),
            done_us: !n,
        }
    }

    #[test]
    fn rejects_bad_capacities() {
        for capacity in [0, 3, 6] {
            let region = ShmRegion::create(4096).unwrap();
            assert_eq!(
                DeliveryRing::init(region, capacity).unwrap_err(),
                RingError::BadCapacity(capacity)
            );
        }
    }

    #[test]
    fn negative_lods_survive_the_word_encoding() {
        let original = tile(7);
        assert_eq!(TileAnnouncement::from_words(original.to_words()), original);
    }

    #[test]
    fn full_ring_refuses_until_the_consumer_advances() {
        let mut ring = ring(4);
        let mut consumer = ring.consumer();
        for n in 0..4 {
            ring.publish(tile(n)).unwrap();
        }
        assert_eq!(ring.publish(tile(4)), Err(RingFull));

        assert_eq!(consumer.peek(), Some(tile(0)));
        consumer.advance();
        ring.publish(tile(4)).unwrap();
        assert_eq!(ring.publish(tile(5)), Err(RingFull));
    }

    #[test]
    fn counters_survive_wrapping_at_u32_max() {
        let mut ring = ring(4);
        // Jump both sides close to the wrap point, as if billions of tiles had passed.
        let start = u32::MAX - 1;
        ring.head = start;
        counter(&ring.region, HEAD_OFFSET).store(start, Ordering::Release);
        counter(&ring.region, CONSUMED_OFFSET).store(start, Ordering::Release);
        let mut consumer = ring.consumer();

        for n in 0..16 {
            ring.publish(tile(n)).unwrap();
            assert_eq!(consumer.peek(), Some(tile(n)));
            consumer.advance();
        }
        assert_eq!(consumer.peek(), None);
    }

    /// JavaScript reads this memory by raw offset, in `apps/desktop/src/protocol.ts`.
    #[test]
    fn layout_matches_what_javascript_reads_by_offset() {
        let mut ring = ring(4);
        ring.publish(tile(1)).unwrap();
        ring.publish(tile(2)).unwrap();

        let base = ring.region.as_ptr();
        // SAFETY: all offsets are inside the region, and this test is single-threaded.
        let word = |offset: usize| unsafe { base.add(offset).cast::<u32>().read() };
        assert_eq!(word(0x00), RING_MAGIC);
        assert_eq!(word(0x08), 4, "capacity");
        assert_eq!(word(0x0C), 32, "entry length");
        assert_eq!(word(0x40), 2, "head");
        assert_eq!(word(0x80), 0, "consumed");
        assert_eq!(word(0xC0), tile(1).slot);
        assert_eq!(word(0xC0 + 32), tile(2).slot);
        assert_eq!(word(0xC0 + 32 + 7 * 4), tile(2).done_us);
    }

    #[test]
    fn concurrent_producer_and_consumer_preserve_order() {
        const COUNT: u32 = 2_000_000;
        let mut ring = ring(64);
        let mut consumer = ring.consumer();

        let producer = thread::spawn(move || {
            let mut next = 0;
            while next < COUNT {
                if ring.publish(tile(next)).is_ok() {
                    next += 1;
                } else {
                    std::hint::spin_loop();
                }
            }
        });

        let mut expected = 0;
        while expected < COUNT {
            if let Some(entry) = consumer.peek() {
                assert_eq!(entry, tile(expected), "entry torn or out of order");
                consumer.advance();
                expected += 1;
            } else {
                std::hint::spin_loop();
            }
        }
        producer.join().unwrap();
    }
}

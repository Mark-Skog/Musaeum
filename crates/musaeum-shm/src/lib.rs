//! Shared-memory layout and control-plane protocol between the Musaeum core and
//! its isolated render workers.
//!
//! The boundary has three parts, and the separation between them is deliberate:
//!
//! * [`slab`] is the **data plane**: raw RGBA pixels in an OS shared-memory
//!   section. PDFium writes straight into it, so no pixel is ever serialized.
//! * [`wire`] is the **control plane**: fixed-size binary messages over the
//!   worker's stdio pipes. They carry slot numbers and timings, never pixels.
//! * [`ring`] is the **delivery ring** that announces finished tiles to the
//!   WebView's main thread on Path B. Render workers never map it, so a
//!   compromised worker can only corrupt pixels, not the delivery bookkeeping.
//!
//! [`tile`] holds the tile arithmetic both sides of the boundary must agree on, [`content`] how a
//! page's measurements become its class, and [`text`] the layout of a text layer in a slot.

#[cfg(not(windows))]
compile_error!("musaeum-shm only implements shared memory for Windows in Phase 0.");

pub mod content;
pub mod region;
pub mod ring;
pub mod slab;
pub mod text;
pub mod tile;
pub mod wire;

pub use content::{ContentClass, PageContent};
pub use region::ShmRegion;
pub use ring::{DeliveryRing, RingConsumer, TileAnnouncement};
pub use slab::PixelSlab;
pub use text::{TextLayer, Word};
pub use tile::{PageGeometry, TILE_BYTES, TILE_SIZE, TILE_STRIDE, TileKey};

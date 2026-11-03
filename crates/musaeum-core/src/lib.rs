//! Tile scheduling and render-worker supervision.
//!
//! Nothing in this crate knows about Tauri or a WebView, so the whole pipeline up to
//! the delivery boundary can be driven and measured from a plain test or command-line
//! tool. [`scheduler`] is a pure state machine; [`pool`] gives it processes and pipes.

pub mod demand;
pub mod geometry;
pub mod pool;
pub mod scheduler;
pub mod stats;

pub use demand::{Demand, DemandError, encode_demand, parse_demand};
pub use geometry::{
    Direction, DocumentHints, DocumentOrientation, Orientation, PageLayout, geometry_bytes,
    summarize_orientation,
};
pub use pool::{DeliveryMode, Poller, PoolConfig, PoolError, RenderPool, encode_tile_batch};
pub use scheduler::{
    CancelOrder, DispatchOrder, ReadyTile, Scheduler, SchedulerError, TileOutcome,
};
pub use stats::{PoolStats, Summary};

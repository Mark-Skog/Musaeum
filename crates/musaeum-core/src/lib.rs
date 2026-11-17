//! Tile scheduling and render-worker supervision.
//!
//! Nothing in this crate knows about Tauri or a WebView, so the whole pipeline up to
//! the delivery boundary can be driven and measured from a plain test or command-line
//! tool. [`scheduler`] is a pure state machine; [`pool`] gives it processes and pipes.

pub mod demand;
pub mod format;
pub mod geometry;
pub mod pool;
pub mod scheduler;
pub mod stats;
pub mod thumbnail;

pub use demand::{Demand, DemandError, encode_demand, parse_demand};
pub use format::FixedFormat;
pub use geometry::{
    Direction, DocumentHints, DocumentOrientation, Orientation, PageLayout, geometry_bytes,
    summarize_orientation,
};
pub use pool::{
    BackgroundResult, BackgroundSink, BackgroundTask, BackgroundWork, DeliveryMode,
    MAX_REGION_PIXELS, Poller, PoolConfig, PoolError, PoolHandle, RegionPicture, RegionRequest,
    RenderPool, TextError, TextReply, encode_tile_batch,
};
pub use scheduler::{
    CancelOrder, DispatchOrder, NO_SLOT, ReadyTile, Scheduler, SchedulerError, SideResult,
    TileOutcome, Work,
};
pub use stats::{PoolStats, Summary};

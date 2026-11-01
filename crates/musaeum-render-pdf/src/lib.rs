//! Rasterizes PDF tiles with PDFium, in a process that does nothing else.
//!
//! The binary `musaeum-render-pdf` is the worker itself. Everything both it and the sandbox
//! test probe need lives here, so the probe starts up along the same code path as the worker.

pub mod engine;
pub mod startup;

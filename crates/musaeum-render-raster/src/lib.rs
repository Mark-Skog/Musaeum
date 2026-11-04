//! Renders comic book archives, CBZ and CBR, in a process that does nothing else.
//!
//! The binary `musaeum-render-raster` is the worker. libarchive reads the archive from the mapped
//! bytes the host shared, and Rust decoders turn each page into pixels, so nothing in this process
//! opens a file, touches the network or runs C code on image data.

pub mod comic;
pub mod order;

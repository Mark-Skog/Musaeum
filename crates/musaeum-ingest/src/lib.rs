//! What Musaeum reads out of a book's bytes: stable identifiers, basic metadata, and the normalized
//! form of flowing books.
//!
//! A book is hostile input, and everything that interprets one runs in a process with no network
//! and no way to open a file. So this crate is two things: [`parse`], which is ordinary safe Rust
//! with a limit on everything it reads, and [`worker`], the process that runs it in a sandbox and
//! gets each file as a read-only handle from the host. The host lives in `musaeum-library`, and
//! treats what comes back as untrusted too: with metadata, all a compromised worker can do is lie
//! about a title; a normalized book has to be well-formed and inert when the host reads it again, or
//! it is refused.

pub mod parse;
pub mod protocol;
pub mod worker;

/// Bumped when extraction starts finding things it did not before, so that what was read by an
/// older version can be read again. Version 2 reads a PDF's `/PageLayout` and `/Direction`; version 3
/// reads the metadata of MOBI, AZW3 and FB2 files.
pub const EXTRACTOR_VERSION: i64 = 3;

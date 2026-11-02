//! Everything the library asks of the disk, behind one seam.
//!
//! In normal use this is just `musaeum-fs`. A test can stand in front of it to report what no test
//! machine can produce on demand, such as a second volume or a File ID that Windows reused.

use std::io;
use std::path::Path;

use musaeum_fs::{ContentFile, Listing, OpenError, ReadPattern, Stat};

/// How the library looks at the disk.
pub trait FsProbe: Send + Sync + 'static {
    /// Lists a directory, opening nothing in it.
    fn list_directory(&self, path: &Path) -> io::Result<Listing>;

    /// What a path is right now, without reading it.
    fn stat(&self, path: &Path) -> io::Result<Stat>;

    /// Opens a file to read its bytes, unless they are in the cloud.
    fn open_content(&self, path: &Path, pattern: ReadPattern) -> Result<ContentFile, OpenError>;
}

/// The actual disk.
#[derive(Clone, Copy, Debug, Default)]
pub struct RealFileSystem;

impl FsProbe for RealFileSystem {
    fn list_directory(&self, path: &Path) -> io::Result<Listing> {
        musaeum_fs::list_directory(path)
    }

    fn stat(&self, path: &Path) -> io::Result<Stat> {
        musaeum_fs::stat(path)
    }

    fn open_content(&self, path: &Path, pattern: ReadPattern) -> Result<ContentFile, OpenError> {
        musaeum_fs::open_content(path, pattern)
    }
}

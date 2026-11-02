//! What the library needs from the file system that `std::fs` does not offer.
//!
//! - **Identity.** A file is known by its volume and a 128-bit File ID, which ReFS requires and
//!   NTFS fits into. Volumes whose IDs do not survive a remount report none at all, so the layers
//!   above cannot trust an ID that means nothing.
//! - **Listing opens nothing.** [`list_directory`] gets each entry's ID, size, date and attributes
//!   from the directory itself. Scanning a library therefore cannot touch a single file, which is
//!   what makes a scan cheap and keeps it from downloading cloud placeholders.
//! - **Reading refuses placeholders.** [`open_content`] checks the attributes before it opens
//!   anything, and again once it has the file open, when they can no longer change.
//! - **Moving never overwrites.** [`move_no_replace`] fails if the target exists.
//! - **Watching admits its limits.** [`watch_directory`] says when the system lost track of changes.

#[cfg(not(windows))]
compile_error!("musaeum-fs only implements Windows in Phase 1.");

mod list;
mod move_file;
mod open;
mod path;
mod priority;
mod stat;
mod volume;
mod watch;

pub use list::{Entry, EntryKind, Listing, list_directory};
pub use move_file::{MoveError, move_no_replace};
pub use open::{ContentFile, OpenError, ReadPattern, open_content};
pub use priority::{BackgroundPriority, enter_background};
pub use stat::{Stat, attributes_of, is_cloud_only, stat};
pub use volume::has_recycle_bin;
pub use watch::{DEFAULT_WATCH_BUFFER, DirectoryWatch, WatchEvent, watch_directory};

/// A file's ID within its volume. NTFS uses 64 of the 128 bits, ReFS all of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FileId(pub [u8; 16]);

impl FileId {
    /// `None` for the values file systems use to say that a file has no ID.
    pub(crate) fn from_raw(bytes: [u8; 16]) -> Option<Self> {
        (bytes != [0; 16] && bytes != [0xFF; 16]).then_some(Self(bytes))
    }
}

/// Nanoseconds since the Unix epoch, from the 100 ns ticks since 1601 that Windows counts in.
pub(crate) fn unix_nanos(filetime: i64) -> i64 {
    const TICKS_TO_UNIX_EPOCH: i64 = 116_444_736_000_000_000;
    filetime
        .saturating_sub(TICKS_TO_UNIX_EPOCH)
        .saturating_mul(100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_convert_to_the_unix_epoch() {
        assert_eq!(unix_nanos(116_444_736_000_000_000), 0);
        assert_eq!(unix_nanos(116_444_736_000_000_001), 100);
    }

    #[test]
    fn the_values_for_no_id_are_not_ids() {
        assert_eq!(FileId::from_raw([0; 16]), None);
        assert_eq!(FileId::from_raw([0xFF; 16]), None);
        assert!(FileId::from_raw([1; 16]).is_some());
    }
}

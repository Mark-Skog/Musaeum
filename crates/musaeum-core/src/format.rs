//! Which worker reads a fixed-layout file, decided from its first bytes rather than its name: a
//! comic named `.cbr` is often a ZIP, and a name can claim anything.

use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::windows::fs::OpenOptionsExt;
use std::path::Path;

use musaeum_sandbox::Policy;
use serde::Serialize;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
};

/// How far into a file to look. PDF lets its header start anywhere in the first 1024 bytes.
pub const SNIFF_LEN: usize = 1024;

/// The fixed-layout formats, by the worker that reads each.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FixedFormat {
    /// PDF, through PDFium.
    Pdf,
    /// A comic book archive, CBZ or CBR, through libarchive and Rust image decoders.
    Comic,
    /// DjVu, through DjVuLibre.
    Djvu,
}

impl FixedFormat {
    /// The format of a file that begins with `head`, or `None` if it is none of these.
    pub fn sniff(head: &[u8]) -> Option<Self> {
        if head.starts_with(b"AT&TFORM") {
            return Some(Self::Djvu);
        }
        if head.starts_with(b"PK\x03\x04")
            || head.starts_with(b"PK\x05\x06")
            || head.starts_with(b"Rar!\x1a\x07")
        {
            return Some(Self::Comic);
        }
        let head = &head[..head.len().min(SNIFF_LEN)];
        if head.windows(5).any(|window| window == b"%PDF-") {
            return Some(Self::Pdf);
        }
        None
    }

    /// The format of the file at `path`, from its first bytes. Reading them interprets nothing.
    pub fn of_file(path: &Path) -> io::Result<Option<Self>> {
        let file = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .open(path)?;
        let mut head = Vec::with_capacity(SNIFF_LEN);
        <File as Read>::take(file, SNIFF_LEN as u64).read_to_end(&mut head)?;
        Ok(Self::sniff(&head))
    }

    /// The worker's executable, which ships next to the app's.
    pub fn worker_exe(self) -> &'static str {
        match self {
            Self::Pdf => "musaeum-render-pdf.exe",
            Self::Comic => "musaeum-render-raster.exe",
            Self::Djvu => "musaeum-render-djvu.exe",
        }
    }

    /// The strictest sandbox the worker can run in. The PDFium build in use needs win32k and is a
    /// DLL Microsoft did not sign; the other workers link everything they use.
    pub fn policy(self) -> Policy {
        match self {
            Self::Pdf => Policy::Lockdown,
            Self::Comic | Self::Djvu => Policy::LockdownParser,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_are_told_apart_by_their_bytes() {
        assert_eq!(FixedFormat::sniff(b"%PDF-1.7\n"), Some(FixedFormat::Pdf));
        let mut late = vec![b' '; 900];
        late.extend_from_slice(b"%PDF-1.4");
        assert_eq!(
            FixedFormat::sniff(&late),
            Some(FixedFormat::Pdf),
            "junk first is allowed"
        );
        let mut too_late = vec![b' '; SNIFF_LEN];
        too_late.extend_from_slice(b"%PDF-1.4");
        assert_eq!(FixedFormat::sniff(&too_late), None);
        assert_eq!(
            FixedFormat::sniff(b"PK\x03\x04rest"),
            Some(FixedFormat::Comic)
        );
        assert_eq!(
            FixedFormat::sniff(b"Rar!\x1a\x07\x01\x00"),
            Some(FixedFormat::Comic)
        );
        assert_eq!(
            FixedFormat::sniff(b"Rar!\x1a\x07\x00"),
            Some(FixedFormat::Comic)
        );
        assert_eq!(
            FixedFormat::sniff(b"AT&TFORM\x00\x00"),
            Some(FixedFormat::Djvu)
        );
        assert_eq!(FixedFormat::sniff(b"<html>"), None);
        assert_eq!(FixedFormat::sniff(b""), None);
    }

    #[test]
    fn only_pdfium_needs_the_looser_sandbox() {
        assert_eq!(FixedFormat::Pdf.policy(), Policy::Lockdown);
        assert_eq!(FixedFormat::Comic.policy(), Policy::LockdownParser);
        assert_eq!(FixedFormat::Djvu.policy(), Policy::LockdownParser);
    }
}

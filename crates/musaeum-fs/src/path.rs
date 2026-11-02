//! Paths the Windows API takes exactly as written.

use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::{Component, Path};

const VERBATIM: [u16; 4] = [b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16];
const DEVICE: [u16; 4] = [b'\\' as u16, b'\\' as u16, b'.' as u16, b'\\' as u16];
const UNC_PREFIX: [u16; 8] = [
    b'\\' as u16,
    b'\\' as u16,
    b'?' as u16,
    b'\\' as u16,
    b'U' as u16,
    b'N' as u16,
    b'C' as u16,
    b'\\' as u16,
];

/// The path as a null-terminated verbatim path. Without this, Windows cuts paths off at 260
/// characters, and quietly drops the trailing dots and spaces a real file name can have.
pub(crate) fn verbatim(path: &Path) -> io::Result<Vec<u16>> {
    let needs_resolving = !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::CurDir | Component::ParentDir));
    let absolute;
    let path = if needs_resolving {
        absolute = std::path::absolute(path)?;
        absolute.as_path()
    } else {
        path
    };
    let wide: Vec<u16> = path.as_os_str().encode_wide().collect();

    let mut result = Vec::with_capacity(wide.len() + UNC_PREFIX.len() + 1);
    if wide.starts_with(&VERBATIM) || wide.starts_with(&DEVICE) {
        result.extend_from_slice(&wide);
    } else if wide.starts_with(&VERBATIM[..2]) {
        result.extend_from_slice(&UNC_PREFIX);
        result.extend(wide[2..].iter().map(|&unit| backslash(unit)));
    } else {
        result.extend_from_slice(&VERBATIM);
        result.extend(wide.iter().map(|&unit| backslash(unit)));
    }
    result.push(0);
    Ok(result)
}

/// A verbatim path takes no forward slashes, which a path typed by hand may have.
fn backslash(unit: u16) -> u16 {
    if unit == u16::from(b'/') {
        u16::from(b'\\')
    } else {
        unit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(path: &str) -> String {
        let wide = verbatim(Path::new(path)).unwrap();
        assert_eq!(wide.last(), Some(&0));
        String::from_utf16(&wide[..wide.len() - 1]).unwrap()
    }

    #[test]
    fn a_drive_path_gains_the_prefix() {
        assert_eq!(text(r"C:\Books\a.pdf"), r"\\?\C:\Books\a.pdf");
        assert_eq!(text("C:/Books/a.pdf"), r"\\?\C:\Books\a.pdf");
    }

    #[test]
    fn a_network_path_uses_the_unc_form() {
        assert_eq!(text(r"\\server\share\a.pdf"), r"\\?\UNC\server\share\a.pdf");
    }

    #[test]
    fn a_verbatim_path_stays_as_it_is() {
        assert_eq!(text(r"\\?\C:\Books\a."), r"\\?\C:\Books\a.");
    }

    #[test]
    fn a_trailing_dot_survives() {
        assert_eq!(text(r"C:\Books\a."), r"\\?\C:\Books\a.");
    }

    #[test]
    fn parent_references_are_resolved() {
        assert_eq!(text(r"C:\Books\x\..\a.pdf"), r"\\?\C:\Books\a.pdf");
    }
}

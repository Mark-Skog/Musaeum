//! Builds the strings `CreateProcessAsUserW` wants: a command line and an environment block.

use std::ffi::{OsStr, OsString};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

/// Appends one argument, quoted the way the C runtime and Rust's `std::env::args` parse it:
/// backslashes only need doubling when a quote follows them.
fn push_argument(line: &mut Vec<u16>, argument: &OsStr) {
    let quote = u16::from(b'"');
    let backslash = u16::from(b'\\');
    let units: Vec<u16> = argument.encode_wide().collect();

    line.push(quote);
    let mut pending_backslashes = 0;
    for &unit in &units {
        if unit == backslash {
            pending_backslashes += 1;
        } else {
            if unit == quote {
                line.extend(std::iter::repeat_n(backslash, pending_backslashes + 1));
            }
            pending_backslashes = 0;
        }
        line.push(unit);
    }
    line.extend(std::iter::repeat_n(backslash, pending_backslashes));
    line.push(quote);
}

/// A null-terminated command line for `exe` and `args`.
pub(crate) fn command_line(exe: &Path, args: &[OsString]) -> io::Result<Vec<u16>> {
    let mut line = Vec::new();
    push_argument(&mut line, exe.as_os_str());
    for argument in args {
        line.push(u16::from(b' '));
        push_argument(&mut line, argument);
    }
    if line.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a command line cannot contain a null character",
        ));
    }
    line.push(0);
    Ok(line)
}

/// A null-terminated wide string.
pub(crate) fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(std::iter::once(0)).collect()
}

/// The environment of a locked-down process. Only what the system's own DLLs look at, so
/// nothing in the user's environment, such as tokens or paths, reaches a parser.
pub(crate) fn minimal_environment() -> Vec<u16> {
    let mut block = Vec::new();
    for name in ["SystemRoot", "SystemDrive", "windir"] {
        if let Some(value) = std::env::var_os(name) {
            block.extend(OsStr::new(name).encode_wide());
            block.push(u16::from(b'='));
            block.extend(value.encode_wide());
            block.push(0);
        }
    }
    // An empty block still needs its own terminator, on top of the last variable's.
    block.push(0);
    block.push(0);
    block
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(args: &[&str]) -> String {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        let units = command_line(Path::new(r"C:\a b\x.exe"), &args).unwrap();
        String::from_utf16(&units[..units.len() - 1]).unwrap()
    }

    #[test]
    fn quotes_every_argument() {
        assert_eq!(line(&[]), r#""C:\a b\x.exe""#);
        assert_eq!(
            line(&["one", "two words"]),
            r#""C:\a b\x.exe" "one" "two words""#
        );
    }

    #[test]
    fn escapes_quotes_and_the_backslashes_before_them() {
        assert_eq!(line(&[r#"say "hi""#]), r#""C:\a b\x.exe" "say \"hi\"""#);
        assert_eq!(line(&[r#"a\"b"#]), r#""C:\a b\x.exe" "a\\\"b""#);
    }

    #[test]
    fn doubles_trailing_backslashes_because_a_quote_follows() {
        assert_eq!(line(&[r"C:\dir\"]), r#""C:\a b\x.exe" "C:\dir\\""#);
    }

    #[test]
    fn leaves_inner_backslashes_alone() {
        assert_eq!(line(&[r"C:\one\two"]), r#""C:\a b\x.exe" "C:\one\two""#);
    }

    #[test]
    fn refuses_a_null_character() {
        let args = [OsString::from("a\0b")];
        assert!(command_line(Path::new("x.exe"), &args).is_err());
    }

    #[test]
    fn the_environment_holds_only_system_locations_and_is_terminated() {
        let block = minimal_environment();
        assert!(block.ends_with(&[0, 0]));
        let text = String::from_utf16(&block).unwrap();
        for variable in text.split('\0').filter(|variable| !variable.is_empty()) {
            let name = variable.split('=').next().unwrap();
            assert!(
                ["SystemRoot", "SystemDrive", "windir"].contains(&name),
                "{name}"
            );
        }
    }
}

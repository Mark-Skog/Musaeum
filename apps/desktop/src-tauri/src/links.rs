//! Opening a book's web link in the default browser, only when the reader asks for it.
//!
//! The reader intercepts every link click and the page shows the address (ADR 0006, D12). The
//! address comes from the book, so it is checked again here: only `http` and `https`, a plain
//! host, no credentials, nothing a shell could read as something else. A run with `--e2e`
//! records the address instead of starting a browser on the machine that runs the test.

use std::sync::Mutex;

use musaeum_i18n::tr;
use tauri::State;

use crate::session::AppState;

/// The longest address the reader sends (`MAX_HREF` in `reader/messages.ts`).
const MAX_URL: usize = 2048;

/// The addresses a test run asked to open, oldest first.
#[derive(Default)]
pub struct OpenedLinks(Mutex<Vec<String>>);

/// Returns the address if it is a web address the browser can be given as it is.
pub fn browser_url(text: &str) -> Option<&str> {
    if text.is_empty() || text.len() > MAX_URL {
        return None;
    }
    // Only printable ASCII: an IRI's other characters come percent-encoded from the book's link,
    // and a space or quote would let the shell see two arguments.
    if !text.bytes().all(|byte| (0x21..0x7f).contains(&byte)) {
        return None;
    }
    if text.contains(['"', '\'', '<', '>', '\\', '^', '`', '{', '|', '}']) {
        return None;
    }
    let scheme_end = text.find("://")?;
    let scheme = &text[..scheme_end];
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let rest = &text[scheme_end + 3..];
    let authority = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    // Credentials in the address only serve to make one host look like another.
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    let host = match authority.strip_prefix('[') {
        Some(inner) => {
            let (address, port) = inner.split_once(']')?;
            if address.is_empty()
                || !address
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() || byte == b':' || byte == b'.')
            {
                return None;
            }
            if !port.is_empty() && !is_port(port.strip_prefix(':')?) {
                return None;
            }
            return Some(text);
        }
        None => match authority.rsplit_once(':') {
            Some((host, port)) if is_port(port) => host,
            Some(_) => return None,
            None => authority,
        },
    };
    let labels_ok = !host.is_empty()
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        });
    labels_ok.then_some(text)
}

fn is_port(text: &str) -> bool {
    !text.is_empty() && text.len() <= 5 && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// Opens a web address from a book in the default browser. The page calls it only from the
/// button the reader presses in the link dialog.
#[tauri::command(async)]
pub fn open_in_browser(
    state: State<'_, AppState>,
    opened: State<'_, OpenedLinks>,
    url: String,
) -> Result<(), String> {
    let Some(url) = browser_url(&url) else {
        return Err(tr!(LinkNotWeb));
    };
    if state.is_e2e() {
        opened
            .0
            .lock()
            .map_err(|_| "poisoned".to_owned())?
            .push(url.to_owned());
        return Ok(());
    }
    open(url)
}

/// The addresses a test run asked to open. Only a run with `--e2e` has them.
#[tauri::command(async)]
pub fn e2e_opened_links(
    state: State<'_, AppState>,
    opened: State<'_, OpenedLinks>,
) -> Result<Vec<String>, String> {
    if !state.is_e2e() {
        return Err("not running a test".to_owned());
    }
    Ok(opened.0.lock().map_err(|_| "poisoned".to_owned())?.clone())
}

#[cfg(windows)]
fn open(url: &str) -> Result<(), String> {
    use windows::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
    };
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{HSTRING, PCWSTR, w};

    let target = HSTRING::from(url);
    // ShellExecute may hand the address to a COM server, so it runs on a thread of its own with
    // COM set up the way its documentation asks.
    std::thread::spawn(move || {
        // SAFETY: the thread initializes COM once and uninitializes it before it ends.
        let initialized =
            unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) }
                .is_ok();
        // SAFETY: every pointer is to a string that lives until the call returns; the address
        // starts with http:// or https://, so the shell hands it to the protocol's handler.
        let result = unsafe {
            ShellExecuteW(
                None,
                w!("open"),
                &target,
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        if initialized {
            // SAFETY: pairs the successful CoInitializeEx above, on the same thread.
            unsafe { CoUninitialize() };
        }
        // Values above 32 mean success.
        if result.0 as usize > 32 {
            Ok(())
        } else {
            Err(tr!(BrowserDidNotOpenCode, code = result.0 as usize))
        }
    })
    .join()
    .map_err(|_| tr!(BrowserDidNotOpen))?
}

#[cfg(not(windows))]
fn open(_url: &str) -> Result<(), String> {
    Err(tr!(BrowserWindowsOnly))
}

#[cfg(test)]
mod tests {
    use super::browser_url;

    #[test]
    fn web_addresses_pass_as_they_are() {
        for url in [
            "https://example.org",
            "http://example.org/",
            "HTTPS://Example.ORG/a/b?c=d&e=f#g",
            "https://www.gutenberg.org/ebooks/2701",
            "https://example.org:8443/path",
            "https://[2001:db8::1]/x",
            "https://[2001:db8::1]:8080",
            "https://pt.wikipedia.org/wiki/Mem%C3%B3rias_P%C3%B3stumas",
            "http://localhost:3000",
        ] {
            assert_eq!(browser_url(url), Some(url), "{url}");
        }
    }

    #[test]
    fn anything_else_is_refused() {
        for url in [
            "",
            "mailto:autor@example.org",
            "tel:+5511999999999",
            "file:///C:/Windows/System32/calc.exe",
            "javascript:alert(1)",
            "C:\\Windows\\System32\\calc.exe",
            "calc.exe",
            "ms-settings:privacy",
            "https://",
            "https:///path",
            "https://user:pass@example.org",
            "https://example.org@evil.example",
            "https://exa mple.org",
            "https://example.org/\" --new-window",
            "https://example.org/a b",
            "https://example.org/\u{0}",
            "https://exam\u{301}ple.org",
            "https://example.org:",
            "https://example.org:99999999",
            "https://-example.org",
            "https://example..org",
            "https://example.org\\..\\..",
            "https://[]/",
            "https://[zz]/",
        ] {
            assert_eq!(browser_url(url), None, "{url:?}");
        }
        assert_eq!(
            browser_url(&format!("https://example.org/{}", "a".repeat(2048))),
            None
        );
    }
}

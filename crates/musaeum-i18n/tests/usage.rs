//! Every message of the catalog is written somewhere: none is left behind when the code that
//! wrote it goes, and none is added that nothing says.

use std::fs;
use std::path::{Path, PathBuf};

use musaeum_i18n::T;

/// The Rust sources of the workspace, but this crate's catalog.
fn sources() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut out = Vec::new();
    let mut visit = vec![root.join("crates"), root.join("apps/desktop/src-tauri/src")];
    while let Some(folder) = visit.pop() {
        let Ok(entries) = fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            if path.is_dir() {
                if name != "target" && name != "tests" {
                    visit.push(path);
                }
            } else if path.extension().is_some_and(|ext| ext == "rs")
                && !path.ends_with("musaeum-i18n/src/catalog.rs")
            {
                out.push(path);
            }
        }
    }
    out
}

#[test]
fn every_message_is_used() {
    let text: String = sources()
        .iter()
        .filter_map(|path| fs::read_to_string(path).ok())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.len() > 100_000, "the sources were found");
    let is_word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let used = |name: &str| {
        // `T::Name`, `tr!(Name`, or `tr_in!(locale, Name`: the name as a whole word after one of them.
        text.match_indices(name).any(|(at, _)| {
            let before = text[..at].trim_end();
            let after = text[at + name.len()..].chars().next();
            after.is_none_or(|c| !is_word(c))
                && (before.ends_with("T::")
                    || before.ends_with("tr!(")
                    || (before.ends_with(',')
                        && before
                            .rsplit("tr_in!(")
                            .next()
                            .is_some_and(|args| !args.contains(')'))))
        })
    };
    let unused: Vec<String> = T::ALL
        .iter()
        .map(|message| format!("{message:?}"))
        .filter(|name| !used(name))
        .collect();
    assert!(unused.is_empty(), "messages nothing writes: {unused:?}");
}

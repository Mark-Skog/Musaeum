//! Walks directories and writes down what is there. It reads no files and changes nothing.

use std::collections::{HashSet, VecDeque};
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};

use musaeum_fs::{Entry, EntryKind, FileId};

use crate::model::FolderId;
use crate::probe::FsProbe;

// `FILE_ATTRIBUTE_*`, which only this one check needs.
const HIDDEN: u32 = 0x2;
const SYSTEM: u32 = 0x4;

const BUILT_IN_EXCLUSIONS: &[&str] = &[
    "$recycle.bin",
    "system volume information",
    ".git",
    "node_modules",
    // Where Office and others keep their lock files.
    "~$*",
];

/// The formats in the plan, by extension.
const FORMATS: &[(&str, &str)] = &[
    ("pdf", "fixed"),
    ("epub", "flow"),
    ("mobi", "flow"),
    ("azw", "flow"),
    ("azw3", "flow"),
    ("fb2", "flow"),
    ("cbz", "fixed"),
    ("cbr", "fixed"),
    ("djvu", "fixed"),
    ("djv", "fixed"),
];

/// The format of a file with this name, if it is one the library handles.
pub(crate) fn format_of(name: &OsStr) -> Option<String> {
    let extension = Path::new(name).extension()?.to_str()?.to_ascii_lowercase();
    layout_of(&extension).map(|_| extension)
}

/// How a format is laid out.
pub(crate) fn layout_of(format: &str) -> Option<&'static str> {
    FORMATS
        .iter()
        .find(|(known, _)| *known == format)
        .map(|(_, layout)| *layout)
}

/// Names that a root leaves out.
#[derive(Clone, Debug, Default)]
pub(crate) struct Exclusions {
    patterns: Vec<Vec<char>>,
}

impl Exclusions {
    /// The built-in ones, plus these. `*` and `?` work as usual, and case does not matter.
    pub fn new(user_patterns: &[String]) -> Self {
        let patterns = BUILT_IN_EXCLUSIONS
            .iter()
            .copied()
            .chain(user_patterns.iter().map(String::as_str))
            .map(|pattern| pattern.to_lowercase().chars().collect())
            .collect();
        Self { patterns }
    }

    pub fn excludes(&self, name: &OsStr) -> bool {
        let lower: Vec<char> = name.to_string_lossy().to_lowercase().chars().collect();
        self.patterns
            .iter()
            .any(|pattern| wildcard_matches(pattern, &lower))
    }
}

/// `*` and `?`, without recursion, so that no pattern can take exponential time.
fn wildcard_matches(pattern: &[char], text: &[char]) -> bool {
    let (mut p, mut t) = (0, 0);
    let mut backtrack = None;
    while t < text.len() {
        match pattern.get(p) {
            Some('*') => {
                backtrack = Some((p + 1, t));
                p += 1;
            }
            Some(&character) if character == '?' || character == text[t] => {
                p += 1;
                t += 1;
            }
            _ => match backtrack {
                Some((after_star, tried)) => {
                    p = after_star;
                    t = tried + 1;
                    backtrack = Some((after_star, tried + 1));
                }
                None => return false,
            },
        }
    }
    pattern[p..].iter().all(|&character| character == '*')
}

/// Where a walked directory hangs in the tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WalkParent {
    /// It is the root's own folder.
    Top,
    /// Its parent was walked too, at this index.
    Walked(usize),
    /// Its parent was not walked, and it is this folder.
    Known(FolderId),
}

/// A directory, with what the scan cares about in it.
#[derive(Debug)]
pub(crate) struct WalkedDir {
    pub path: PathBuf,
    pub parent: WalkParent,
    pub name: OsString,
    pub volume: u64,
    pub directory_id: Option<FileId>,
    /// The files in formats the library handles.
    pub files: Vec<Entry>,
    /// The subdirectories that exist, whether or not they were walked. Whatever a folder used to
    /// have and is not here went away.
    pub subdirectories: HashSet<OsString>,
}

/// Where a walk starts.
#[derive(Clone, Debug)]
pub(crate) struct WalkStart {
    pub path: PathBuf,
    pub parent: WalkParent,
    pub name: OsString,
}

/// What a walk found.
#[derive(Debug, Default)]
pub(crate) struct Walk {
    /// Parents always come before their children.
    pub dirs: Vec<WalkedDir>,
    /// How many directories exist and could not be listed. What is in them is left as it was.
    pub unreadable_directories: u64,
    /// Starting points that do not exist any more, which is news for their parents.
    pub missing_starts: Vec<PathBuf>,
}

/// Says whether a walk goes into a subdirectory, given its path, the volume, the ID of the directory
/// being listed and the subdirectory's entry.
pub(crate) type Descend<'a> = dyn Fn(&Path, u64, Option<FileId>, &Entry) -> bool + 'a;

/// Lists `starts` and then whichever subdirectories `descend` asks for.
pub(crate) fn walk(
    probe: &dyn FsProbe,
    exclusions: &Exclusions,
    starts: Vec<WalkStart>,
    descend: &Descend<'_>,
) -> io::Result<Walk> {
    let mut result = Walk::default();
    let mut queue: VecDeque<WalkStart> = starts.into();
    let mut visited = HashSet::new();

    while let Some(start) = queue.pop_front() {
        if !visited.insert(start.path.clone()) {
            continue;
        }
        let listing = match probe.list_directory(&start.path) {
            Ok(listing) => listing,
            Err(error) if is_gone(&error) => {
                result.missing_starts.push(start.path);
                continue;
            }
            Err(error) if start.parent == WalkParent::Top => return Err(error),
            Err(_) => {
                result.unreadable_directories += 1;
                continue;
            }
        };

        let mut files = Vec::new();
        let mut subdirectories = HashSet::new();
        let index = result.dirs.len();
        for entry in listing.entries {
            if exclusions.excludes(&entry.name)
                || entry.attributes & (HIDDEN | SYSTEM) == HIDDEN | SYSTEM
            {
                continue;
            }
            match entry.kind {
                EntryKind::File if format_of(&entry.name).is_some() => files.push(entry),
                EntryKind::Directory => {
                    let path = start.path.join(&entry.name);
                    if descend(&path, listing.volume, listing.directory_id, &entry) {
                        queue.push_back(WalkStart {
                            path,
                            parent: WalkParent::Walked(index),
                            name: entry.name.clone(),
                        });
                    }
                    subdirectories.insert(entry.name);
                }
                EntryKind::File | EntryKind::Link => {}
            }
        }
        result.dirs.push(WalkedDir {
            path: start.path,
            parent: start.parent,
            name: start.name,
            volume: listing.volume,
            directory_id: listing.directory_id,
            files,
            subdirectories,
        });
    }

    forget_what_vanished_midway(&mut result);
    Ok(result)
}

fn is_gone(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    )
}

/// A subdirectory gone by the time it was listed is not there, which its parent has to know.
fn forget_what_vanished_midway(walk: &mut Walk) {
    let missing: Vec<PathBuf> = walk.missing_starts.clone();
    for path in missing {
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
            continue;
        };
        if let Some(dir) = walk.dirs.iter_mut().find(|dir| dir.path == parent) {
            dir.subdirectories.remove(name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches(pattern: &str, text: &str) -> bool {
        let pattern: Vec<char> = pattern.chars().collect();
        let text: Vec<char> = text.chars().collect();
        wildcard_matches(&pattern, &text)
    }

    #[test]
    fn wildcards() {
        assert!(matches("*", ""));
        assert!(matches("*", "anything"));
        assert!(matches("a*c", "abbbc"));
        assert!(matches("a?c", "abc"));
        assert!(!matches("a?c", "ac"));
        assert!(matches("*.tmp", "file.tmp"));
        assert!(!matches("*.tmp", "file.tmp.pdf"));
        assert!(matches("a*b*c", "a--b--b--c"));
        assert!(!matches("a*b*c", "a--c"));
        assert!(matches("~$*", "~$book.pdf"));
    }

    #[test]
    fn a_hard_pattern_takes_no_time() {
        let text = "a".repeat(10_000);
        assert!(!matches(&format!("{}b", "*a".repeat(50)), &text));
    }

    #[test]
    fn exclusions_ignore_case_and_include_the_usual_suspects() {
        let exclusions = Exclusions::new(&["Rascunhos*".to_owned()]);
        assert!(exclusions.excludes(OsStr::new("$RECYCLE.BIN")));
        assert!(exclusions.excludes(OsStr::new("node_modules")));
        assert!(exclusions.excludes(OsStr::new("rascunhos de 2024")));
        assert!(!exclusions.excludes(OsStr::new("Livros")));
    }

    #[test]
    fn formats_go_by_extension_and_ignore_case() {
        assert_eq!(format_of(OsStr::new("a.PDF")).as_deref(), Some("pdf"));
        assert_eq!(format_of(OsStr::new("a.b.epub")).as_deref(), Some("epub"));
        assert_eq!(format_of(OsStr::new("a.tmp")), None);
        assert_eq!(format_of(OsStr::new("pdf")), None);
        assert_eq!(layout_of("djvu"), Some("fixed"));
        assert_eq!(layout_of("epub"), Some("flow"));
    }
}

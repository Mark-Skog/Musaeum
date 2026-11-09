//! References inside a book: what they point at, and how to write them from another place.
//!
//! Paths here are the container's: no leading slash, segments decoded, `.` and `..` resolved, never
//! above the root, which is where a browser would stop too.

/// What a reference in a book points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reference {
    /// A file in the book, by its path from the container's root, and maybe a fragment.
    Internal {
        path: String,
        fragment: Option<String>,
    },
    /// A fragment of the document the reference is in.
    Fragment(String),
    /// Something outside the book, with its scheme in lowercase (empty for `//host`).
    External { scheme: String, url: String },
    /// Nothing at all.
    Empty,
}

/// Decodes `%XX` sequences, keeping anything that is not one, and the bytes as UTF-8.
pub fn percent_decode(text: &str) -> String {
    if !text.contains('%') {
        return text.to_owned();
    }
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) = (
                (bytes[index + 1] as char).to_digit(16),
                (bytes[index + 2] as char).to_digit(16),
            )
        {
            decoded.push((high * 16 + low) as u8);
            index += 3;
            continue;
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// The directory a file is in, with its trailing slash, or empty at the root.
pub fn directory(path: &str) -> &str {
    path.rfind('/').map_or("", |at| &path[..=at])
}

/// `path` with `.`, `..` and empty segments resolved.
pub fn normalize_path(path: &str) -> String {
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            segment => segments.push(segment),
        }
    }
    segments.join("/")
}

fn scheme_of(text: &str) -> Option<&str> {
    let end = text.find(':')?;
    let scheme = &text[..end];
    let mut chars = scheme.chars();
    let first = chars.next()?;
    (first.is_ascii_alphabetic()
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')))
    .then_some(scheme)
}

/// What `href`, written in the file at `base`, points at.
pub fn resolve(base: &str, href: &str) -> Reference {
    // Browsers drop tabs and line breaks anywhere in a URL, trim the rest of the white space, and
    // read backslashes as slashes in the schemes a book is served from.
    let cleaned: String = href
        .trim_matches(|c: char| c.is_ascii_whitespace() || c.is_control())
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .map(|c| if c == '\\' { '/' } else { c })
        .collect();
    if cleaned.is_empty() {
        return Reference::Empty;
    }
    if let Some(fragment) = cleaned.strip_prefix('#') {
        return Reference::Fragment(percent_decode(fragment));
    }
    if let Some(scheme) = scheme_of(&cleaned) {
        return Reference::External {
            scheme: scheme.to_ascii_lowercase(),
            url: cleaned,
        };
    }
    if cleaned.starts_with("//") {
        return Reference::External {
            scheme: String::new(),
            url: cleaned,
        };
    }
    let (before_fragment, fragment) = match cleaned.split_once('#') {
        Some((path, fragment)) => (path, Some(percent_decode(fragment))),
        None => (cleaned.as_str(), None),
    };
    let path_part = before_fragment.split('?').next().unwrap_or("");
    let joined = if let Some(absolute) = path_part.strip_prefix('/') {
        absolute.to_owned()
    } else if path_part.is_empty() {
        base.to_owned()
    } else {
        format!("{}{path_part}", directory(base))
    };
    let path = normalize_path(&percent_decode(&joined));
    if path.is_empty() {
        return Reference::Empty;
    }
    Reference::Internal { path, fragment }
}

fn encode_into(text: &str, keep: impl Fn(u8) -> bool, out: &mut String) {
    for &byte in text.as_bytes() {
        if keep(byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
}

/// A path's segments, percent-encoded wherever a URL path could misread them.
pub fn encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    encode_into(
        path,
        |byte| byte.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@/".contains(&byte),
        &mut out,
    );
    out
}

/// A fragment, encoded so that decoding it gives the identifier back.
pub fn encode_fragment(fragment: &str) -> String {
    let mut out = String::with_capacity(fragment.len());
    for c in fragment.chars() {
        if c.is_ascii() {
            let byte = c as u8;
            if byte.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@/?".contains(&byte) {
                out.push(c);
            } else {
                out.push_str(&format!("%{byte:02X}"));
            }
        } else {
            let mut buffer = [0u8; 4];
            encode_into(c.encode_utf8(&mut buffer), |_| false, &mut out);
        }
    }
    out
}

/// How to reach the file `to` from the file `from`, both paths from the root, encoded.
pub fn relative(from: &str, to: &str) -> String {
    let from_dirs: Vec<&str> = directory(from)
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    let to_parts: Vec<&str> = to.split('/').collect();
    let (to_dirs, to_file) = to_parts.split_at(to_parts.len() - 1);
    let common = from_dirs
        .iter()
        .zip(to_dirs)
        .take_while(|(a, b)| a == b)
        .count();
    let mut parts: Vec<String> = Vec::new();
    for _ in common..from_dirs.len() {
        parts.push("..".to_owned());
    }
    for dir in &to_dirs[common..] {
        parts.push(encode_path(dir));
    }
    parts.push(encode_path(to_file[0]));
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn internal(path: &str, fragment: Option<&str>) -> Reference {
        Reference::Internal {
            path: path.to_owned(),
            fragment: fragment.map(str::to_owned),
        }
    }

    #[test]
    fn references_resolve_like_a_browser() {
        let base = "OEBPS/Text/ch01.xhtml";
        assert_eq!(
            resolve(base, "ch02.xhtml"),
            internal("OEBPS/Text/ch02.xhtml", None)
        );
        assert_eq!(
            resolve(base, "../Images/My%20Cat.jpg"),
            internal("OEBPS/Images/My Cat.jpg", None)
        );
        assert_eq!(
            resolve(base, " ..\\Styles\\a.css?v=1 "),
            internal("OEBPS/Styles/a.css", None)
        );
        assert_eq!(
            resolve(base, "../../../../x.xhtml#a%20b"),
            internal("x.xhtml", Some("a b"))
        );
        assert_eq!(
            resolve(base, "#note-1"),
            Reference::Fragment("note-1".to_owned())
        );
        assert_eq!(resolve(base, "?q"), internal("OEBPS/Text/ch01.xhtml", None));
        assert_eq!(resolve(base, "/OPS/a.xhtml"), internal("OPS/a.xhtml", None));
        assert!(
            matches!(resolve(base, "JavaScript:alert(1)"), Reference::External { scheme, .. } if scheme == "javascript")
        );
        assert!(
            matches!(resolve(base, "java\tscript:alert(1)"), Reference::External { scheme, .. } if scheme == "javascript")
        );
        assert!(matches!(
            resolve(base, "//evil.example/x"),
            Reference::External { .. }
        ));
        assert_eq!(resolve(base, "   "), Reference::Empty);
    }

    #[test]
    fn relative_paths_go_up_and_down() {
        assert_eq!(
            relative("text/0001.xhtml", "images/0002.jpg"),
            "../images/0002.jpg"
        );
        assert_eq!(
            relative("text/0001.xhtml", "text/0003-1.xhtml"),
            "0003-1.xhtml"
        );
        assert_eq!(
            relative("package.opf", "text/a b.xhtml"),
            "text/a%20b.xhtml"
        );
        assert_eq!(encode_fragment("a b#ç"), "a%20b%23%C3%A7");
        assert_eq!(percent_decode(&encode_fragment("a b#ç%")), "a b#ç%");
    }
}

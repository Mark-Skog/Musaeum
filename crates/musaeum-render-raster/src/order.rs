//! Which files in a comic archive are pages, and in what order.

use std::cmp::Ordering;

/// Extensions of the images this worker decodes, lowercase.
const IMAGE_EXTENSIONS: [&str; 8] = ["jpg", "jpeg", "jpe", "jfif", "png", "gif", "webp", "bmp"];

/// What a file in the archive is to a reader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// An image that is a page.
    Page,
    /// The ComicRack metadata file.
    ComicInfo,
    /// Anything else, including what operating systems leave behind.
    Other,
}

/// Decides what a file is from its path in the archive. Hidden files and the folders macOS adds to
/// archives it creates are never pages, even when they end in `.jpg`.
pub fn role(path: &str) -> Role {
    let parts: Vec<&str> = path
        .split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .collect();
    let Some(name) = parts.last() else {
        return Role::Other;
    };
    if parts
        .iter()
        .any(|part| part.starts_with('.') || part.eq_ignore_ascii_case("__MACOSX"))
    {
        return Role::Other;
    }
    if name.eq_ignore_ascii_case("ComicInfo.xml") {
        return Role::ComicInfo;
    }
    let extension = name.rsplit_once('.').map(|(_, extension)| extension);
    match extension {
        Some(extension)
            if IMAGE_EXTENSIONS
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known)) =>
        {
            Role::Page
        }
        _ => Role::Other,
    }
}

/// Compares paths the way people number pages: `p2` before `p10`, ignoring case.
pub fn natural(a: &str, b: &str) -> Ordering {
    let mut left = Runs::new(a);
    let mut right = Runs::new(b);
    loop {
        match (left.next(), right.next()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let order = match (x, y) {
                    (Run::Number(x), Run::Number(y)) => {
                        let (x, y) = (x.trim_start_matches('0'), y.trim_start_matches('0'));
                        x.len().cmp(&y.len()).then_with(|| x.cmp(y))
                    }
                    (Run::Text(x), Run::Text(y)) => x
                        .chars()
                        .flat_map(char::to_lowercase)
                        .cmp(y.chars().flat_map(char::to_lowercase)),
                    (Run::Number(_), Run::Text(_)) => Ordering::Less,
                    (Run::Text(_), Run::Number(_)) => Ordering::Greater,
                };
                if order != Ordering::Equal {
                    return order;
                }
            }
        }
    }
}

enum Run<'a> {
    Number(&'a str),
    Text(&'a str),
}

struct Runs<'a> {
    rest: &'a str,
}

impl<'a> Runs<'a> {
    fn new(text: &'a str) -> Self {
        Self { rest: text }
    }
}

impl<'a> Iterator for Runs<'a> {
    type Item = Run<'a>;

    fn next(&mut self) -> Option<Run<'a>> {
        let first = self.rest.chars().next()?;
        let digits = first.is_ascii_digit();
        let end = self
            .rest
            .find(|c: char| c.is_ascii_digit() != digits)
            .unwrap_or(self.rest.len());
        let (run, rest) = self.rest.split_at(end);
        self.rest = rest;
        Some(if digits {
            Run::Number(run)
        } else {
            Run::Text(run)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_are_images_that_are_not_hidden() {
        assert_eq!(role("p1.jpg"), Role::Page);
        assert_eq!(role("Vol 1/Chapter 2/007.PNG"), Role::Page);
        assert_eq!(role("cover.webp"), Role::Page);
        assert_eq!(role("__MACOSX/._p1.jpg"), Role::Other);
        assert_eq!(role("chapter/.p1.jpg"), Role::Other);
        assert_eq!(role(".hidden/p1.jpg"), Role::Other);
        assert_eq!(role("Thumbs.db"), Role::Other);
        assert_eq!(role("notes.txt"), Role::Other);
        assert_eq!(role("extras/"), Role::Other);
        assert_eq!(role("comicinfo.XML"), Role::ComicInfo);
    }

    #[test]
    fn numbers_sort_as_numbers_and_letters_ignore_case() {
        let mut names = vec![
            "p10.png", "P2.jpg", "p1.jpg", "p02b.jpg", "a/p3.jpg", "p2a.jpg", "p011.jpg",
        ];
        names.sort_by(|a, b| natural(a, b));
        assert_eq!(
            names,
            vec![
                "a/p3.jpg", "p1.jpg", "P2.jpg", "p2a.jpg", "p02b.jpg", "p10.png", "p011.jpg"
            ]
        );
        assert_eq!(natural("x", "x"), Ordering::Equal);
        assert_eq!(
            natural("007", "7"),
            Ordering::Less,
            "ties fall back to the exact text"
        );
    }
}

//! The conversation between the host and the worker, over the worker's stdin and stdout.
//!
//! The worker speaks first, once it has locked itself down. From then on, each request is answered
//! by one reply, and either side closing its pipe ends the conversation. A normalized book is the one
//! reply with more than JSON: [`Reply::Normalized`] is followed by its files, raw, each with its
//! path and its length, which the host checks before keeping any of it.

use std::io::{self, Read, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::parse::Extracted;

/// Bumped when a message changes, since the host and the worker are built separately. Version 2
/// adds normalizing a flowing book.
pub const PROTOCOL_VERSION: u32 = 3;

/// No honest message comes close to this, and neither side allocates more for one.
pub const MAX_FRAME: u32 = 1 << 20;

/// A format this crate can read.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    /// PDF.
    Pdf,
    /// EPUB.
    Epub,
    /// Mobipocket: MOBI, PRC and AZW.
    Mobi,
    /// Kindle Format 8.
    Azw3,
    /// FictionBook 2.
    Fb2,
    /// Plain text.
    Txt,
    /// Markdown.
    Markdown,
}

impl Format {
    /// The format of a file, given its lowercase extension.
    pub fn from_extension(extension: &str) -> Option<Self> {
        match extension {
            "pdf" => Some(Self::Pdf),
            "epub" => Some(Self::Epub),
            "mobi" | "prc" | "azw" => Some(Self::Mobi),
            "azw3" => Some(Self::Azw3),
            "fb2" => Some(Self::Fb2),
            "txt" => Some(Self::Txt),
            "md" | "markdown" => Some(Self::Markdown),
            _ => None,
        }
    }

    /// The format as the normalizer knows it, for the formats that flow.
    pub fn flow(self) -> Option<musaeum_flow::package::SourceFormat> {
        use musaeum_flow::package::SourceFormat;
        match self {
            Self::Pdf => None,
            Self::Epub => Some(SourceFormat::Epub),
            Self::Mobi => Some(SourceFormat::Mobi),
            Self::Azw3 => Some(SourceFormat::Azw3),
            Self::Fb2 => Some(SourceFormat::Fb2),
            Self::Txt => Some(SourceFormat::Txt),
            Self::Markdown => Some(SourceFormat::Markdown),
        }
    }
}

/// What the host wants done with a file.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Task {
    /// Identifiers and basic metadata.
    #[default]
    Extract,
    /// The whole book, normalized: a flowing format only.
    Normalize,
    /// Only the cover of a flowing book, as the normalizer finds it, in a [`Reply::Normalized`]
    /// with one file, or none when the book has no cover.
    Cover,
}

/// What the host asks.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct Request {
    /// A read-only handle the host duplicated into the worker, which now owns it.
    pub handle: u64,
    /// What the host expects this file to be.
    pub format: Format,
    #[serde(default)]
    pub task: Task,
}

/// What the worker says.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum Reply {
    /// It is locked down and will not hold anything but what the host sends from here on.
    Ready {
        /// [`PROTOCOL_VERSION`].
        protocol: u32,
    },
    /// What was found in the file.
    Extracted(Extracted),
    /// The normalized book: this many files follow, this many bytes of content in all.
    Normalized {
        /// How many files follow.
        files: u32,
        /// Their contents' length together.
        bytes: u64,
    },
    /// The file could not be read as what the host said it was. This is an answer, not a failure:
    /// a damaged book is not a reason to restart anything.
    Rejected {
        /// For the log.
        reason: String,
    },
}

/// The longest path a normalized book may have, in bytes.
pub const MAX_PATH: usize = 128;

/// Whether a path in a normalized book is one the host will write: relative, at most three
/// levels, each a name of letters, digits, `.`, `_` and `-` that does not start with a dot.
pub fn is_safe_path(path: &str) -> bool {
    let segments: Vec<&str> = path.split('/').collect();
    path.len() <= MAX_PATH
        && segments.len() <= 3
        && segments.iter().all(|segment| {
            !segment.is_empty()
                && !segment.starts_with('.')
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        })
}

/// Writes the files of a normalized book after its [`Reply::Normalized`]: each as the length of its
/// path (4 bytes), the path, the length of its content (8 bytes), and the content.
pub fn write_files(writer: &mut impl Write, files: &[(String, Vec<u8>)]) -> io::Result<()> {
    for (path, data) in files {
        writer.write_all(&(path.len() as u32).to_le_bytes())?;
        writer.write_all(path.as_bytes())?;
        writer.write_all(&(data.len() as u64).to_le_bytes())?;
        writer.write_all(data)?;
    }
    writer.flush()
}

/// What the host accepts from a worker in one normalized book.
#[derive(Clone, Copy, Debug)]
pub struct FileLimits {
    pub max_files: u32,
    pub max_file: u64,
    pub max_total: u64,
}

impl Default for FileLimits {
    fn default() -> Self {
        Self {
            max_files: 100_000,
            max_file: 512 << 20,
            max_total: 1 << 30,
        }
    }
}

/// Reads `count` files written by [`write_files`], announced as `bytes` in all. Nothing is
/// allocated for a claim before it is checked: a path that is not safe, a file or a total over the
/// limits, or a count that does not match, ends the reading with an error.
pub fn read_files(
    reader: &mut impl Read,
    count: u32,
    bytes: u64,
    limits: FileLimits,
) -> io::Result<Vec<(String, Vec<u8>)>> {
    let invalid = |what: String| io::Error::new(io::ErrorKind::InvalidData, what);
    if count > limits.max_files || bytes > limits.max_total {
        return Err(invalid(format!("{count} files, {bytes} bytes")));
    }
    let mut files = Vec::with_capacity(count as usize);
    let mut total = 0u64;
    for _ in 0..count {
        let mut length = [0; 4];
        reader.read_exact(&mut length)?;
        let path_length = u32::from_le_bytes(length) as usize;
        if path_length > MAX_PATH {
            return Err(invalid(format!("a path of {path_length} bytes")));
        }
        let mut path = vec![0; path_length];
        reader.read_exact(&mut path)?;
        let path =
            String::from_utf8(path).map_err(|_| invalid("a path that is not UTF-8".to_owned()))?;
        if !is_safe_path(&path) {
            return Err(invalid(format!("the path {path:?}")));
        }
        let mut size = [0; 8];
        reader.read_exact(&mut size)?;
        let size = u64::from_le_bytes(size);
        total = total.saturating_add(size);
        if size > limits.max_file || total > bytes {
            return Err(invalid(format!("{path} claims {size} bytes")));
        }
        let mut data = vec![0; size as usize];
        reader.read_exact(&mut data)?;
        files.push((path, data));
    }
    if total != bytes {
        return Err(invalid(format!(
            "{total} bytes came, {bytes} were announced"
        )));
    }
    Ok(files)
}

/// Writes one message: its length as 4 bytes, then its JSON.
pub fn write_frame(writer: &mut impl Write, message: &impl Serialize) -> io::Result<()> {
    let body = serde_json::to_vec(message).map_err(io::Error::other)?;
    let length = u32::try_from(body.len())
        .ok()
        .filter(|&length| length <= MAX_FRAME)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "the message is too long"))?;
    writer.write_all(&length.to_le_bytes())?;
    writer.write_all(&body)?;
    writer.flush()
}

/// Reads one message, or `None` if the other side has closed its pipe. The length is checked before
/// anything is allocated, because the other side is not trusted to tell the truth about it.
pub fn read_frame<T: DeserializeOwned>(reader: &mut impl Read) -> io::Result<Option<T>> {
    let mut length = [0; 4];
    match reader.read_exact(&mut length) {
        Ok(()) => {}
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::UnexpectedEof | io::ErrorKind::BrokenPipe
            ) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error),
    }
    let length = u32::from_le_bytes(length);
    if length > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("a message claims to be {length} bytes"),
        ));
    }
    let mut body = vec![0; length as usize];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_survives_the_trip() {
        let mut pipe = Vec::new();
        let request = Request {
            handle: 0x1234,
            format: Format::Epub,
            task: Task::Normalize,
        };
        write_frame(&mut pipe, &request).unwrap();
        let read: Request = read_frame(&mut pipe.as_slice()).unwrap().unwrap();
        assert_eq!(read, request);
    }

    #[test]
    fn a_request_without_a_task_is_an_extraction() {
        let request: Request = serde_json::from_str(r#"{"handle":1,"format":"pdf"}"#).unwrap();
        assert_eq!(request.task, Task::Extract);
    }

    #[test]
    fn files_survive_the_trip_and_lies_about_them_do_not() {
        let files = vec![
            ("text/0000.xhtml".to_owned(), b"<html/>".to_vec()),
            ("musaeum.json".to_owned(), b"{}".to_vec()),
        ];
        let mut pipe = Vec::new();
        write_files(&mut pipe, &files).unwrap();
        let read = read_files(&mut pipe.as_slice(), 2, 9, FileLimits::default()).unwrap();
        assert_eq!(read, files);
        assert!(
            read_files(&mut pipe.as_slice(), 2, 8, FileLimits::default()).is_err(),
            "a wrong total"
        );
        assert!(
            read_files(&mut pipe.as_slice(), 3, 9, FileLimits::default()).is_err(),
            "a missing file"
        );
        for bad in [
            "../evil",
            "/abs",
            "a\\b",
            "C:/x",
            ".hidden",
            "a//b",
            "a/b/c/d",
            "text/évil",
            "",
        ] {
            let mut pipe = Vec::new();
            write_files(&mut pipe, &[(bad.to_owned(), b"x".to_vec())]).unwrap();
            assert!(
                read_files(&mut pipe.as_slice(), 1, 1, FileLimits::default()).is_err(),
                "{bad}"
            );
        }
        let mut huge = Vec::new();
        huge.extend_from_slice(&5u32.to_le_bytes());
        huge.extend_from_slice(b"a.txt");
        huge.extend_from_slice(&u64::MAX.to_le_bytes());
        assert!(
            read_files(&mut huge.as_slice(), 1, 1 << 20, FileLimits::default()).is_err(),
            "no allocation for a claim"
        );
    }

    #[test]
    fn a_closed_pipe_is_the_end_and_not_an_error() {
        let empty: &[u8] = &[];
        assert_eq!(read_frame::<Request>(&mut { empty }).unwrap(), None);
    }

    #[test]
    fn a_claim_of_an_enormous_message_is_refused_before_any_allocation() {
        let frame = u32::MAX.to_le_bytes();
        let error = read_frame::<Request>(&mut frame.as_slice()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn garbage_is_an_error() {
        let mut frame = 5u32.to_le_bytes().to_vec();
        frame.extend_from_slice(b"@@@@@");
        assert!(read_frame::<Request>(&mut frame.as_slice()).is_err());
    }
}

//! The conversation between the host and the OCR worker, over the worker's stdin and stdout.
//!
//! The worker speaks first, once it has locked itself down. Each request is answered by one reply.
//! Two requests carry bytes after their JSON: [`Request::Load`] the models, one after another, and
//! [`Request::Recognize`] the picture's pixels. Every length is checked against a limit before
//! anything is allocated for it, on both sides.

use std::io::{self, Read, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// Bumped when a message changes, since the host and the worker are built separately.
pub const PROTOCOL_VERSION: u32 = 1;

/// No honest message comes close to this.
pub const MAX_FRAME: u32 = 4 << 20;

/// The most bytes one model may have.
pub const MAX_MODEL: u64 = 64 << 20;

/// The most models one engine loads.
pub const MAX_MODELS: usize = 8;

/// The longest side of a picture, and the most pixels one may have.
pub const MAX_SIDE: u32 = 16_000;
pub const MAX_PIXELS: u64 = 64 << 20;

/// How Tesseract looks for text on the picture.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Finds blocks and lines itself.
    Auto,
    /// One block of text.
    Block,
    /// One line of text.
    Line,
    /// Whatever text there is, in no order.
    Sparse,
}

/// A model that follows a [`Request::Load`].
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ModelHeader {
    /// Tesseract's name for its language: `por`, `eng`.
    pub language: String,
    pub bytes: u64,
}

/// What the host asks.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum Request {
    /// These models follow, raw, in this order; then the worker makes an engine for `languages`
    /// (`por+eng`), replacing any it had.
    Load {
        models: Vec<ModelHeader>,
        languages: String,
    },
    /// `width × height` bytes of gray follow, rows packed, 0 black; read them in `mode` at `ppi`
    /// pixels per inch.
    Recognize {
        width: u32,
        height: u32,
        mode: Mode,
        ppi: u32,
    },
}

/// A word the worker read, in pixels of the picture it was given.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Word {
    pub text: String,
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    /// From 0 to 100.
    pub confidence: f32,
    pub block: u32,
    pub paragraph: u32,
    pub line: u32,
}

/// What the worker says.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum Reply {
    /// It is locked down and holds nothing but what the host sends from here on.
    Ready { protocol: u32 },
    /// The engine is ready.
    Loaded,
    /// The words of the picture, in reading order.
    Words(Vec<Word>),
    /// The request could not be done. An answer, not a failure of the worker.
    Failed { reason: String },
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

/// Reads one message, or `None` if the other side has closed its pipe. The length is checked
/// before anything is allocated.
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

/// Reads `count` bytes that a message announced, refusing more than `limit`.
pub fn read_bytes(reader: &mut impl Read, count: u64, limit: u64) -> io::Result<Vec<u8>> {
    if count > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{count} bytes announced, {limit} allowed"),
        ));
    }
    let mut data = vec![0; count as usize];
    reader.read_exact(&mut data)?;
    Ok(data)
}

/// Whether a picture of this size may be sent.
pub fn picture_fits(width: u32, height: u32) -> bool {
    width > 0
        && height > 0
        && width <= MAX_SIDE
        && height <= MAX_SIDE
        && u64::from(width) * u64::from(height) <= MAX_PIXELS
}

/// Whether a language name is one Tesseract could have a model for: a few lowercase letters,
/// digits or `_`.
pub fn is_language(name: &str) -> bool {
    (2..=32).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_and_its_bytes_go_and_come_back() {
        let mut wire = Vec::new();
        write_frame(
            &mut wire,
            &Request::Recognize {
                width: 3,
                height: 2,
                mode: Mode::Line,
                ppi: 300,
            },
        )
        .unwrap();
        wire.extend_from_slice(&[1, 2, 3, 4, 5, 6]);
        let mut reader = wire.as_slice();
        let request: Request = read_frame(&mut reader).unwrap().unwrap();
        let Request::Recognize { width, height, .. } = request else {
            panic!("{request:?}");
        };
        let pixels = read_bytes(&mut reader, u64::from(width * height), MAX_PIXELS).unwrap();
        assert_eq!(pixels, [1, 2, 3, 4, 5, 6]);
        assert!(read_frame::<Request>(&mut reader).unwrap().is_none());
    }

    #[test]
    fn lengths_are_checked_before_anything_is_kept() {
        let mut wire = (MAX_FRAME + 1).to_le_bytes().to_vec();
        wire.extend_from_slice(b"{}");
        assert!(read_frame::<Reply>(&mut wire.as_slice()).is_err());
        assert!(read_bytes(&mut [0u8; 4].as_slice(), 5, 4).is_err());
        assert!(!picture_fits(0, 10));
        assert!(!picture_fits(MAX_SIDE + 1, 10));
        assert!(picture_fits(2000, 3000));
        assert!(is_language("por"));
        assert!(!is_language("../x"));
    }
}

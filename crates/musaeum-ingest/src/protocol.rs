//! The conversation between the host and the worker, over the worker's stdin and stdout.
//!
//! The worker speaks first, once it has locked itself down. From then on, each request is answered
//! by one reply, and either side closing its pipe ends the conversation.

use std::io::{self, Read, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::parse::Extracted;

/// Bumped when a message changes, since the host and the worker are built separately.
pub const PROTOCOL_VERSION: u32 = 1;

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
}

impl Format {
    /// The format of a file, given its lowercase extension.
    pub fn from_extension(extension: &str) -> Option<Self> {
        match extension {
            "pdf" => Some(Self::Pdf),
            "epub" => Some(Self::Epub),
            _ => None,
        }
    }
}

/// What the host asks.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct Request {
    /// A read-only handle the host duplicated into the worker, which now owns it.
    pub handle: u64,
    /// What the host expects this file to be.
    pub format: Format,
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
    /// The file could not be read as what the host said it was. This is an answer, not a failure:
    /// a damaged book is not a reason to restart anything.
    Rejected {
        /// For the log.
        reason: String,
    },
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
        };
        write_frame(&mut pipe, &request).unwrap();
        let read: Request = read_frame(&mut pipe.as_slice()).unwrap().unwrap();
        assert_eq!(read, request);
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

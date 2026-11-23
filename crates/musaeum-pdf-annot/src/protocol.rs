//! The conversation between the host and the PDF annotations worker, over its stdin and stdout.
//!
//! The worker speaks first, once it has locked itself down. Each request is answered by one reply.
//! Every request names a PDF by a read-only handle the host duplicated into the worker, which then
//! owns it. [`Request::ReadXfdf`] is followed by the XFDF file's bytes, and [`Reply::Bytes`] by the
//! bytes it announces: an update to put after a PDF's own, or an XFDF file. Every length is checked
//! against a limit before anything is allocated for it, on both sides.

use std::collections::BTreeMap;
use std::io::{self, Read, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::model::Annot;
use crate::read::Reading;
use crate::write::{WriteOptions, WrittenAnnot};

/// Bumped when a message changes, since the host and the worker are built separately.
pub const PROTOCOL_VERSION: u32 = 1;

/// No honest message comes close to this.
pub const MAX_FRAME: u32 = 128 << 20;

/// The most bytes an update or an XFDF file may have.
pub const MAX_BYTES: u64 = 256 << 20;

/// Which reader reads a PDF's annotations.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub enum Reader {
    /// This crate's, from the file's objects, when it can read them and its pages are placed as
    /// PDFium places them; PDFium's otherwise, as for an encrypted file.
    #[default]
    Auto,
    /// PDFium's, page by page: what checks a written copy.
    Pdfium,
    /// This crate's alone: what an update is written from.
    Objects,
}

/// What the host asks.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum Request {
    /// Every page's frame and the PDF's annotations of the kinds Musaeum keeps, with places as
    /// fractions of their page's frame.
    Read { handle: u64, with: Reader },
    /// The bytes to put after the PDF's to add `annots`, whose places are fractions of their
    /// pages' frames.
    Export {
        handle: u64,
        annots: Vec<Annot>,
        options: WriteOptions,
    },
    /// `bytes` of XFDF follow, about the PDF: its annotations, placed on its pages.
    ReadXfdf { handle: u64, bytes: u64 },
    /// An XFDF file of `annots`, placed on the PDF's pages, naming it `href`.
    WriteXfdf {
        handle: u64,
        annots: Vec<Annot>,
        href: Option<String>,
    },
}

/// What a PDF, or an XFDF file about it, holds.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Found {
    pub reading: Reading,
    /// The PDF's `/ID`, as hex: the permanent one and the changing one.
    pub id: Option<(String, String)>,
    /// The `/ID` an XFDF file names, as hex, when it names one.
    pub xfdf_ids: Option<(String, String)>,
    pub encrypted: bool,
    /// Why no update can be written to the PDF, when none can.
    pub not_writable: Option<String>,
    /// How many annotations of each kind PDFium sees on each page, all kinds, by PDF name: what
    /// a written copy is checked against.
    #[serde(default)]
    pub counts: Vec<BTreeMap<String, u32>>,
}

/// What the worker says.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum Reply {
    /// It is locked down and holds nothing but what the host sends from here on.
    Ready {
        protocol: u32,
    },
    Found(Found),
    /// `length` bytes follow; for an update, what is in it, and how long the PDF it goes after
    /// was when the worker read it, which its offsets count from.
    Bytes {
        length: u64,
        written: Vec<WrittenAnnot>,
        #[serde(default)]
        base: u64,
    },
    /// The request could not be done. An answer, not a failure of the worker.
    Failed {
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

/// Bytes as hex, as identifiers travel.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02X}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_goes_and_comes_back_and_lengths_are_checked() {
        let mut wire = Vec::new();
        write_frame(
            &mut wire,
            &Request::ReadXfdf {
                handle: 7,
                bytes: 3,
            },
        )
        .unwrap();
        wire.extend_from_slice(b"abc");
        let mut reader = wire.as_slice();
        let request: Request = read_frame(&mut reader).unwrap().unwrap();
        assert_eq!(
            request,
            Request::ReadXfdf {
                handle: 7,
                bytes: 3
            }
        );
        assert_eq!(read_bytes(&mut reader, 3, 3).unwrap(), b"abc");
        assert!(read_frame::<Request>(&mut reader).unwrap().is_none());
        assert!(read_bytes(&mut [0u8; 8].as_slice(), 9, 8).is_err());
        let mut lying = (MAX_FRAME + 1).to_le_bytes().to_vec();
        lying.extend_from_slice(&[0; 16]);
        assert!(read_frame::<Request>(&mut lying.as_slice()).is_err());
    }
}

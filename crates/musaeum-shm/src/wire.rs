//! Control-plane messages exchanged over a render worker's stdio pipes.
//!
//! Every message is exactly [`MESSAGE_LEN`] bytes, little-endian. Fixed-size frames
//! mean there is no length field for a compromised worker to lie about, and decoding
//! cannot over-read or allocate.

use std::io::{self, Read, Write};

use thiserror::Error;

use crate::tile::{PageGeometry, TileKey};

/// Bumped on any change to this file or to the shared-memory layouts. Version 2 renders tiles at
/// any scale instead of powers of two, and adds the catalog's hints.
pub const PROTOCOL_VERSION: u32 = 2;
/// Length of every message.
pub const MESSAGE_LEN: usize = 48;

/// First message a worker receives. All handles are already valid in the worker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InitParams {
    /// The host's [`PROTOCOL_VERSION`].
    pub protocol: u32,
    /// Index of this worker's cancel cell.
    pub worker_index: u32,
    /// Whether this worker should report the document's page geometry.
    pub send_geometry: bool,
    /// Section handle of the pixel slab.
    pub slab_handle: u64,
    /// Length of the pixel slab in bytes.
    pub slab_len: u64,
    /// Read-only file handle of the document.
    pub doc_handle: u64,
}

/// One tile to render.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderJob {
    /// Unique, increasing job number.
    pub job_seq: u64,
    /// Slab slot to write into.
    pub slot: u32,
    /// The tile.
    pub tile: TileKey,
}

/// Messages from the host to a worker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostMessage {
    /// Sets up the worker. Always the first message.
    Init(InitParams),
    /// Renders one tile. A worker has at most one of these outstanding.
    Render(RenderJob),
    /// Exits cleanly.
    Shutdown,
    /// Nothing has been asked of the worker for a while: it should give memory back.
    Trim,
    /// Report the document's geometry after all, as `send_geometry` would have at startup. The
    /// host asks when the geometry it already had turns out not to fit the document.
    SendGeometry,
}

/// Why a worker cannot start.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
#[repr(u32)]
pub enum FatalCode {
    /// Host and worker were built from different protocol versions.
    #[error("protocol version mismatch between host and worker")]
    ProtocolMismatch = 1,
    /// The slab handle could not be mapped or validated.
    #[error("the pixel slab could not be attached")]
    SlabAttach = 2,
    /// The document handle could not be mapped.
    #[error("the document could not be mapped")]
    DocumentMap = 3,
    /// `pdfium.dll` is missing or incompatible.
    #[error("the PDFium library could not be loaded")]
    LibraryLoad = 4,
    /// PDFium rejected the document; the detail word holds `FPDF_GetLastError`.
    #[error("PDFium could not open the document")]
    DocumentOpen = 5,
    /// The first message was not `Init`.
    #[error("the worker did not receive an initialization message")]
    BadHandshake = 6,
    /// The worker could not lock itself down, so it refuses to read the document.
    #[error("the worker could not enter its sandbox")]
    Sandbox = 7,
}

impl FatalCode {
    fn from_u32(value: u32) -> Option<Self> {
        Some(match value {
            1 => Self::ProtocolMismatch,
            2 => Self::SlabAttach,
            3 => Self::DocumentMap,
            4 => Self::LibraryLoad,
            5 => Self::DocumentOpen,
            6 => Self::BadHandshake,
            7 => Self::Sandbox,
            _ => return None,
        })
    }
}

/// Why one tile could not be rendered. The worker stays usable afterwards.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Error)]
#[repr(u32)]
pub enum TileFailure {
    /// The slot does not exist in the slab.
    #[error("slot out of range")]
    InvalidSlot = 1,
    /// The tile does not exist on the page, or the page does not exist.
    #[error("tile out of bounds")]
    OutOfBounds = 2,
    /// `FPDF_LoadPage` failed.
    #[error("page failed to load")]
    PageLoad = 3,
    /// `FPDFBitmap_CreateEx` failed.
    #[error("bitmap could not be created")]
    BitmapCreate = 4,
    /// PDFium reported a render failure.
    #[error("render failed")]
    RenderFailed = 5,
}

impl TileFailure {
    fn from_u32(value: u32) -> Option<Self> {
        Some(match value {
            1 => Self::InvalidSlot,
            2 => Self::OutOfBounds,
            3 => Self::PageLoad,
            4 => Self::BitmapCreate,
            5 => Self::RenderFailed,
            _ => return None,
        })
    }
}

/// Messages from a worker to the host. Host code treats all of them as untrusted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WorkerMessage {
    /// The document is open and the worker is ready for jobs.
    Ready {
        /// The worker's [`PROTOCOL_VERSION`].
        protocol: u32,
        /// Pages in the document.
        page_count: u32,
        /// Microseconds spent mapping and opening the document.
        open_us: u64,
    },
    /// Size of one page, in page order.
    Geometry {
        /// Page index.
        page: u32,
        /// The page's size.
        size: PageGeometry,
    },
    /// What the document's catalog asks of a reader. Sent between the last `Geometry` and
    /// `GeometryEnd`, by the worker that sends the geometry.
    Hints {
        /// `/PageLayout`: 0 if absent, then 1 to 6 for `SinglePage`, `OneColumn`,
        /// `TwoColumnLeft`, `TwoColumnRight`, `TwoPageLeft` and `TwoPageRight`.
        page_layout: u8,
        /// `/ViewerPreferences /Direction`: 0 if absent, 1 for `L2R`, 2 for `R2L`.
        direction: u8,
    },
    /// All page sizes have been sent.
    GeometryEnd {
        /// Pages sent.
        page_count: u32,
        /// Microseconds spent reading all sizes, none of them rasterized.
        elapsed_us: u64,
    },
    /// The slot holds a complete tile.
    TileDone {
        /// Finished job.
        job_seq: u64,
        /// The job's slot.
        slot: u32,
        /// Microseconds spent on the whole job.
        render_us: u32,
        /// The portion of `render_us` spent loading the page.
        load_us: u32,
    },
    /// The job was abandoned; the slot holds garbage and may be reused.
    TileCancelled {
        /// Cancelled job.
        job_seq: u64,
        /// The job's slot.
        slot: u32,
        /// Microseconds spent before stopping.
        elapsed_us: u32,
    },
    /// The job failed; the slot holds garbage and may be reused.
    TileFailed {
        /// Failed job.
        job_seq: u64,
        /// The job's slot.
        slot: u32,
        /// What went wrong.
        failure: TileFailure,
    },
    /// The worker cannot continue and is about to exit.
    Fatal {
        /// What went wrong.
        code: FatalCode,
        /// Code-specific detail.
        detail: u32,
    },
}

/// A frame that could not be decoded, or a pipe that broke mid-frame.
#[derive(Debug, Error)]
pub enum WireError {
    /// The pipe failed.
    #[error("control pipe failed: {0}")]
    Io(#[from] io::Error),
    /// The frame's tag or an enumerated field has no defined meaning.
    #[error("malformed control message (tag {tag}, field value {value})")]
    Malformed {
        /// The frame's tag.
        tag: u32,
        /// The offending value.
        value: u32,
    },
}

const TAG_INIT: u32 = 1;
const TAG_RENDER: u32 = 2;
const TAG_SHUTDOWN: u32 = 3;
const TAG_TRIM: u32 = 4;
const TAG_SEND_GEOMETRY: u32 = 5;
const TAG_READY: u32 = 10;
const TAG_GEOMETRY: u32 = 11;
const TAG_GEOMETRY_END: u32 = 12;
const TAG_TILE_DONE: u32 = 13;
const TAG_TILE_CANCELLED: u32 = 14;
const TAG_TILE_FAILED: u32 = 15;
const TAG_FATAL: u32 = 16;
const TAG_HINTS: u32 = 17;

/// Most `/PageLayout` code a worker may send.
const MAX_PAGE_LAYOUT_CODE: u8 = 6;
/// Most `/Direction` code a worker may send.
const MAX_DIRECTION_CODE: u8 = 2;

struct Encoder {
    frame: [u8; MESSAGE_LEN],
    at: usize,
}

impl Encoder {
    fn new(tag: u32) -> Self {
        Self {
            frame: [0; MESSAGE_LEN],
            at: 0,
        }
        .u32(tag)
    }

    fn put<const N: usize>(mut self, bytes: [u8; N]) -> Self {
        self.frame[self.at..self.at + N].copy_from_slice(&bytes);
        self.at += N;
        self
    }

    fn u16(self, value: u16) -> Self {
        self.put(value.to_le_bytes())
    }

    fn u8(self, value: u8) -> Self {
        self.put(value.to_le_bytes())
    }

    fn u32(self, value: u32) -> Self {
        self.put(value.to_le_bytes())
    }

    fn u64(self, value: u64) -> Self {
        self.put(value.to_le_bytes())
    }

    fn f32(self, value: f32) -> Self {
        self.put(value.to_le_bytes())
    }
}

struct Decoder<'a> {
    frame: &'a [u8; MESSAGE_LEN],
    at: usize,
}

impl Decoder<'_> {
    fn take<const N: usize>(&mut self) -> [u8; N] {
        let mut bytes = [0; N];
        bytes.copy_from_slice(&self.frame[self.at..self.at + N]);
        self.at += N;
        bytes
    }

    fn u16(&mut self) -> u16 {
        u16::from_le_bytes(self.take())
    }

    fn u8(&mut self) -> u8 {
        u8::from_le_bytes(self.take())
    }

    fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.take())
    }

    fn u64(&mut self) -> u64 {
        u64::from_le_bytes(self.take())
    }

    fn f32(&mut self) -> f32 {
        f32::from_le_bytes(self.take())
    }
}

impl HostMessage {
    /// Encodes the message into one frame.
    pub fn encode(&self) -> [u8; MESSAGE_LEN] {
        match *self {
            Self::Init(params) => {
                Encoder::new(TAG_INIT)
                    .u32(params.protocol)
                    .u32(params.worker_index)
                    .u32(u32::from(params.send_geometry))
                    .u64(params.slab_handle)
                    .u64(params.slab_len)
                    .u64(params.doc_handle)
                    .frame
            }
            Self::Render(job) => {
                Encoder::new(TAG_RENDER)
                    .u32(job.slot)
                    .u64(job.job_seq)
                    .u32(job.tile.page)
                    .u16(job.tile.x)
                    .u16(job.tile.y)
                    .u32(job.tile.scale)
                    .frame
            }
            Self::Shutdown => Encoder::new(TAG_SHUTDOWN).frame,
            Self::Trim => Encoder::new(TAG_TRIM).frame,
            Self::SendGeometry => Encoder::new(TAG_SEND_GEOMETRY).frame,
        }
    }

    /// Decodes one frame.
    pub fn decode(frame: &[u8; MESSAGE_LEN]) -> Result<Self, WireError> {
        let mut d = Decoder { frame, at: 0 };
        match d.u32() {
            TAG_INIT => {
                let protocol = d.u32();
                let worker_index = d.u32();
                let send_geometry = d.u32() != 0;
                Ok(Self::Init(InitParams {
                    protocol,
                    worker_index,
                    send_geometry,
                    slab_handle: d.u64(),
                    slab_len: d.u64(),
                    doc_handle: d.u64(),
                }))
            }
            TAG_RENDER => {
                let slot = d.u32();
                let job_seq = d.u64();
                let page = d.u32();
                let x = d.u16();
                let y = d.u16();
                let scale = d.u32();
                Ok(Self::Render(RenderJob {
                    job_seq,
                    slot,
                    tile: TileKey { page, scale, x, y },
                }))
            }
            TAG_SHUTDOWN => Ok(Self::Shutdown),
            TAG_TRIM => Ok(Self::Trim),
            TAG_SEND_GEOMETRY => Ok(Self::SendGeometry),
            tag => Err(WireError::Malformed { tag, value: tag }),
        }
    }
}

impl WorkerMessage {
    /// Encodes the message into one frame.
    pub fn encode(&self) -> [u8; MESSAGE_LEN] {
        match *self {
            Self::Ready {
                protocol,
                page_count,
                open_us,
            } => {
                Encoder::new(TAG_READY)
                    .u32(protocol)
                    .u32(page_count)
                    .u64(open_us)
                    .frame
            }
            Self::Geometry { page, size } => {
                Encoder::new(TAG_GEOMETRY)
                    .u32(page)
                    .f32(size.width_pt)
                    .f32(size.height_pt)
                    .frame
            }
            Self::Hints {
                page_layout,
                direction,
            } => Encoder::new(TAG_HINTS).u8(page_layout).u8(direction).frame,
            Self::GeometryEnd {
                page_count,
                elapsed_us,
            } => {
                Encoder::new(TAG_GEOMETRY_END)
                    .u32(page_count)
                    .u64(elapsed_us)
                    .frame
            }
            Self::TileDone {
                job_seq,
                slot,
                render_us,
                load_us,
            } => {
                Encoder::new(TAG_TILE_DONE)
                    .u32(slot)
                    .u64(job_seq)
                    .u32(render_us)
                    .u32(load_us)
                    .frame
            }
            Self::TileCancelled {
                job_seq,
                slot,
                elapsed_us,
            } => {
                Encoder::new(TAG_TILE_CANCELLED)
                    .u32(slot)
                    .u64(job_seq)
                    .u32(elapsed_us)
                    .frame
            }
            Self::TileFailed {
                job_seq,
                slot,
                failure,
            } => {
                Encoder::new(TAG_TILE_FAILED)
                    .u32(slot)
                    .u64(job_seq)
                    .u32(failure as u32)
                    .frame
            }
            Self::Fatal { code, detail } => {
                Encoder::new(TAG_FATAL).u32(code as u32).u32(detail).frame
            }
        }
    }

    /// Decodes one frame. The result still has to be checked against the host's state.
    pub fn decode(frame: &[u8; MESSAGE_LEN]) -> Result<Self, WireError> {
        let mut d = Decoder { frame, at: 0 };
        let tag = d.u32();
        match tag {
            TAG_READY => Ok(Self::Ready {
                protocol: d.u32(),
                page_count: d.u32(),
                open_us: d.u64(),
            }),
            TAG_GEOMETRY => Ok(Self::Geometry {
                page: d.u32(),
                size: PageGeometry {
                    width_pt: d.f32(),
                    height_pt: d.f32(),
                },
            }),
            TAG_HINTS => {
                let page_layout = d.u8();
                let direction = d.u8();
                if page_layout > MAX_PAGE_LAYOUT_CODE {
                    return Err(WireError::Malformed {
                        tag,
                        value: u32::from(page_layout),
                    });
                }
                if direction > MAX_DIRECTION_CODE {
                    return Err(WireError::Malformed {
                        tag,
                        value: u32::from(direction),
                    });
                }
                Ok(Self::Hints {
                    page_layout,
                    direction,
                })
            }
            TAG_GEOMETRY_END => Ok(Self::GeometryEnd {
                page_count: d.u32(),
                elapsed_us: d.u64(),
            }),
            TAG_TILE_DONE => {
                let slot = d.u32();
                Ok(Self::TileDone {
                    slot,
                    job_seq: d.u64(),
                    render_us: d.u32(),
                    load_us: d.u32(),
                })
            }
            TAG_TILE_CANCELLED => {
                let slot = d.u32();
                Ok(Self::TileCancelled {
                    slot,
                    job_seq: d.u64(),
                    elapsed_us: d.u32(),
                })
            }
            TAG_TILE_FAILED => {
                let slot = d.u32();
                let job_seq = d.u64();
                let value = d.u32();
                let failure =
                    TileFailure::from_u32(value).ok_or(WireError::Malformed { tag, value })?;
                Ok(Self::TileFailed {
                    job_seq,
                    slot,
                    failure,
                })
            }
            TAG_FATAL => {
                let value = d.u32();
                let code = FatalCode::from_u32(value).ok_or(WireError::Malformed { tag, value })?;
                Ok(Self::Fatal {
                    code,
                    detail: d.u32(),
                })
            }
            _ => Err(WireError::Malformed { tag, value: tag }),
        }
    }
}

/// Reads one frame. `Ok(None)` means the pipe closed at a frame boundary or mid-frame;
/// either way the peer is gone and there is nothing to salvage.
pub fn read_frame(reader: &mut impl Read) -> Result<Option<[u8; MESSAGE_LEN]>, WireError> {
    let mut frame = [0; MESSAGE_LEN];
    match reader.read_exact(&mut frame) {
        Ok(()) => Ok(Some(frame)),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(None),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Writes one frame.
pub fn write_frame(writer: &mut impl Write, frame: &[u8; MESSAGE_LEN]) -> io::Result<()> {
    writer.write_all(frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile() -> TileKey {
        TileKey {
            page: 1_999,
            scale: 0x0001_5A3F,
            x: 7,
            y: 65_535,
        }
    }

    #[test]
    fn host_messages_round_trip() {
        let messages = [
            HostMessage::Init(InitParams {
                protocol: PROTOCOL_VERSION,
                worker_index: 15,
                send_geometry: true,
                slab_handle: 0x1234_5678_9ABC_DEF0,
                slab_len: u64::MAX,
                doc_handle: 42,
            }),
            HostMessage::Render(RenderJob {
                job_seq: u64::MAX - 1,
                slot: 63,
                tile: tile(),
            }),
            HostMessage::Shutdown,
            HostMessage::Trim,
            HostMessage::SendGeometry,
        ];
        for message in messages {
            assert_eq!(HostMessage::decode(&message.encode()).unwrap(), message);
        }
    }

    #[test]
    fn worker_messages_round_trip() {
        let messages = [
            WorkerMessage::Ready {
                protocol: PROTOCOL_VERSION,
                page_count: 2_000,
                open_us: 1_234,
            },
            WorkerMessage::Geometry {
                page: 3,
                size: PageGeometry {
                    width_pt: 841.89,
                    height_pt: 595.28,
                },
            },
            WorkerMessage::Hints {
                page_layout: 6,
                direction: 2,
            },
            WorkerMessage::GeometryEnd {
                page_count: 2_000,
                elapsed_us: 99,
            },
            WorkerMessage::TileDone {
                job_seq: 9,
                slot: 8,
                render_us: 7,
                load_us: 6,
            },
            WorkerMessage::TileCancelled {
                job_seq: 9,
                slot: 8,
                elapsed_us: 7,
            },
            WorkerMessage::TileFailed {
                job_seq: 9,
                slot: 8,
                failure: TileFailure::PageLoad,
            },
            WorkerMessage::Fatal {
                code: FatalCode::DocumentOpen,
                detail: 4,
            },
        ];
        for message in messages {
            assert_eq!(WorkerMessage::decode(&message.encode()).unwrap(), message);
        }
    }

    #[test]
    fn unknown_tags_and_enum_values_are_rejected() {
        let mut frame = [0; MESSAGE_LEN];
        frame[0] = 200;
        assert!(matches!(
            WorkerMessage::decode(&frame),
            Err(WireError::Malformed { tag: 200, .. })
        ));
        assert!(HostMessage::decode(&frame).is_err());

        let mut failed = WorkerMessage::TileFailed {
            job_seq: 1,
            slot: 1,
            failure: TileFailure::PageLoad,
        }
        .encode();
        // The failure code follows the tag (4), slot (4) and job number (8).
        failed[16] = 99;
        assert!(matches!(
            WorkerMessage::decode(&failed),
            Err(WireError::Malformed { value: 99, .. })
        ));

        for (at, value) in [(4, 7), (5, 3)] {
            let mut hints = WorkerMessage::Hints {
                page_layout: 0,
                direction: 0,
            }
            .encode();
            hints[at] = value;
            assert!(
                matches!(
                    WorkerMessage::decode(&hints),
                    Err(WireError::Malformed { value: v, .. }) if v == u32::from(value)
                ),
                "a code with no meaning must not reach the host"
            );
        }
    }

    #[test]
    fn closed_pipes_are_not_errors_but_broken_frames_end_the_stream() {
        let mut empty: &[u8] = &[];
        assert!(read_frame(&mut empty).unwrap().is_none());

        let mut partial: &[u8] = &[1, 2, 3];
        assert!(read_frame(&mut partial).unwrap().is_none());
    }
}

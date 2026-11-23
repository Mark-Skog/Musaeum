//! The host's side of the PDF annotations worker.
//!
//! The worker reads PDFs and XFDF files from anywhere, so what it says back is treated as written by
//! an attacker: counts are limited, numbers must be finite and near their page, texts are stripped
//! of control characters and cut to length, and a worker that crashes, hangs or talks out of turn is
//! killed. Each request duplicates a read-only handle into it: it opens nothing itself.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Write};
use std::os::windows::io::AsHandle;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use musaeum_sandbox::{Policy, SandboxedChild, SpawnConfig};
use musaeum_shm::region::duplicate_file_read_only;

use crate::model::{
    Annot, Frame, MAX_CONTENTS, MAX_NAME, MAX_QUADS, MAX_STROKE_POINTS, MAX_STROKES,
};
use crate::protocol::{
    Found, MAX_BYTES, PROTOCOL_VERSION, Reader, Reply, Request, read_bytes, read_frame, write_frame,
};
use crate::write::{WriteOptions, WrittenAnnot};
use crate::xfdf::MAX_XFDF_BYTES;

const WORKER_FILE_NAME: &str = "musaeum-pdf-annots.exe";
/// The most annotations one answer may carry, and pages.
const MAX_ANNOTATIONS: usize = 200_000;
const MAX_PAGES: usize = 1 << 20;

/// How to start the worker.
#[derive(Clone, Debug)]
pub struct AnnotWorkerConfig {
    /// Its executable. `None` looks next to this program.
    pub exe: Option<PathBuf>,
    pub policy: Policy,
    /// How long one request may take, after which the worker is assumed stuck.
    pub timeout: Duration,
    /// Cap on its committed memory.
    pub memory_limit: usize,
}

impl Default for AnnotWorkerConfig {
    fn default() -> Self {
        Self {
            exe: None,
            // PDFium in use needs win32k, as in the render workers.
            policy: Policy::Lockdown,
            timeout: Duration::from_secs(180),
            memory_limit: 3 << 30,
        }
    }
}

impl AnnotWorkerConfig {
    fn executable(&self) -> Option<PathBuf> {
        self.exe.clone().or_else(|| {
            std::env::current_exe()
                .ok()
                .map(|exe| exe.with_file_name(WORKER_FILE_NAME))
        })
    }
}

type Message = (Reply, Option<Vec<u8>>);

/// An update the worker wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exported {
    /// The bytes to put after the PDF's.
    pub tail: Vec<u8>,
    pub written: Vec<WrittenAnnot>,
    /// How long the PDF was when the worker read it: the update's offsets count from there.
    pub base: u64,
}

struct Worker {
    child: SandboxedChild,
    stdin: File,
    replies: Receiver<std::io::Result<Option<Message>>>,
}

/// Keeps one worker, replacing it whenever it stops being trustworthy.
pub struct AnnotClient {
    config: AnnotWorkerConfig,
    worker: Option<Worker>,
}

impl AnnotClient {
    pub fn new(config: AnnotWorkerConfig) -> Self {
        Self {
            config,
            worker: None,
        }
    }

    /// The process id of the current worker, if one is running.
    pub fn worker_id(&self) -> Option<u32> {
        self.worker.as_ref().map(|worker| worker.child.id())
    }

    /// What `with` reads in `pdf`.
    pub fn read(&mut self, pdf: &File, with: Reader) -> Result<Found, String> {
        self.found(pdf, |handle| Request::Read { handle, with }, None)
    }

    /// An XFDF file's annotations, placed on `pdf`'s pages.
    pub fn read_xfdf(&mut self, pdf: &File, xfdf: &[u8]) -> Result<Found, String> {
        if xfdf.len() > MAX_XFDF_BYTES {
            return Err("o arquivo XFDF é grande demais".to_owned());
        }
        let length = xfdf.len() as u64;
        self.found(
            pdf,
            |handle| Request::ReadXfdf {
                handle,
                bytes: length,
            },
            Some(xfdf),
        )
    }

    /// The bytes to put after `pdf`'s to add `annots`, what is in them, and how long the PDF was
    /// when the worker read it.
    pub fn export(
        &mut self,
        pdf: &File,
        annots: Vec<Annot>,
        options: WriteOptions,
    ) -> Result<Exported, String> {
        let count = annots.len();
        match self.ask(
            pdf,
            move |handle| Request::Export {
                handle,
                annots,
                options,
            },
            None,
        )? {
            (Reply::Bytes { written, base, .. }, Some(tail)) => {
                if written.len() != count {
                    return Err(self.discard(&format!(
                        "it wrote {} annotations of {count}",
                        written.len()
                    )));
                }
                Ok(Exported {
                    tail,
                    written,
                    base,
                })
            }
            (Reply::Failed { reason }, _) => Err(clean(&reason, 512)),
            (other, _) => Err(self.discard(&format!("it answered {other:?}"))),
        }
    }

    /// An XFDF file of `annots`, placed on `pdf`'s pages.
    pub fn write_xfdf(
        &mut self,
        pdf: &File,
        annots: Vec<Annot>,
        href: Option<String>,
    ) -> Result<Vec<u8>, String> {
        match self.ask(
            pdf,
            move |handle| Request::WriteXfdf {
                handle,
                annots,
                href,
            },
            None,
        )? {
            (Reply::Bytes { .. }, Some(bytes)) => Ok(bytes),
            (Reply::Failed { reason }, _) => Err(clean(&reason, 512)),
            (other, _) => Err(self.discard(&format!("it answered {other:?}"))),
        }
    }

    fn found(
        &mut self,
        pdf: &File,
        request: impl FnOnce(u64) -> Request,
        after: Option<&[u8]>,
    ) -> Result<Found, String> {
        match self.ask(pdf, request, after)? {
            (Reply::Found(found), None) => match checked(found) {
                Ok(found) => Ok(found),
                Err(reason) => Err(self.discard(&reason)),
            },
            (Reply::Failed { reason }, _) => Err(clean(&reason, 512)),
            (other, _) => Err(self.discard(&format!("it answered {other:?}"))),
        }
    }

    fn ask(
        &mut self,
        pdf: &File,
        request: impl FnOnce(u64) -> Request,
        after: Option<&[u8]>,
    ) -> Result<Message, String> {
        if self.worker.is_none() {
            self.worker = Some(self.start_worker()?);
        }
        let timeout = self.config.timeout;
        let worker = self.worker.as_mut().ok_or("no worker")?;
        // If the worker fails to read this message, it never takes ownership of this handle, and
        // the process's end takes the handle with it.
        let handle = match duplicate_file_read_only(pdf, worker.child.as_handle()) {
            Ok(handle) => handle,
            Err(error) => {
                return Err(format!(
                    "o arquivo não pôde ser entregue ao leitor: {error}"
                ));
            }
        };
        let sent = write_frame(&mut worker.stdin, &request(handle)).and_then(|()| match after {
            Some(bytes) => worker
                .stdin
                .write_all(bytes)
                .and_then(|()| worker.stdin.flush()),
            None => Ok(()),
        });
        if let Err(error) = sent {
            return Err(self.discard(&format!("it could not be sent the request: {error}")));
        }
        match next_reply(worker, timeout) {
            Ok(message) => Ok(message),
            Err(reason) => Err(self.discard(&reason)),
        }
    }

    fn discard(&mut self, reason: &str) -> String {
        // Dropping it kills the process, and with it anything a file may have done to it.
        self.worker = None;
        format!("o leitor de anotações foi encerrado: {reason}")
    }

    fn start_worker(&self) -> Result<Worker, String> {
        let exe = self
            .config
            .executable()
            .ok_or("this program cannot tell where it is")?;
        start_worker(&exe, &self.config).map_err(|error| format!("{}: {error}", exe.display()))
    }
}

fn start_worker(exe: &Path, config: &AnnotWorkerConfig) -> Result<Worker, String> {
    let mut child = musaeum_sandbox::spawn(SpawnConfig {
        exe,
        args: &[],
        policy: config.policy,
        memory_limit: Some(config.memory_limit),
    })
    .map_err(|error| error.to_string())?;
    let missing = || "a pipe was not created".to_owned();
    let stdin = child.stdin.take().ok_or_else(missing)?;
    let mut stdout = child.stdout.take().ok_or_else(missing)?;
    let mut stderr = child.stderr.take().ok_or_else(missing)?;
    // Reading on another thread is what makes a timeout possible. Killing the worker closes the
    // pipe, which ends the thread.
    let (sender, replies) = mpsc::channel();
    thread::Builder::new()
        .name("pdf-annots-reader".to_owned())
        .spawn(move || {
            loop {
                let message = match read_frame::<Reply>(&mut stdout) {
                    Ok(Some(Reply::Bytes {
                        length,
                        written,
                        base,
                    })) => read_bytes(&mut stdout, length, MAX_BYTES).map(|bytes| {
                        Some((
                            Reply::Bytes {
                                length,
                                written,
                                base,
                            },
                            Some(bytes),
                        ))
                    }),
                    Ok(Some(reply)) => Ok(Some((reply, None))),
                    Ok(None) => Ok(None),
                    Err(error) => Err(error),
                };
                let finished = !matches!(message, Ok(Some(_)));
                if sender.send(message).is_err() || finished {
                    return;
                }
            }
        })
        .map_err(|error| error.to_string())?;
    // The pipe has to keep being drained, or a worker writing a lot would block.
    thread::Builder::new()
        .name("pdf-annots-log".to_owned())
        .spawn(move || {
            let mut buffer = [0u8; 1024];
            while let Ok(read @ 1..) = stderr.read(&mut buffer) {
                eprint!("{}", String::from_utf8_lossy(&buffer[..read]));
            }
        })
        .map_err(|error| error.to_string())?;
    let mut worker = Worker {
        child,
        stdin,
        replies,
    };
    match next_reply(&mut worker, config.timeout)? {
        (Reply::Ready { protocol }, None) if protocol == PROTOCOL_VERSION => {}
        (other, _) => return Err(format!("it did not say it was ready, but {other:?}")),
    }
    if config.policy != Policy::Unrestricted {
        match worker.child.is_impersonating() {
            Ok(false) => {}
            Ok(true) => return Err("it still holds its startup token".to_owned()),
            Err(error) => return Err(format!("its token could not be checked: {error}")),
        }
    }
    Ok(worker)
}

fn next_reply(worker: &mut Worker, timeout: Duration) -> Result<Message, String> {
    match worker.replies.recv_timeout(timeout) {
        Ok(Ok(Some(reply))) => Ok(reply),
        Ok(Ok(None)) => Err("it closed its pipe".to_owned()),
        Ok(Err(error)) => Err(format!("it said something unreadable: {error}")),
        Err(RecvTimeoutError::Timeout) => Err(format!("it took more than {timeout:?}")),
        Err(RecvTimeoutError::Disconnected) => Err("it is gone".to_owned()),
    }
}

/// Text from the worker without control characters (a note keeps its line breaks and tabs), cut
/// to `limit` characters.
pub fn clean(text: &str, limit: usize) -> String {
    text.chars()
        .filter(|ch| !ch.is_control() || *ch == '\n' || *ch == '\t')
        .take(limit)
        .collect()
}

fn near_page(value: f64) -> bool {
    value.is_finite() && (-1.0..=2.0).contains(&value)
}

fn fine_frame(frame: &Frame) -> bool {
    [frame.left, frame.bottom, frame.right, frame.top]
        .iter()
        .all(|value| value.is_finite() && value.abs() < 1e7)
        && frame.rotation < 4
}

/// An annotation as the worker sent it, made safe to keep, or `None` when it cannot be.
fn checked_annot(mut annot: Annot, pages: usize) -> Option<Annot> {
    if annot.page as usize >= pages || annot.subtype.is_none() {
        return None;
    }
    if !annot.rect.iter().all(|&value| near_page(value))
        || annot.quads.len() > MAX_QUADS
        || !annot.quads.iter().flatten().all(|&value| near_page(value))
        || annot.ink.len() > MAX_STROKES
        || annot
            .ink
            .iter()
            .any(|stroke| stroke.len() > MAX_STROKE_POINTS * 2 || stroke.len() % 2 != 0)
        || !annot.ink.iter().flatten().all(|&value| near_page(value))
    {
        return None;
    }
    let channel = |color: Option<[f64; 3]>| {
        color.filter(|color| {
            color
                .iter()
                .all(|value| value.is_finite() && (0.0..=1.0).contains(value))
        })
    };
    annot.color = channel(annot.color);
    annot.interior = channel(annot.interior);
    annot.opacity = if annot.opacity.is_finite() {
        annot.opacity.clamp(0.0, 1.0)
    } else {
        1.0
    };
    annot.width = annot
        .width
        .filter(|width| width.is_finite() && (0.0..=1000.0).contains(width));
    annot.contents = clean(&annot.contents, MAX_CONTENTS);
    let short = |text: Option<String>| {
        text.map(|text| clean(&text, MAX_NAME).replace(['\n', '\t'], " "))
            .filter(|text| !text.trim().is_empty())
    };
    annot.name = short(annot.name);
    annot.author = short(annot.author);
    annot.subject = short(annot.subject);
    annot.icon = short(annot.icon);
    let moment = |value: Option<i64>| value.filter(|ms| (0..=253_402_300_799_000).contains(ms));
    annot.created = moment(annot.created);
    annot.modified = moment(annot.modified);
    Some(annot)
}

fn hex_id(id: Option<(String, String)>) -> Option<(String, String)> {
    let fine = |text: &str| text.len() <= 128 && text.bytes().all(|byte| byte.is_ascii_hexdigit());
    id.filter(|(first, second)| fine(first) && fine(second))
}

/// What the worker found, made safe to keep.
fn checked(found: Found) -> Result<Found, String> {
    let reading = found.reading;
    if reading.frames.len() > MAX_PAGES || !reading.frames.iter().all(fine_frame) {
        return Err("it sent pages that cannot be".to_owned());
    }
    if reading.annots.len() > MAX_ANNOTATIONS {
        return Err(format!("it sent {} annotations", reading.annots.len()));
    }
    let pages = reading.frames.len();
    let mut skipped: BTreeMap<String, u32> = reading
        .skipped
        .into_iter()
        .take(64)
        .map(|(kind, count)| (clean(&kind, 32), count))
        .collect();
    let mut annots = Vec::with_capacity(reading.annots.len());
    for annot in reading.annots {
        match checked_annot(annot, pages) {
            Some(annot) => annots.push(annot),
            None => *skipped.entry("(broken)".to_owned()).or_default() += 1,
        }
    }
    let counts = found
        .counts
        .into_iter()
        .take(MAX_PAGES)
        .map(|page| {
            page.into_iter()
                .take(64)
                .map(|(kind, count)| (clean(&kind, 32), count))
                .collect()
        })
        .collect();
    Ok(Found {
        reading: crate::read::Reading {
            frames: reading.frames,
            annots,
            skipped,
        },
        id: hex_id(found.id),
        xfdf_ids: hex_id(found.xfdf_ids),
        encrypted: found.encrypted,
        not_writable: found.not_writable.map(|reason| clean(&reason, 512)),
        counts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Subtype;
    use crate::read::Reading;

    #[test]
    fn what_a_worker_says_is_reduced_to_what_can_be() {
        let frame = Frame::of(None, None, 0);
        let fine = Annot {
            page: 0,
            subtype: Some(Subtype::Highlight),
            rect: [0.1, 0.1, 0.2, 0.2],
            quads: vec![[0.1, 0.1, 0.2, 0.1, 0.2, 0.2, 0.1, 0.2]],
            color: Some([2.0, 0.0, 0.0]),
            opacity: f64::NAN,
            contents: "nota\u{0}\u{7}\ncom linha".into(),
            name: Some("x\ny".into()),
            created: Some(-5),
            ..Annot::default()
        };
        let mut far = fine.clone();
        far.quads[0][0] = 1e9;
        let mut other_page = fine.clone();
        other_page.page = 3;
        let found = checked(Found {
            reading: Reading {
                frames: vec![frame],
                annots: vec![fine, far, other_page],
                skipped: BTreeMap::new(),
            },
            id: Some(("ZZ".into(), "00".into())),
            ..Found::default()
        })
        .unwrap();
        assert_eq!(found.reading.annots.len(), 1);
        let annot = &found.reading.annots[0];
        assert_eq!(annot.color, None);
        assert_eq!(annot.opacity, 1.0);
        assert_eq!(annot.contents, "nota\ncom linha");
        assert_eq!(annot.name.as_deref(), Some("x y"));
        assert_eq!(annot.created, None);
        assert_eq!(found.reading.skipped.get("(broken)"), Some(&2));
        assert_eq!(found.id, None);
    }

    #[test]
    fn with_no_executable_nothing_is_asked() {
        let mut client = AnnotClient::new(AnnotWorkerConfig {
            exe: Some(PathBuf::from("C:\\there\\is\\no\\such\\worker.exe")),
            ..AnnotWorkerConfig::default()
        });
        let file = File::open(std::env::current_exe().unwrap()).unwrap();
        assert!(client.read(&file, Reader::Auto).is_err());
    }
}

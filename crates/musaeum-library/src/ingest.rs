//! The host's side of the isolated worker.
//!
//! Everything that comes back is treated as written by an attacker, because a book may have taken
//! over the worker. It can lie about a title: nothing more, since what it says is length-limited,
//! stripped of control characters and only ever stored as data. A worker that crashes, hangs or
//! talks nonsense is killed, and the next book gets a new one.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use musaeum_flow::NORMALIZER_VERSION;
use musaeum_flow::package::{MANIFEST_PATH, Manifest};
use musaeum_ingest::parse::Extracted;
use musaeum_ingest::protocol::{
    FileLimits, Format, PROTOCOL_VERSION, Reply, Request, Task, read_files, read_frame, write_frame,
};
use musaeum_sandbox::{Policy, SandboxedChild, SpawnConfig};
use musaeum_shm::region::duplicate_file_read_only;

const WORKER_FILE_NAME: &str = "musaeum-ingest.exe";
const MAX_TITLE_CHARS: usize = 2048;
const MAX_IDENTIFIER_CHARS: usize = 512;
const MAX_CREATOR_CHARS: usize = 512;
const MAX_CREATORS: usize = 64;
const MAX_LANGUAGE_CHARS: usize = 64;

/// How to start the worker.
#[derive(Clone, Debug)]
pub struct IngestConfig {
    /// Its executable. `None` looks next to this program.
    pub exe: Option<PathBuf>,
    /// How much it may do.
    pub policy: Policy,
    /// How long one file may take, after which the worker is assumed to be stuck on it.
    pub timeout: Duration,
    /// Cap on its committed memory.
    pub memory_limit: usize,
}

impl Default for IngestConfig {
    fn default() -> Self {
        Self {
            exe: None,
            policy: Policy::LockdownParser,
            timeout: Duration::from_secs(10),
            memory_limit: 128 * 1024 * 1024,
        }
    }
}

impl IngestConfig {
    /// For normalizing books: the whole book and its normalized form are in memory at once, and a
    /// large one takes seconds.
    pub fn for_normalizing() -> Self {
        Self {
            timeout: Duration::from_secs(180),
            memory_limit: 3 << 30,
            ..Self::default()
        }
    }

    fn executable(&self) -> Option<PathBuf> {
        self.exe.clone().or_else(|| {
            std::env::current_exe()
                .ok()
                .map(|exe| exe.with_file_name(WORKER_FILE_NAME))
        })
    }
}

/// A normalized book as the worker sent it, checked: every path safe, the manifest readable and
/// of this normalizer's version, and every file it names present.
#[derive(Clone, Debug)]
pub struct NormalizedFiles {
    pub manifest: Manifest,
    pub files: Vec<(String, Vec<u8>)>,
}

/// What came of asking for a normalized book.
#[derive(Clone, Debug)]
pub enum NormalizeOutcome {
    Normalized(Box<NormalizedFiles>),
    /// The worker says the book cannot be normalized, or it died on the way.
    Nothing(String),
    /// No worker could be started.
    Unavailable(String),
}

/// What came of asking for a book's cover.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CoverOutcome {
    /// The image, with what its first bytes say it is.
    Cover { kind: ImageKind, bytes: Vec<u8> },
    /// The book names no cover, or names one that is not a picture this program draws.
    NoCover,
    /// The worker says the book cannot be read, or it died on the way.
    Nothing(String),
    /// No worker could be started.
    Unavailable(String),
}

/// The pictures a cover may be, told by their first bytes and never by a name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageKind {
    Jpeg,
    Png,
    Gif,
    Webp,
}

impl ImageKind {
    /// What `bytes` are, if they are one of these.
    pub fn sniff(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            Some(Self::Jpeg)
        } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(Self::Png)
        } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
            Some(Self::Gif)
        } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
            Some(Self::Webp)
        } else {
            None
        }
    }

    /// The usual extension of a file of this kind.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::Gif => "gif",
            Self::Webp => "webp",
        }
    }
}

/// The largest cover kept.
const MAX_COVER: usize = 32 << 20;

/// The largest manifest the host parses.
const MAX_MANIFEST: usize = 64 << 20;

/// Checks what a worker sent as a normalized book. A worker a book took over could send anything,
/// so nothing is assumed: paths were checked as they arrived, and here the manifest has to agree
/// with the files.
pub fn check_normalized(files: Vec<(String, Vec<u8>)>) -> Result<NormalizedFiles, String> {
    let mut seen = std::collections::HashSet::new();
    for (path, _) in &files {
        if !seen.insert(path.to_ascii_lowercase()) {
            return Err(format!("{path} comes twice"));
        }
    }
    let manifest_bytes = files
        .iter()
        .find(|(path, _)| path == MANIFEST_PATH)
        .map(|(_, data)| data)
        .ok_or("there is no manifest")?;
    if manifest_bytes.len() > MAX_MANIFEST {
        return Err("the manifest is too large".to_owned());
    }
    let manifest: Manifest = serde_json::from_slice(manifest_bytes)
        .map_err(|error| format!("the manifest makes no sense: {error}"))?;
    if manifest.normalizer != NORMALIZER_VERSION {
        return Err(format!(
            "normalizer version {}, not {NORMALIZER_VERSION}",
            manifest.normalizer
        ));
    }
    if manifest.documents.is_empty() {
        return Err("no documents".to_owned());
    }
    let present: std::collections::HashSet<&str> =
        files.iter().map(|(path, _)| path.as_str()).collect();
    for document in &manifest.documents {
        if document.parts.is_empty() {
            return Err(format!("{} has no parts", document.href));
        }
        for part in &document.parts {
            if !present.contains(part.href.as_str()) {
                return Err(format!("{} is missing", part.href));
            }
            if part.start.len() > 1024 || part.start.iter().any(|step| step % 2 != 0 || *step == 0)
            {
                return Err(format!("{} starts at an impossible place", part.href));
            }
        }
    }
    for resource in &manifest.resources {
        if !present.contains(resource.href.as_str()) {
            return Err(format!("{} is missing", resource.href));
        }
    }
    // What the worker made has to be what the normalizer makes: well-formed and inert. A worker a
    // book took over could send a script; it would not get past this.
    for (path, data) in &files {
        if !(path.ends_with(".xhtml") || path.ends_with(".svg")) {
            continue;
        }
        let text = std::str::from_utf8(data).map_err(|_| format!("{path} is not UTF-8"))?;
        let dom = musaeum_flow::xml::parse(text, &musaeum_flow::xml::Limits::default())
            .map_err(|error| format!("{path} is not well-formed: {error}"))?;
        if let Some(danger) = musaeum_flow::sanitize::find_danger(&dom) {
            return Err(format!("{path} still has {danger}"));
        }
    }
    Ok(NormalizedFiles { manifest, files })
}

/// What came of asking about a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum IngestOutcome {
    /// What was found.
    Extracted(Extracted),
    /// The worker says the file is not what it should be, or it died on the way to saying
    /// anything. Asking again about the same bytes would end the same way.
    Nothing(String),
    /// No worker could be started, which says nothing about the file, so this can be tried later.
    Unavailable(String),
}

/// A reply, with the files that came after it if it was a normalized book.
type Message = (Reply, Option<Vec<(String, Vec<u8>)>>);

struct Worker {
    child: SandboxedChild,
    stdin: File,
    replies: Receiver<std::io::Result<Option<Message>>>,
}

/// Keeps one worker, replacing it whenever it stops being trustworthy.
pub struct IngestClient {
    config: IngestConfig,
    worker: Option<Worker>,
    restarts: u64,
    // Set once a worker could not be started. Each attempt can take as long as the timeout, so
    // with thousands of books waiting, trying again for each one would look like a hang.
    cannot_start: Option<String>,
}

impl IngestClient {
    /// Normalizes a flowing book, passed as a handle and never as a path.
    pub fn normalize(&mut self, file: &File, format: Format) -> NormalizeOutcome {
        if let Some(reason) = &self.cannot_start {
            return NormalizeOutcome::Unavailable(reason.clone());
        }
        if self.worker.is_none() {
            match self.start_worker() {
                Ok(worker) => self.worker = Some(worker),
                Err(reason) => {
                    self.cannot_start = Some(reason.clone());
                    return NormalizeOutcome::Unavailable(reason);
                }
            }
        }
        let timeout = self.config.timeout;
        let Some(worker) = self.worker.as_mut() else {
            return NormalizeOutcome::Unavailable("no worker".to_owned());
        };
        match ask(worker, file, format, Task::Normalize, timeout) {
            Ok((Reply::Normalized { .. }, Some(files))) => match check_normalized(files) {
                Ok(normalized) => NormalizeOutcome::Normalized(Box::new(normalized)),
                Err(reason) => {
                    // A worker that sends a book that does not hold together is not trusted again.
                    self.discard(&reason);
                    NormalizeOutcome::Nothing(format!("what the worker sent was refused: {reason}"))
                }
            },
            Ok((Reply::Rejected { reason }, _)) => {
                NormalizeOutcome::Nothing(clean(&reason, MAX_TITLE_CHARS).unwrap_or_default())
            }
            Ok(_) => match self.discard("it answered out of turn") {
                IngestOutcome::Nothing(reason) => NormalizeOutcome::Nothing(reason),
                _ => NormalizeOutcome::Nothing("the worker was replaced".to_owned()),
            },
            Err(reason) => match self.discard(&reason) {
                IngestOutcome::Nothing(reason) => NormalizeOutcome::Nothing(reason),
                _ => NormalizeOutcome::Nothing("the worker was replaced".to_owned()),
            },
        }
    }

    /// The cover of a flowing book, passed as a handle and never as a path. The worker sends the
    /// bytes it found; whether they are a picture is decided here, by their first bytes.
    pub fn cover(&mut self, file: &File, format: Format) -> CoverOutcome {
        if let Some(reason) = &self.cannot_start {
            return CoverOutcome::Unavailable(reason.clone());
        }
        if self.worker.is_none() {
            match self.start_worker() {
                Ok(worker) => self.worker = Some(worker),
                Err(reason) => {
                    self.cannot_start = Some(reason.clone());
                    return CoverOutcome::Unavailable(reason);
                }
            }
        }
        let timeout = self.config.timeout;
        let Some(worker) = self.worker.as_mut() else {
            return CoverOutcome::Unavailable("no worker".to_owned());
        };
        match ask(worker, file, format, Task::Cover, timeout) {
            Ok((Reply::Normalized { .. }, Some(mut files))) => {
                if files.len() > 1 {
                    self.discard("it sent more than one cover");
                    return CoverOutcome::Nothing("the worker was replaced".to_owned());
                }
                match files.pop() {
                    None => CoverOutcome::NoCover,
                    Some((_, bytes)) if bytes.len() > MAX_COVER => CoverOutcome::NoCover,
                    Some((_, bytes)) => match ImageKind::sniff(&bytes) {
                        Some(kind) => CoverOutcome::Cover { kind, bytes },
                        None => CoverOutcome::NoCover,
                    },
                }
            }
            Ok((Reply::Rejected { reason }, _)) => {
                CoverOutcome::Nothing(clean(&reason, MAX_TITLE_CHARS).unwrap_or_default())
            }
            Ok(_) => match self.discard("it answered out of turn") {
                IngestOutcome::Nothing(reason) => CoverOutcome::Nothing(reason),
                _ => CoverOutcome::Nothing("the worker was replaced".to_owned()),
            },
            Err(reason) => match self.discard(&reason) {
                IngestOutcome::Nothing(reason) => CoverOutcome::Nothing(reason),
                _ => CoverOutcome::Nothing("the worker was replaced".to_owned()),
            },
        }
    }

    /// The process id of the current worker, if one is running.
    pub fn current_worker(&self) -> Option<u32> {
        self.worker.as_ref().map(|worker| worker.child.id())
    }

    pub fn new(config: IngestConfig) -> Self {
        Self {
            config,
            worker: None,
            restarts: 0,
            cannot_start: None,
        }
    }

    /// How many workers had to be thrown away.
    pub fn restarts(&self) -> u64 {
        self.restarts
    }

    /// The process id of the current worker, if one is running.
    #[cfg(test)]
    pub fn worker_id(&self) -> Option<u32> {
        self.worker.as_ref().map(|worker| worker.child.id())
    }

    /// Asks what is in `file`, passing it as a handle and never as a path.
    pub(crate) fn extract(&mut self, file: &File, format: Format) -> IngestOutcome {
        if let Some(reason) = &self.cannot_start {
            return IngestOutcome::Unavailable(reason.clone());
        }
        if self.worker.is_none() {
            match self.start_worker() {
                Ok(worker) => self.worker = Some(worker),
                Err(reason) => {
                    self.cannot_start = Some(reason.clone());
                    return IngestOutcome::Unavailable(reason);
                }
            }
        }
        let timeout = self.config.timeout;
        let Some(worker) = self.worker.as_mut() else {
            return IngestOutcome::Unavailable("no worker".to_owned());
        };
        match ask(worker, file, format, Task::Extract, timeout) {
            Ok((Reply::Extracted(extracted), _)) => IngestOutcome::Extracted(sanitized(extracted)),
            Ok((Reply::Rejected { reason }, _)) => {
                IngestOutcome::Nothing(clean(&reason, MAX_TITLE_CHARS).unwrap_or_default())
            }
            Ok(_) => self.discard("it answered out of turn"),
            Err(reason) => self.discard(&reason),
        }
    }

    fn discard(&mut self, reason: &str) -> IngestOutcome {
        // Dropping it kills the process, and with it anything a book may have done to it.
        self.worker = None;
        self.restarts += 1;
        IngestOutcome::Nothing(format!("the worker was replaced: {reason}"))
    }

    fn start_worker(&self) -> Result<Worker, String> {
        let exe = self
            .config
            .executable()
            .ok_or("this program cannot tell where it is")?;
        start_worker(&exe, &self.config).map_err(|error| format!("{}: {error}", exe.display()))
    }
}

fn start_worker(exe: &Path, config: &IngestConfig) -> Result<Worker, String> {
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

    // Reading on another thread is what makes a timeout possible, since nothing else can interrupt
    // a read from a pipe. Killing the worker closes the pipe, which ends the thread.
    let (sender, replies) = mpsc::channel();
    thread::Builder::new()
        .name("ingest-reader".to_owned())
        .spawn(move || {
            loop {
                let message = match read_frame::<Reply>(&mut stdout) {
                    Ok(Some(Reply::Normalized { files, bytes })) => {
                        read_files(&mut stdout, files, bytes, FileLimits::default())
                            .map(|list| Some((Reply::Normalized { files, bytes }, Some(list))))
                    }
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
    // The pipe has to keep being drained, or a worker writing a lot would block forever.
    thread::Builder::new()
        .name("ingest-log".to_owned())
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

fn ask(
    worker: &mut Worker,
    file: &File,
    format: Format,
    task: Task,
    timeout: Duration,
) -> Result<Message, String> {
    use std::os::windows::io::AsHandle;

    // If the worker fails to read this message, it never takes ownership of this handle, and the
    // process's end takes the handle with it.
    let handle = duplicate_file_read_only(file, worker.child.as_handle())
        .map_err(|error| format!("sharing the file: {error}"))?;
    write_frame(
        &mut worker.stdin,
        &Request {
            handle,
            format,
            task,
        },
    )
    .map_err(|error| format!("sending the request: {error}"))?;
    next_reply(worker, timeout)
}

fn next_reply(worker: &mut Worker, timeout: Duration) -> Result<Message, String> {
    match worker.replies.recv_timeout(timeout) {
        Ok(Ok(Some(reply))) => Ok(reply),
        Ok(Ok(None)) => Err("it closed its pipe".to_owned()),
        Ok(Err(error)) => Err(format!("what it sent makes no sense: {error}")),
        Err(RecvTimeoutError::Timeout) => Err(format!("no answer in {timeout:?}")),
        Err(RecvTimeoutError::Disconnected) => Err("it is gone".to_owned()),
    }
}

/// Whitespace folded, control characters dropped and length limited, or `None` if nothing is left.
fn clean(text: &str, max_chars: usize) -> Option<String> {
    let visible: String = text
        .chars()
        .filter(|character| character.is_whitespace() || !character.is_control())
        .collect();
    let cleaned: String = visible
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max_chars)
        .collect();
    (!cleaned.is_empty()).then_some(cleaned)
}

/// What the worker said, reduced to what such a field can be.
fn sanitized(extracted: Extracted) -> Extracted {
    let hexadecimal = |text: &String| {
        text.len() <= 128 && !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_hexdigit())
    };
    let isbn = |text: &String| {
        matches!(text.len(), 10 | 13)
            && text
                .bytes()
                .all(|byte| byte.is_ascii_digit() || byte == b'X')
    };
    Extracted {
        pdf_id0: extracted.pdf_id0.filter(hexadecimal),
        dc_identifier: extracted
            .dc_identifier
            .and_then(|text| clean(&text, MAX_IDENTIFIER_CHARS)),
        isbn: extracted.isbn.filter(isbn),
        title: extracted
            .title
            .and_then(|text| clean(&text, MAX_TITLE_CHARS)),
        creators: extracted
            .creators
            .iter()
            .filter_map(|text| clean(text, MAX_CREATOR_CHARS))
            .take(MAX_CREATORS)
            .collect(),
        language: extracted
            .language
            .and_then(|text| clean(&text, MAX_LANGUAGE_CHARS)),
        year: extracted.year.filter(|year| (1000..=2999).contains(year)),
        // Enumerations, so deserializing them already refused anything else.
        page_layout: extracted.page_layout,
        direction: extracted.direction,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_a_worker_says_is_reduced_to_what_a_field_can_be() {
        let lying = Extracted {
            pdf_id0: Some("not hexadecimal".to_owned()),
            dc_identifier: Some("x".repeat(100_000)),
            isbn: Some("<script>".to_owned()),
            title: Some("  Um\u{0007}\ttítulo\n ".to_owned()),
            creators: vec!["  ".to_owned(); 3]
                .into_iter()
                .chain(vec!["Autor".to_owned(); 1000])
                .collect(),
            language: Some("x".repeat(1000)),
            year: Some(-5),
            page_layout: None,
            direction: None,
        };
        let cleaned = sanitized(lying);
        assert_eq!(cleaned.pdf_id0, None);
        assert_eq!(
            cleaned.dc_identifier.unwrap().chars().count(),
            MAX_IDENTIFIER_CHARS
        );
        assert_eq!(cleaned.isbn, None);
        assert_eq!(cleaned.title.as_deref(), Some("Um título"));
        assert_eq!(cleaned.creators.len(), MAX_CREATORS);
        assert_eq!(
            cleaned.language.unwrap().chars().count(),
            MAX_LANGUAGE_CHARS
        );
        assert_eq!(cleaned.year, None);
    }

    #[test]
    fn what_a_worker_says_honestly_stays() {
        let honest = Extracted {
            pdf_id0: Some("00ab".to_owned()),
            dc_identifier: Some("urn:uuid:1234".to_owned()),
            isbn: Some("9780306406157".to_owned()),
            title: Some("Dom Casmurro".to_owned()),
            creators: vec!["Machado de Assis".to_owned()],
            language: Some("pt-BR".to_owned()),
            year: Some(1899),
            page_layout: Some(musaeum_ingest::parse::pdf_catalog::PageLayout::TwoPageRight),
            direction: Some(musaeum_ingest::parse::pdf_catalog::Direction::R2L),
        };
        assert_eq!(sanitized(honest.clone()), honest);
    }

    #[test]
    fn with_no_executable_to_start_the_answer_is_try_again_later() {
        let mut client = IngestClient::new(IngestConfig {
            exe: Some(PathBuf::from(r"C:\does\not\exist.exe")),
            ..IngestConfig::default()
        });
        let file = File::open(std::env::current_exe().unwrap()).unwrap();
        assert!(matches!(
            client.extract(&file, Format::Pdf),
            IngestOutcome::Unavailable(_)
        ));
        assert_eq!(client.worker_id(), None);
    }
}

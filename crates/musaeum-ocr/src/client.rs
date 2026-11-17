//! The host's side of the OCR worker.
//!
//! The worker reads pixels that came from a book, so what it says back is treated as written by an
//! attacker: words are length-limited and stripped of control characters, boxes are clamped to the
//! picture, and a worker that crashes, hangs or talks out of turn is killed and replaced. The models
//! are read here, from the program's own folder, and sent to the worker as bytes: it opens nothing.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use musaeum_sandbox::{Policy, SandboxedChild, SpawnConfig};

use crate::protocol::{
    MAX_MODEL, MAX_MODELS, Mode, ModelHeader, PROTOCOL_VERSION, Reply, Request, Word, is_language,
    picture_fits, read_frame, write_frame,
};
use crate::strip::Gray;

const WORKER_FILE_NAME: &str = "musaeum-ocr.exe";
const MODELS_FOLDER: &str = "tessdata";
/// The most words one picture may give, and the longest a word may be.
const MAX_WORDS: usize = 20_000;
const MAX_WORD_CHARS: usize = 256;

/// How to start the worker.
#[derive(Clone, Debug)]
pub struct OcrConfig {
    /// Its executable. `None` looks next to this program.
    pub exe: Option<PathBuf>,
    /// The folder of `.traineddata` files. `None` is `tessdata` next to the worker.
    pub models: Option<PathBuf>,
    pub policy: Policy,
    /// How long one picture may take, after which the worker is assumed stuck.
    pub timeout: Duration,
    /// Cap on its committed memory.
    pub memory_limit: usize,
}

impl Default for OcrConfig {
    fn default() -> Self {
        Self {
            exe: None,
            models: None,
            policy: Policy::LockdownParser,
            timeout: Duration::from_secs(30),
            memory_limit: 768 << 20,
        }
    }
}

impl OcrConfig {
    fn executable(&self) -> Option<PathBuf> {
        self.exe.clone().or_else(|| {
            std::env::current_exe()
                .ok()
                .map(|exe| exe.with_file_name(WORKER_FILE_NAME))
        })
    }

    fn models_folder(&self) -> Option<PathBuf> {
        self.models.clone().or_else(|| {
            self.executable()?
                .parent()
                .map(|dir| dir.join(MODELS_FOLDER))
        })
    }
}

/// Why there are no words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OcrError {
    /// No worker could be started, or no model loaded: nothing about the picture.
    Unavailable(String),
    /// The picture could not be read, or the worker died reading it.
    Failed(String),
}

impl std::fmt::Display for OcrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(reason) => write!(f, "the OCR is not available: {reason}"),
            Self::Failed(reason) => write!(f, "the OCR failed: {reason}"),
        }
    }
}

impl std::error::Error for OcrError {}

struct Worker {
    child: SandboxedChild,
    stdin: File,
    replies: Receiver<std::io::Result<Option<Reply>>>,
    /// The languages its engine was made for.
    languages: Option<String>,
}

/// Keeps one worker, replacing it whenever it stops being trustworthy.
pub struct OcrClient {
    config: OcrConfig,
    worker: Option<Worker>,
    models: BTreeMap<String, Arc<Vec<u8>>>,
    restarts: u64,
    cannot_start: Option<String>,
}

impl OcrClient {
    pub fn new(config: OcrConfig) -> Self {
        Self {
            config,
            worker: None,
            models: BTreeMap::new(),
            restarts: 0,
            cannot_start: None,
        }
    }

    /// The languages there are models for, by Tesseract's names.
    pub fn languages(&self) -> Vec<String> {
        let Some(folder) = self.config.models_folder() else {
            return Vec::new();
        };
        let mut found: Vec<String> = std::fs::read_dir(folder)
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                let name = entry.ok()?.file_name().into_string().ok()?;
                let language = name.strip_suffix(".traineddata")?;
                is_language(language).then(|| language.to_owned())
            })
            .collect();
        found.sort();
        found
    }

    /// How many workers had to be thrown away.
    pub fn restarts(&self) -> u64 {
        self.restarts
    }

    /// The process id of the current worker, if one is running.
    pub fn worker_id(&self) -> Option<u32> {
        self.worker.as_ref().map(|worker| worker.child.id())
    }

    /// The words of `picture`, read in `languages` (`por+eng`) in `mode` at `ppi`.
    pub fn recognize(
        &mut self,
        languages: &str,
        picture: &Gray,
        mode: Mode,
        ppi: u32,
    ) -> Result<Vec<Word>, OcrError> {
        if !picture_fits(picture.width, picture.height)
            || picture.pixels.len() != picture.width as usize * picture.height as usize
        {
            return Err(OcrError::Failed(
                "the picture is empty or too large".to_owned(),
            ));
        }
        self.ready(languages)?;
        let timeout = self.config.timeout;
        let worker = self
            .worker
            .as_mut()
            .ok_or_else(|| OcrError::Unavailable("no worker".to_owned()))?;
        let sent = write_frame(
            &mut worker.stdin,
            &Request::Recognize {
                width: picture.width,
                height: picture.height,
                mode,
                ppi,
            },
        )
        .and_then(|()| worker.stdin.write_all(&picture.pixels))
        .and_then(|()| worker.stdin.flush());
        if let Err(error) = sent {
            return Err(self.discard(&format!("it could not be sent the picture: {error}")));
        }
        match next_reply(worker, timeout) {
            Ok(Reply::Words(words)) => match checked(words, picture) {
                Ok(words) => Ok(words),
                Err(reason) => Err(self.discard(&reason)),
            },
            Ok(Reply::Failed { reason }) => Err(OcrError::Failed(clean(&reason))),
            Ok(other) => Err(self.discard(&format!("it answered {other:?}"))),
            Err(reason) => Err(self.discard(&reason)),
        }
    }

    /// A worker with an engine for `languages`.
    fn ready(&mut self, languages: &str) -> Result<(), OcrError> {
        if let Some(reason) = &self.cannot_start {
            return Err(OcrError::Unavailable(reason.clone()));
        }
        let wanted: Vec<&str> = languages.split('+').collect();
        if wanted.is_empty() || wanted.len() > MAX_MODELS || !wanted.iter().all(|l| is_language(l))
        {
            return Err(OcrError::Unavailable(format!(
                "no such languages: {languages}"
            )));
        }
        for language in &wanted {
            if !self.models.contains_key(*language) {
                let bytes = self.read_model(language)?;
                self.models.insert((*language).to_owned(), Arc::new(bytes));
            }
        }
        if self.worker.is_none() {
            match self.start_worker() {
                Ok(worker) => self.worker = Some(worker),
                Err(reason) => {
                    self.cannot_start = Some(reason.clone());
                    return Err(OcrError::Unavailable(reason));
                }
            }
        }
        let timeout = self.config.timeout;
        let models: Vec<(String, Arc<Vec<u8>>)> = wanted
            .iter()
            .map(|language| ((*language).to_owned(), Arc::clone(&self.models[*language])))
            .collect();
        let worker = self
            .worker
            .as_mut()
            .ok_or_else(|| OcrError::Unavailable("no worker".to_owned()))?;
        if worker.languages.as_deref() == Some(languages) {
            return Ok(());
        }
        let request = Request::Load {
            models: models
                .iter()
                .map(|(language, bytes)| ModelHeader {
                    language: language.clone(),
                    bytes: bytes.len() as u64,
                })
                .collect(),
            languages: languages.to_owned(),
        };
        let sent = write_frame(&mut worker.stdin, &request).and_then(|()| {
            for (_, bytes) in &models {
                worker.stdin.write_all(bytes)?;
            }
            worker.stdin.flush()
        });
        if let Err(error) = sent {
            return Err(self.discard(&format!("it could not be sent the models: {error}")));
        }
        // Loading takes longer than reading a picture, the first time.
        match next_reply(worker, timeout * 2) {
            Ok(Reply::Loaded) => {
                worker.languages = Some(languages.to_owned());
                Ok(())
            }
            Ok(Reply::Failed { reason }) => {
                let reason = clean(&reason);
                self.discard(&reason);
                Err(OcrError::Unavailable(reason))
            }
            Ok(other) => Err(self.discard(&format!("it answered {other:?}"))),
            Err(reason) => Err(self.discard(&reason)),
        }
    }

    fn read_model(&self, language: &str) -> Result<Vec<u8>, OcrError> {
        let folder = self.config.models_folder().ok_or_else(|| {
            OcrError::Unavailable("this program cannot tell where it is".to_owned())
        })?;
        let path = folder.join(format!("{language}.traineddata"));
        let size = std::fs::metadata(&path)
            .map_err(|error| OcrError::Unavailable(format!("{}: {error}", path.display())))?
            .len();
        if size > MAX_MODEL {
            return Err(OcrError::Unavailable(format!(
                "{} is too large",
                path.display()
            )));
        }
        std::fs::read(&path)
            .map_err(|error| OcrError::Unavailable(format!("{}: {error}", path.display())))
    }

    fn discard(&mut self, reason: &str) -> OcrError {
        // Dropping it kills the process, and with it anything a picture may have done to it.
        self.worker = None;
        self.restarts += 1;
        OcrError::Failed(format!("the OCR worker was replaced: {reason}"))
    }

    fn start_worker(&self) -> Result<Worker, String> {
        let exe = self
            .config
            .executable()
            .ok_or("this program cannot tell where it is")?;
        start_worker(&exe, &self.config).map_err(|error| format!("{}: {error}", exe.display()))
    }
}

fn start_worker(exe: &Path, config: &OcrConfig) -> Result<Worker, String> {
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
        .name("ocr-reader".to_owned())
        .spawn(move || {
            loop {
                let message = read_frame::<Reply>(&mut stdout);
                let finished = !matches!(message, Ok(Some(_)));
                if sender.send(message).is_err() || finished {
                    return;
                }
            }
        })
        .map_err(|error| error.to_string())?;
    // The pipe has to keep being drained, or a worker writing a lot would block.
    thread::Builder::new()
        .name("ocr-log".to_owned())
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
        languages: None,
    };
    match next_reply(&mut worker, config.timeout)? {
        Reply::Ready { protocol } if protocol == PROTOCOL_VERSION => {}
        other => return Err(format!("it did not say it was ready, but {other:?}")),
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

fn next_reply(worker: &mut Worker, timeout: Duration) -> Result<Reply, String> {
    match worker.replies.recv_timeout(timeout) {
        Ok(Ok(Some(reply))) => Ok(reply),
        Ok(Ok(None)) => Err("it closed its pipe".to_owned()),
        Ok(Err(error)) => Err(format!("it said something unreadable: {error}")),
        Err(RecvTimeoutError::Timeout) => Err(format!("it took more than {timeout:?}")),
        Err(RecvTimeoutError::Disconnected) => Err("it is gone".to_owned()),
    }
}

/// Text from the worker, without control characters and not too long.
fn clean(text: &str) -> String {
    text.chars()
        .filter(|ch| !ch.is_control())
        .take(MAX_WORD_CHARS)
        .collect()
}

/// The words as the worker sent them, made safe to keep: a word is short and printable, its box
/// inside the picture, its confidence between 0 and 100.
fn checked(words: Vec<Word>, picture: &Gray) -> Result<Vec<Word>, String> {
    if words.len() > MAX_WORDS {
        return Err(format!("it sent {} words", words.len()));
    }
    let (w, h) = (picture.width as i32, picture.height as i32);
    Ok(words
        .into_iter()
        .filter_map(|word| {
            let text = clean(&word.text).trim().to_owned();
            if text.is_empty() {
                return None;
            }
            let left = word.left.clamp(0, w);
            let right = word.right.clamp(left, w);
            let top = word.top.clamp(0, h);
            let bottom = word.bottom.clamp(top, h);
            Some(Word {
                text,
                left,
                top,
                right,
                bottom,
                confidence: if word.confidence.is_finite() {
                    word.confidence.clamp(0.0, 100.0)
                } else {
                    0.0
                },
                block: word.block.min(100_000),
                paragraph: word.paragraph.min(100_000),
                line: word.line.min(100_000),
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_a_worker_says_is_made_safe() {
        let picture = Gray {
            width: 100,
            height: 20,
            pixels: vec![255; 2000],
        };
        let words = vec![
            Word {
                text: "ok\u{7}".to_owned(),
                left: -5,
                top: 2,
                right: 500,
                bottom: 30,
                confidence: 140.0,
                block: 0,
                paragraph: 0,
                line: 0,
            },
            Word {
                text: " \u{1b} ".to_owned(),
                left: 0,
                top: 0,
                right: 1,
                bottom: 1,
                confidence: f32::NAN,
                block: 0,
                paragraph: 0,
                line: 0,
            },
        ];
        let words = checked(words, &picture).unwrap();
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].text, "ok");
        assert_eq!(
            (words[0].left, words[0].right, words[0].bottom),
            (0, 100, 20)
        );
        assert_eq!(words[0].confidence, 100.0);
    }
}

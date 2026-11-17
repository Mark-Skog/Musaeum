//! Covers for the library's grid, and the `thumb` protocol that serves them.
//!
//! A fixed-layout book's cover is its first page, drawn by that format's own worker in its sandbox,
//! as when it is read. A flowing book's cover is the picture the book names, found by the
//! normalizer in the ingest worker; its bytes are then drawn by the comic worker, in its sandbox,
//! as a one-page comic. Either way, what reaches the app's page is a JPEG this program encoded from
//! pixels: no picture from a book is ever decoded where the app's IPC lives.
//!
//! Covers are kept with the page thumbnails, by the file's BLAKE3, so a cover drawn once is never
//! drawn again for the same bytes, wherever the file goes.
//!
//! | Request | Answer |
//! |---------|--------|
//! | `GET http://thumb.localhost/<blake3>.jpg` | The cover, or 404 while there is none |

use std::borrow::Cow;
use std::collections::HashSet;
use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use musaeum_core::thumbnail::encode_jpeg;
use musaeum_core::{
    BackgroundResult, BackgroundWork, DeliveryMode, FixedFormat, PoolConfig, RenderPool,
};
use musaeum_flow::zipwrite::ZipWriter;
use musaeum_ingest::protocol::Format;
use musaeum_library::{BookFile, CoverOutcome, Hash, IngestClient, IngestConfig, Library};
use musaeum_shm::slab::slab_len;
use musaeum_shm::{PixelSlab, ShmRegion};
use tauri::http::{Request, Response, StatusCode, header};
use tauri::{AppHandle, Emitter, Manager, UriSchemeResponder};

use crate::reading;
use crate::session::AppState;

/// The protocol's scheme name.
pub const SCHEME: &str = "thumb";
/// Sent to the page with the hashes of new covers.
pub const COVERS_CHANGED: &str = "covers-changed";

/// Slots for one thumbnail and a little room: each slot holds a 512 px tile.
const SLOTS: u32 = 2;
/// How long a worker may take to start and draw one cover.
const DRAW_TIMEOUT: Duration = Duration::from_secs(60);
/// Covers announced together, at most this often, so a large library does not flood the page.
const ANNOUNCE_EVERY: Duration = Duration::from_millis(500);

/// The hex form of a hash, as the protocol and the page name covers.
pub fn hex(hash: &Hash) -> String {
    hash.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn from_hex(text: &str) -> Option<Hash> {
    if text.len() != 64 {
        return None;
    }
    let mut hash = [0u8; 32];
    for (index, byte) in hash.iter_mut().enumerate() {
        *byte = u8::from_str_radix(text.get(index * 2..index * 2 + 2)?, 16).ok()?;
    }
    Some(hash)
}

#[derive(Default)]
struct Signal {
    wanted: Mutex<bool>,
    changed: Condvar,
    stop: AtomicBool,
}

/// Wakes the cover service when the library may have new books.
#[derive(Clone)]
pub struct CoverWaker(Arc<Signal>);

impl CoverWaker {
    pub fn wake(&self) {
        *self.0.wanted.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.0.changed.notify_all();
    }
}

/// Draws the covers that are missing, one at a time, at background priority. Dropping it stops it
/// after the cover underway.
pub struct CoverService {
    signal: Arc<Signal>,
    thread: Option<JoinHandle<()>>,
}

impl CoverService {
    pub fn start(library: Arc<Library>, app: AppHandle) -> Self {
        let signal = Arc::new(Signal::default());
        *signal.wanted.lock().unwrap_or_else(PoisonError::into_inner) = true;
        let thread = {
            let signal = Arc::clone(&signal);
            thread::Builder::new()
                .name("covers".to_owned())
                .spawn(move || run(&library, &app, &signal))
                .map_err(|error| eprintln!("could not start drawing covers: {error}"))
                .ok()
        };
        Self { signal, thread }
    }

    pub fn waker(&self) -> CoverWaker {
        CoverWaker(Arc::clone(&self.signal))
    }
}

impl Drop for CoverService {
    fn drop(&mut self) {
        self.signal.stop.store(true, Ordering::Relaxed);
        self.signal.changed.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(library: &Library, app: &AppHandle, signal: &Signal) {
    let _background = musaeum_fs::enter_background();
    let mut client: Option<IngestClient> = None;
    // Books whose cover failed this run for a reason that may pass, such as a worker that could
    // not start. They are tried again the next time the app runs.
    let mut failed: HashSet<Hash> = HashSet::new();
    loop {
        {
            let mut wanted = signal.wanted.lock().unwrap_or_else(PoisonError::into_inner);
            while !*wanted && !signal.stop.load(Ordering::Relaxed) {
                wanted = signal
                    .changed
                    .wait(wanted)
                    .unwrap_or_else(PoisonError::into_inner);
            }
            *wanted = false;
        }
        if signal.stop.load(Ordering::Relaxed) {
            return;
        }
        let files = library.hashed_book_files().unwrap_or_else(|error| {
            eprintln!("could not list the books for covers: {error}");
            Vec::new()
        });
        let store = library.thumbnails();
        let mut drawn: Vec<String> = Vec::new();
        let mut announced = Instant::now();
        for file in files {
            if signal.stop.load(Ordering::Relaxed) {
                return;
            }
            let Some(hash) = file.blake3 else {
                continue;
            };
            if store.path(&hash, 0).exists()
                || store.is_without_cover(&hash)
                || failed.contains(&hash)
            {
                continue;
            }
            let result = if file.layout == "fixed" {
                draw_page(library, &file.path, &hash, true)
            } else {
                draw_flowing(library, &file, &hash, &mut client)
            };
            match result {
                Ok(true) => drawn.push(hex(&hash)),
                Ok(false) => {}
                Err(reason) => {
                    eprintln!("no cover for {}: {reason}", file.path.display());
                    failed.insert(hash);
                }
            }
            if !drawn.is_empty() && announced.elapsed() >= ANNOUNCE_EVERY {
                let _ = app.emit(COVERS_CHANGED, std::mem::take(&mut drawn));
                announced = Instant::now();
            }
        }
        if !drawn.is_empty() {
            let _ = app.emit(COVERS_CHANGED, drawn);
        }
    }
}

/// Draws the first page of a fixed-layout file as the cover of the file with `hash`. Returns
/// whether there is now a cover; a file with no first page to draw is marked as having none.
fn draw_page(library: &Library, path: &Path, hash: &Hash, own_file: bool) -> Result<bool, String> {
    let store = library.thumbnails();
    let format = FixedFormat::of_file(path)
        .map_err(|error| error.to_string())?
        .ok_or("not a PDF, comic or DjVu file")?;
    let worker_exe = std::env::current_exe()
        .map_err(|error| error.to_string())?
        .with_file_name(format.worker_exe());
    // Pages measured before are reused, but only for the file itself: a flowing book's cover is
    // drawn from a comic made for it, whose pages are not the book's.
    let known = own_file
        .then(|| library.page_geometry(hash).ok().flatten())
        .flatten();
    let region = ShmRegion::create(slab_len(SLOTS)).map_err(|error| error.to_string())?;
    let slab = PixelSlab::init(region, SLOTS).map_err(|error| error.to_string())?;
    let pool = RenderPool::start(
        PoolConfig {
            worker_exe,
            document: path.to_owned(),
            worker_count: 1,
            process_memory_limit: Some(1 << 30),
            policy: format.policy(),
            startup_timeout: DRAW_TIMEOUT,
            known_geometry: known.as_ref().map(reading::known_geometry),
        },
        Arc::new(slab),
        DeliveryMode::Pull { max_batch: 1 },
    )
    .map_err(|error| error.to_string())?;
    if own_file && known.is_none() {
        let measured = reading::measured(pool.pages(), pool.hints());
        if let Err(error) = library.store_page_geometry(hash, &measured) {
            eprintln!("could not keep the page sizes: {error}");
        }
    }
    if pool.pages().is_empty() {
        pool.shutdown();
        let _ = store.mark_without_cover(hash);
        return Ok(false);
    }
    let (sender, results) = mpsc::channel();
    pool.start_background(
        BackgroundWork {
            analyze: Vec::new(),
            thumbnails: vec![0],
            lines: Vec::new(),
        },
        Box::new(move |result| {
            let _ = sender.send(result);
        }),
    );
    let outcome = loop {
        match results.recv_timeout(DRAW_TIMEOUT) {
            Ok(BackgroundResult::Thumbnail {
                width,
                height,
                pixels,
                ..
            }) => {
                break encode_jpeg(width, height, &pixels).and_then(|jpeg| {
                    store
                        .put(hash, 0, &jpeg)
                        .map(|()| true)
                        .map_err(|error| error.to_string())
                });
            }
            Ok(BackgroundResult::Failed { .. }) => {
                let _ = store.mark_without_cover(hash);
                break Ok(false);
            }
            Ok(BackgroundResult::Finished) => break Ok(false),
            Ok(BackgroundResult::Content { .. } | BackgroundResult::Lines { .. }) => {}
            Err(_) => break Err("the worker did not draw the cover in time".to_owned()),
        }
    };
    pool.shutdown();
    outcome
}

/// Finds a flowing book's cover in the ingest worker and draws it through the comic worker.
fn draw_flowing(
    library: &Library,
    file: &BookFile,
    hash: &Hash,
    client: &mut Option<IngestClient>,
) -> Result<bool, String> {
    let store = library.thumbnails();
    let format = Format::from_extension(&file.format).ok_or("not a format the worker reads")?;
    if matches!(format, Format::Txt | Format::Markdown | Format::Pdf) {
        let _ = store.mark_without_cover(hash);
        return Ok(false);
    }
    let client = client.get_or_insert_with(|| IngestClient::new(IngestConfig::for_normalizing()));
    let book = File::open(&file.path).map_err(|error| error.to_string())?;
    match client.cover(&book, format) {
        CoverOutcome::Cover { kind, bytes } => {
            let work = library.paths().cache_dir.join("covers-work");
            std::fs::create_dir_all(&work).map_err(|error| error.to_string())?;
            let comic = work.join(format!("{}.{}.cbz", hex(hash), std::process::id()));
            let mut zip = ZipWriter::new();
            zip.add(&format!("cover.{}", kind.extension()), &bytes, false);
            std::fs::write(&comic, zip.finish()).map_err(|error| error.to_string())?;
            let drawn = draw_page(library, &comic, hash, false);
            let _ = std::fs::remove_file(&comic);
            if !matches!(drawn, Ok(true)) {
                let _ = store.mark_without_cover(hash);
            }
            drawn.or(Ok(false))
        }
        CoverOutcome::NoCover | CoverOutcome::Nothing(_) => {
            let _ = store.mark_without_cover(hash);
            Ok(false)
        }
        CoverOutcome::Unavailable(reason) => Err(reason),
    }
}

fn respond(responder: UriSchemeResponder, status: StatusCode) {
    let mut response = Response::new(Cow::<'static, [u8]>::Borrowed(&[]));
    *response.status_mut() = status;
    responder.respond(response);
}

/// Serves a cover. Reading it happens off the UI thread.
pub fn handle(app: &AppHandle, request: Request<Vec<u8>>, responder: UriSchemeResponder) {
    let Some(hash) = request
        .uri()
        .path()
        .strip_prefix('/')
        .and_then(|name| name.strip_suffix(".jpg"))
        .and_then(from_hex)
    else {
        respond(responder, StatusCode::NOT_FOUND);
        return;
    };
    let Some(library) = app.state::<AppState>().shelf.current() else {
        respond(responder, StatusCode::NOT_FOUND);
        return;
    };
    thread::spawn(move || match library.thumbnails().get(&hash, 0) {
        Ok(Some(jpeg)) => {
            let response = Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "image/jpeg")
                // The name is the file's hash: what it names never changes.
                .header(header::CACHE_CONTROL, "max-age=31536000, immutable")
                .header("Cross-Origin-Resource-Policy", "cross-origin")
                .header("X-Content-Type-Options", "nosniff")
                .body(Cow::<'static, [u8]>::Owned(jpeg));
            match response {
                Ok(response) => responder.respond(response),
                Err(_) => respond(responder, StatusCode::INTERNAL_SERVER_ERROR),
            }
        }
        _ => respond(responder, StatusCode::NOT_FOUND),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cover_is_named_by_its_hash_and_nothing_else() {
        let hash: Hash = std::array::from_fn(|index| index as u8 * 7);
        assert_eq!(from_hex(&hex(&hash)), Some(hash));
        assert_eq!(from_hex("../../library.db"), None);
        assert_eq!(from_hex(&"g".repeat(64)), None);
        assert_eq!(from_hex(&"a".repeat(63)), None);
    }
}

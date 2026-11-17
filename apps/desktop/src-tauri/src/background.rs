//! What the workers do while nobody is waiting for a tile: classify every page, draw its
//! thumbnail and, on a page that is a picture (scanned, or scanned with OCR's text over it), find
//! its lines of text, for a document the library knows by its bytes. The pool schedules it at its
//! lowest priority; this keeps the results, on a thread in Windows' background mode, where the
//! line detector runs too.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use musaeum_core::thumbnail::encode_jpeg;
use musaeum_core::{BackgroundResult, BackgroundTask, BackgroundWork, PoolHandle, RenderPool};
use musaeum_library::{Hash, Library, LineModelRecord, PageContentClass, PageContentRecord};
use musaeum_shm::ContentClass;
use musaeum_vision::{DetectOptions, Gray, LINES_VERSION, detect, dhash};

/// Pages classified before they are written together, so the cache takes one transaction for many.
const BATCH: usize = 64;
/// How long the keeper waits for a page's rotation, which comes with its characters.
const ROTATION_TIMEOUT: Duration = Duration::from_secs(10);

/// What is left to do for a document, as found when it opened.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pending {
    /// Pages not classified by the current rules.
    pub to_classify: usize,
    /// Pages with no thumbnail on disk.
    pub to_draw: usize,
    /// Pages known to be pictures whose lines were not traced by the current detector. Pages
    /// classified as pictures in this session are traced too, and not counted here.
    pub to_trace: usize,
}

pub fn is_picture(class: PageContentClass) -> bool {
    matches!(class, PageContentClass::Scanned | PageContentClass::Hybrid)
}

/// Starts whatever is missing for the document with these bytes, and returns how much that is.
pub fn start(pool: &RenderPool, library: Arc<Library>, blake3: Hash) -> Pending {
    let page_count = pool.pages().len() as u32;
    let (analyze, pictures) = match library.page_content(&blake3) {
        Ok(Some(content)) => {
            let pictures: Vec<u32> = (0..content.pages.len() as u32)
                .filter(|&page| {
                    content.pages[page as usize].is_some_and(|record| is_picture(record.class))
                })
                .collect();
            (content.unclassified(), pictures)
        }
        // Its pages were never measured into the cache, so there is nowhere to keep classes.
        Ok(None) => (Vec::new(), Vec::new()),
        Err(error) => {
            eprintln!("could not read the page classes: {error}");
            (Vec::new(), Vec::new())
        }
    };
    let thumbnails = library
        .thumbnails()
        .missing(&blake3, page_count)
        .unwrap_or_else(|error| {
            eprintln!("could not look for thumbnails: {error}");
            Vec::new()
        });
    let lines = library
        .pages_without_line_model(&blake3, &pictures, LINES_VERSION)
        .unwrap_or_else(|error| {
            eprintln!("could not look for line models: {error}");
            Vec::new()
        });
    let pending = Pending {
        to_classify: analyze.len(),
        to_draw: thumbnails.len(),
        to_trace: lines.len(),
    };
    // The keeper runs even with nothing left to do: a page the reader asks the lines of later
    // goes through it too.
    let (sender, results) = mpsc::channel();
    let handle = pool.handle();
    let spawned = thread::Builder::new()
        .name("background-results".into())
        .spawn(move || keep(&results, &handle, &library, &blake3));
    if let Err(error) = spawned {
        eprintln!("could not start keeping background results: {error}");
        return Pending::default();
    }
    pool.start_background(
        BackgroundWork {
            analyze,
            thumbnails,
            lines,
        },
        Box::new(move |result| {
            let _ = sender.send(result);
        }),
    );
    pending
}

fn class_of(class: ContentClass) -> PageContentClass {
    match class {
        ContentClass::Text => PageContentClass::Text,
        ContentClass::Scanned => PageContentClass::Scanned,
        ContentClass::Hybrid => PageContentClass::Hybrid,
    }
}

/// Finds the lines on a page's picture, as shown, and makes the record the cache keeps. The
/// rotation comes from the page's characters: the model is in fractions of the unrotated crop
/// box, as the characters are, so the two line up whatever the page's `/Rotate`.
fn trace(
    handle: &PoolHandle,
    page: u32,
    width: u32,
    height: u32,
    gray: Vec<u8>,
) -> Result<LineModelRecord, String> {
    let rotation = handle
        .request_chars(page)
        .recv_timeout(ROTATION_TIMEOUT)
        .map_err(|_| "the pool did not say how the page is turned".to_owned())?
        .map_err(|error| format!("the page's characters: {error}"))?
        .rotation;
    let picture = Gray {
        width,
        height,
        pixels: gray,
    };
    let found = detect(
        &picture,
        &DetectOptions {
            rotation,
            ..DetectOptions::default()
        },
    );
    let page_hash = dhash(&picture, 0, 0, i64::from(width), i64::from(height));
    Ok(LineModelRecord {
        page,
        version: LINES_VERSION,
        page_hash,
        model: found.model.encode(),
    })
}

/// Writes results until the pool is gone. Encoding a thumbnail, finding a page's lines and
/// writing them take this thread's time, never the pool's.
fn keep(
    results: &Receiver<BackgroundResult>,
    handle: &PoolHandle,
    library: &Library,
    blake3: &Hash,
) {
    let _background = musaeum_fs::enter_background();
    let store = library.thumbnails();
    let mut classified = Vec::with_capacity(BATCH);
    // Once a batch of classes is kept, the pages in it that are pictures are traced too.
    let flush = |classified: &mut Vec<(u32, PageContentRecord)>| {
        if classified.is_empty() {
            return;
        }
        if let Err(error) = library.store_page_content(blake3, classified) {
            eprintln!("could not keep the page classes: {error}");
            classified.clear();
            return;
        }
        let pictures: Vec<u32> = classified
            .iter()
            .filter(|(_, record)| is_picture(record.class))
            .map(|(page, _)| *page)
            .collect();
        classified.clear();
        match library.pages_without_line_model(blake3, &pictures, LINES_VERSION) {
            Ok(lines) if !lines.is_empty() => handle.add_background(BackgroundWork {
                lines,
                ..BackgroundWork::default()
            }),
            Ok(_) => {}
            Err(error) => eprintln!("could not look for line models: {error}"),
        }
    };
    while let Ok(result) = results.recv() {
        match result {
            BackgroundResult::Content { page, content } => {
                classified.push((
                    page,
                    PageContentRecord {
                        class: class_of(content.class()),
                        text_chars: content.text_chars,
                        image_coverage: content.image_coverage,
                    },
                ));
                if classified.len() >= BATCH {
                    flush(&mut classified);
                }
            }
            BackgroundResult::Thumbnail {
                page,
                width,
                height,
                pixels,
            } => {
                let kept = encode_jpeg(width, height, &pixels).and_then(|jpeg| {
                    store
                        .put(blake3, page, &jpeg)
                        .map_err(|error| error.to_string())
                });
                if let Err(error) = kept {
                    eprintln!("could not keep the thumbnail of page {page}: {error}");
                }
            }
            BackgroundResult::Lines {
                page,
                width,
                height,
                gray,
            } => {
                let kept = trace(handle, page, width, height, gray).and_then(|record| {
                    library
                        .store_line_models(blake3, &[record])
                        .map_err(|error| error.to_string())
                });
                if let Err(error) = kept {
                    eprintln!("could not keep the lines of page {page}: {error}");
                }
            }
            BackgroundResult::Failed { page, task } => {
                let what = match task {
                    BackgroundTask::Classify => "classified",
                    BackgroundTask::Thumbnail => "drawn",
                    BackgroundTask::Lines => "drawn for its lines",
                };
                eprintln!("page {page} could not be {what}");
            }
            BackgroundResult::Finished => flush(&mut classified),
        }
    }
    flush(&mut classified);
}

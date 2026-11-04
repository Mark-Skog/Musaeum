//! What the workers do while nobody is waiting for a tile: classify every page and draw its
//! thumbnail, for a document the library knows by its bytes. The pool schedules it at its lowest
//! priority; this keeps the results, on a thread in Windows' background mode.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::thread;

use musaeum_core::thumbnail::encode_jpeg;
use musaeum_core::{BackgroundResult, BackgroundWork, RenderPool};
use musaeum_library::{Hash, Library, PageContentClass, PageContentRecord};
use musaeum_shm::ContentClass;

/// Pages classified before they are written together, so the cache takes one transaction for many.
const BATCH: usize = 64;

/// What is left to do for a document, as found when it opened.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pending {
    /// Pages not classified by the current rules.
    pub to_classify: usize,
    /// Pages with no thumbnail on disk.
    pub to_draw: usize,
}

/// Starts whatever is missing for the document with these bytes, and returns how much that is.
pub fn start(pool: &RenderPool, library: Arc<Library>, blake3: Hash) -> Pending {
    let page_count = pool.pages().len() as u32;
    let analyze = match library.page_content(&blake3) {
        Ok(Some(content)) => content.unclassified(),
        // Its pages were never measured into the cache, so there is nowhere to keep classes.
        Ok(None) => Vec::new(),
        Err(error) => {
            eprintln!("could not read the page classes: {error}");
            Vec::new()
        }
    };
    let thumbnails = library
        .thumbnails()
        .missing(&blake3, page_count)
        .unwrap_or_else(|error| {
            eprintln!("could not look for thumbnails: {error}");
            Vec::new()
        });
    let pending = Pending {
        to_classify: analyze.len(),
        to_draw: thumbnails.len(),
    };
    if analyze.is_empty() && thumbnails.is_empty() {
        return pending;
    }

    let (sender, results) = mpsc::channel();
    let spawned = thread::Builder::new()
        .name("background-results".into())
        .spawn(move || keep(&results, &library, &blake3));
    if let Err(error) = spawned {
        eprintln!("could not start keeping background results: {error}");
        return Pending::default();
    }
    pool.start_background(
        BackgroundWork {
            analyze,
            thumbnails,
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

/// Writes results until the pool is gone. Encoding a thumbnail and writing it take this thread's
/// time, never the pool's.
fn keep(results: &Receiver<BackgroundResult>, library: &Library, blake3: &Hash) {
    let _background = musaeum_fs::enter_background();
    let store = library.thumbnails();
    let mut classified = Vec::with_capacity(BATCH);
    let flush = |classified: &mut Vec<(u32, PageContentRecord)>| {
        if classified.is_empty() {
            return;
        }
        if let Err(error) = library.store_page_content(blake3, classified) {
            eprintln!("could not keep the page classes: {error}");
        }
        classified.clear();
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
            BackgroundResult::Failed { page, thumbnail } => {
                let what = if thumbnail { "drawn" } else { "classified" };
                eprintln!("page {page} could not be {what}");
            }
            BackgroundResult::Finished => flush(&mut classified),
        }
    }
    flush(&mut classified);
}

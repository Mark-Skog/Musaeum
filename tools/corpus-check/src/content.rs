//! What every page of a real file is made of, and its thumbnails, through the same pool and
//! background work the reader uses: classes measured by the sandboxed worker, decided by the host,
//! kept in `cache.db`, and thumbnails encoded by the host into the store on disk.

use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use musaeum_core::thumbnail::encode_jpeg;
use musaeum_core::{BackgroundResult, BackgroundWork, DeliveryMode, DocumentHints, RenderPool};
use musaeum_library::{Hash, Library, PageContentClass, PageContentRecord, ThumbnailStore};
use musaeum_shm::{ContentClass, PageGeometry};
use serde::Serialize;

use crate::render::{OpenFailure, config, pool_error, slab};

/// Longest a whole document's background work may take.
const BACKGROUND_WAIT: Duration = Duration::from_secs(1800);

/// What one document's pages turned out to be made of.
#[derive(Debug, Default, Serialize)]
pub struct ContentRun {
    /// Pages classified.
    pub classified: usize,
    /// Of those, text.
    pub text: usize,
    /// Scanned.
    pub scanned: usize,
    /// Hybrid.
    pub hybrid: usize,
    /// Pages that could not be classified, or drawn.
    pub failed: usize,
    /// The document's summary, as the cache keeps it once every page is known.
    pub summary: Option<String>,
    /// Whether every page has the class of the summary.
    pub is_strictly_uniform: Option<bool>,
    /// Microseconds per page classified, in the worker.
    pub analyze_us: musaeum_core::Summary,
    /// Thumbnails drawn.
    pub thumbnails: usize,
    /// Their size on disk, together.
    pub thumbnail_bytes: u64,
    /// Microseconds per thumbnail, in the worker.
    pub thumbnail_us: musaeum_core::Summary,
    /// Microseconds per thumbnail spent encoding it on the host.
    pub encode_us: musaeum_core::Summary,
    /// Wall time of the whole background run.
    pub wall_ms: f64,
}

fn class_of(class: ContentClass) -> PageContentClass {
    match class {
        ContentClass::Text => PageContentClass::Text,
        ContentClass::Scanned => PageContentClass::Scanned,
        ContentClass::Hybrid => PageContentClass::Hybrid,
    }
}

/// Classifies every page of `pdf` and, with `thumbnails`, draws every page's thumbnail too.
pub fn classify(
    worker_exe: &Path,
    pdf: &Path,
    pages: &[PageGeometry],
    workers: usize,
    thumbnails: bool,
    library: &Library,
    hash: Option<Hash>,
) -> Result<ContentRun, OpenFailure> {
    let mut configuration = config(worker_exe, pdf, workers);
    configuration.known_geometry = Some((pages.to_vec(), DocumentHints::default()));
    let pool = RenderPool::start(
        configuration,
        slab(32).map_err(OpenFailure::Core)?,
        DeliveryMode::Pull { max_batch: 1 },
    )
    .map_err(pool_error)?;
    let pool = Arc::new(pool);

    let count = pages.len() as u32;
    let (sender, results) = mpsc::channel();
    let started = Instant::now();
    pool.start_background(
        BackgroundWork {
            analyze: (0..count).collect(),
            thumbnails: if thumbnails {
                (0..count).collect()
            } else {
                Vec::new()
            },
        },
        Box::new(move |result| {
            let _ = sender.send(result);
        }),
    );

    let store = ThumbnailStore::new(&library.paths().cache_dir);
    let mut run = ContentRun::default();
    let mut records = Vec::with_capacity(pages.len());
    let mut encode_us = Vec::new();
    loop {
        let result = results
            .recv_timeout(BACKGROUND_WAIT)
            .map_err(|_| OpenFailure::Core("background work did not finish".to_owned()))?;
        match result {
            BackgroundResult::Content { page, content } => {
                run.classified += 1;
                match content.class() {
                    ContentClass::Text => run.text += 1,
                    ContentClass::Scanned => run.scanned += 1,
                    ContentClass::Hybrid => run.hybrid += 1,
                }
                records.push((
                    page,
                    PageContentRecord {
                        class: class_of(content.class()),
                        text_chars: content.text_chars,
                        image_coverage: content.image_coverage,
                    },
                ));
            }
            BackgroundResult::Thumbnail {
                page,
                width,
                height,
                pixels,
            } => {
                let encoding = Instant::now();
                let jpeg = encode_jpeg(width, height, &pixels).map_err(OpenFailure::Core)?;
                encode_us.push(u32::try_from(encoding.elapsed().as_micros()).unwrap_or(u32::MAX));
                if let Some(hash) = &hash {
                    store
                        .put(hash, page, &jpeg)
                        .map_err(|error| OpenFailure::Core(error.to_string()))?;
                }
                run.thumbnails += 1;
                run.thumbnail_bytes += jpeg.len() as u64;
            }
            BackgroundResult::Failed { .. } => run.failed += 1,
            BackgroundResult::Finished => break,
        }
    }
    run.wall_ms = started.elapsed().as_secs_f64() * 1000.0;
    let stats = pool.stats().unwrap_or_default();
    run.analyze_us = stats.analyze_us;
    run.thumbnail_us = stats.thumbnail_us;
    run.encode_us = musaeum_core::Summary::of(&encode_us);
    pool.shutdown();

    if let Some(hash) = &hash {
        library
            .store_page_content(hash, &records)
            .map_err(|error| OpenFailure::Core(error.to_string()))?;
        let content = library
            .page_content(hash)
            .map_err(|error| OpenFailure::Core(error.to_string()))?;
        run.summary = content.as_ref().and_then(|content| content.summary.clone());
        run.is_strictly_uniform = content.and_then(|content| content.is_strictly_uniform);
    }
    Ok(run)
}

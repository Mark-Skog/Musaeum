//! Bringing a Calibre library in, from the page: choosing its folder, seeing what it holds, and the
//! import, which waits for the library to read the books' files and brings each one in once it
//! has. The Calibre folder is only read (`musaeum_library::calibre` says how); it becomes one of
//! the folders the library follows, unless one already holds it.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use musaeum_i18n::tr;
use musaeum_library::{CalibreColumn, CalibreReport, Library, read_calibre};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State, WebviewWindow};

use crate::picker::pick_folder;
use crate::session::AppState;
use crate::shelf::owner_of;

/// Sent to the page as an import goes.
pub const CALIBRE_PROGRESS: &str = "calibre-progress";
/// How long the import waits for the library to read files that keep coming, and how often it
/// tries again meanwhile.
const PATIENCE: Duration = Duration::from_secs(120);
const ROUND: Duration = Duration::from_secs(20);

fn library_of(state: &AppState) -> Result<Arc<Library>, String> {
    state
        .shelf
        .library()
        .ok_or_else(|| "there is no library open".to_owned())
}

/// What a Calibre library holds, before it is brought in.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalibrePreview {
    source: String,
    read_from: String,
    books: usize,
    files: usize,
    /// Books with no file on the disk.
    missing: usize,
    columns: Vec<CalibreColumn>,
    /// A few of its titles.
    sample: Vec<String>,
    /// Whether the library already follows the folder.
    followed: bool,
}

fn followed(library: &Library, folder: &Path) -> bool {
    library
        .roots()
        .unwrap_or_default()
        .into_iter()
        .filter(|root| root.status != "removed")
        .any(|root| folder.starts_with(&root.path))
}

#[tauri::command(async)]
pub fn calibre_choose(window: WebviewWindow) -> Result<Option<String>, String> {
    Ok(pick_folder(owner_of(&window), &tr!(PickCalibreFolder))?
        .map(|path| path.to_string_lossy().into_owned()))
}

#[tauri::command(async)]
pub fn calibre_preview(state: State<'_, AppState>, path: String) -> Result<CalibrePreview, String> {
    let library = library_of(&state)?;
    let calibre = read_calibre(Path::new(&path)).map_err(|error| error.to_string())?;
    Ok(CalibrePreview {
        source: calibre.source.to_string_lossy().into_owned(),
        read_from: calibre.read_from.clone(),
        books: calibre.books.len(),
        files: calibre.present_files(),
        missing: calibre
            .books
            .iter()
            .filter(|book| !book.files.iter().any(|file| file.exists))
            .count(),
        sample: calibre
            .books
            .iter()
            .take(8)
            .map(|book| book.title.clone())
            .collect(),
        followed: followed(&library, &calibre.source),
        columns: calibre.columns,
    })
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportProgress {
    /// `following`, `reading` or `done`.
    phase: &'static str,
    report: Option<CalibreReport>,
}

/// Brings a Calibre library in. The folder is followed from now on; its books come in as the
/// library reads their files, which this waits for, telling the page how far it is.
#[tauri::command(async)]
pub fn calibre_import(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> Result<CalibreReport, String> {
    let calibre = read_calibre(Path::new(&path)).map_err(|error| error.to_string())?;
    let tell = |phase: &'static str, report: Option<&CalibreReport>| {
        let _ = app.emit(
            CALIBRE_PROGRESS,
            ImportProgress {
                phase,
                report: report.cloned(),
            },
        );
    };
    let library = library_of(&state)?;
    if !followed(&library, &calibre.source) {
        tell("following", None);
        state.shelf.add_root(&PathBuf::from(&calibre.source))?;
    }
    let library = library_of(&state)?;
    let mut last_waiting = usize::MAX;
    let mut since_progress = Instant::now();
    loop {
        state.shelf.reconcile_now();
        state.shelf.wait_until_idle(ROUND);
        let report = library
            .import_calibre(&calibre)
            .map_err(|error| error.to_string())?;
        if report.waiting == 0 {
            tell("done", Some(&report));
            state.shelf.wake_fulltext();
            return Ok(report);
        }
        tell("reading", Some(&report));
        if report.waiting < last_waiting {
            last_waiting = report.waiting;
            since_progress = Instant::now();
        } else if since_progress.elapsed() >= PATIENCE {
            // The rest are not being read (a file the library cannot open, say): another import
            // brings them in when they are.
            tell("done", Some(&report));
            return Ok(report);
        }
        // The indexer may not have started on what it was just asked to look at.
        std::thread::sleep(Duration::from_secs(1));
    }
}

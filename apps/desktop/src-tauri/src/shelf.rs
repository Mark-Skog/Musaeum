//! The library as the app's own surface uses it: which library is open, the folders it follows, the
//! books in them, and keeping all of that current while the app runs.
//!
//! A library is only opened where the command line says (`--library`), or in the usual place when
//! there is one there already. A new one is only created when a person picks a folder of books, and
//! then only in one of those two places. Benchmarks open it as before and nothing more: no indexer,
//! no covers, nothing running beside what they measure.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::thread;
use std::time::Duration;

use musaeum_i18n::tr;
use musaeum_library::{
    BrowsePage, BrowseQuery, FolderEntry, Indexer, IndexerConfig, IndexerEvent, Library,
    LibraryOptions, LibraryPaths, MAX_BROWSE,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State, WebviewWindow};

use crate::covers::{CoverService, CoverWaker};
use crate::fulltext::{FullTextService, FullTextWaker, Progress};
use crate::picker::{pick_folder, pick_folders};
use crate::session::AppState;

/// Sent to the page whenever what the library holds may have changed.
pub const LIBRARY_CHANGED: &str = "library-changed";

struct Opened {
    // These stop and join their threads when dropped, which happens first, in this order.
    indexer: Option<Indexer>,
    covers: Option<CoverService>,
    fulltext: Option<FullTextService>,
    library: Arc<Library>,
    location: PathBuf,
}

#[derive(Default)]
struct Inner {
    opened: Option<Opened>,
    /// Whether the library the command line or the usual place names has been looked for.
    looked: bool,
    error: Option<String>,
}

/// The open library, if there is one, with what keeps it current.
pub struct Shelf {
    inner: Mutex<Inner>,
    /// `--library`, if it was given.
    named: Option<PathBuf>,
    /// A benchmark: the library is opened as before and left alone.
    quiet: bool,
    app: OnceLock<AppHandle>,
}

impl Shelf {
    pub fn new(named: Option<PathBuf>, quiet: bool) -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            named,
            quiet,
            app: OnceLock::new(),
        }
    }

    /// Gives the shelf the app, for its events. Called once, when the app starts.
    pub fn set_app(&self, app: AppHandle) {
        let _ = self.app.set(app);
    }

    /// The open library, opening the one the command line or the usual place names the first time.
    pub fn library(&self) -> Option<Arc<Library>> {
        let mut inner = self.lock();
        if !inner.looked {
            inner.looked = true;
            if let Some(paths) = self.existing_paths() {
                self.open_into(&mut inner, paths);
            }
        }
        inner
            .opened
            .as_ref()
            .map(|opened| Arc::clone(&opened.library))
    }

    /// The open library, without looking for one.
    pub fn current(&self) -> Option<Arc<Library>> {
        self.lock()
            .opened
            .as_ref()
            .map(|opened| Arc::clone(&opened.library))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Where a library already is, if there is one to open without creating anything.
    fn existing_paths(&self) -> Option<LibraryPaths> {
        match &self.named {
            Some(directory) => Some(LibraryPaths::in_directory(directory)),
            None => LibraryPaths::default_location()
                .ok()
                .filter(|paths| paths.library_db().exists()),
        }
    }

    /// Where a new library goes.
    fn new_paths(&self) -> Result<LibraryPaths, String> {
        match &self.named {
            Some(directory) => Ok(LibraryPaths::in_directory(directory)),
            None => LibraryPaths::default_location().map_err(|error| error.to_string()),
        }
    }

    fn open_into(&self, inner: &mut Inner, paths: LibraryPaths) {
        let location = paths.data_dir.clone();
        match Library::open(paths, LibraryOptions::default()) {
            Ok(library) => {
                let library = Arc::new(library);
                let (indexer, covers, fulltext) = if self.quiet {
                    (None, None, None)
                } else {
                    self.start_keeping(&library)
                };
                inner.opened = Some(Opened {
                    indexer,
                    covers,
                    fulltext,
                    library,
                    location,
                });
                inner.error = None;
            }
            Err(error) => {
                eprintln!("reading without a library, so nothing will be remembered: {error}");
                inner.error = Some(error.to_string());
            }
        }
    }

    /// Starts the indexer, the covers and the full text, and passes what they report on to the page.
    fn start_keeping(
        &self,
        library: &Arc<Library>,
    ) -> (
        Option<Indexer>,
        Option<CoverService>,
        Option<FullTextService>,
    ) {
        let Some(app) = self.app.get().cloned() else {
            return (None, None, None);
        };
        let covers = CoverService::start(Arc::clone(library), app.clone());
        let fulltext = FullTextService::start(Arc::clone(library), app.clone());
        let waker = covers.waker();
        let text_waker = fulltext.waker();
        let (sender, events) = mpsc::channel();
        let indexer =
            match Indexer::start(Arc::clone(library), IndexerConfig::default(), Some(sender)) {
                Ok(indexer) => Some(indexer),
                Err(error) => {
                    eprintln!("the library will not follow its folders: {error}");
                    None
                }
            };
        let forwarded = thread::Builder::new()
            .name("library-events".to_owned())
            .spawn(move || forward(&events, &app, &waker, &text_waker));
        if let Err(error) = forwarded {
            eprintln!("the page will not hear about the library: {error}");
        }
        (indexer, Some(covers), Some(fulltext))
    }

    fn fulltext_waker(&self) -> Option<FullTextWaker> {
        self.lock()
            .opened
            .as_ref()?
            .fulltext
            .as_ref()
            .map(FullTextService::waker)
    }

    /// What the full-text indexer is doing.
    pub fn fulltext_progress(&self) -> Progress {
        self.fulltext_waker()
            .map(|waker| waker.progress())
            .unwrap_or_default()
    }

    /// Has the full-text indexer look for books to read now.
    pub fn wake_fulltext(&self) {
        if let Some(waker) = self.fulltext_waker() {
            waker.wake();
        }
    }

    /// Tells the full-text indexer whether a book is open in the reader, so it gives way.
    pub fn set_reading(&self, reading: bool) {
        if let Some(waker) = self.fulltext_waker() {
            waker.set_reading(reading);
        }
    }

    /// Has the indexer look at every folder now.
    pub fn reconcile_now(&self) {
        if let Some(indexer) = self
            .lock()
            .opened
            .as_ref()
            .and_then(|opened| opened.indexer.as_ref())
        {
            indexer.reconcile_now();
        }
    }

    /// Waits until the indexer has nothing left to do, at most `timeout`. True if it is idle.
    pub fn wait_until_idle(&self, timeout: Duration) -> bool {
        // Not holding the lock while waiting: the indexer's events reach the page meanwhile.
        let indexer_idle = {
            let inner = self.lock();
            match inner
                .opened
                .as_ref()
                .and_then(|opened| opened.indexer.as_ref())
            {
                Some(indexer) => indexer.wait_until_idle(Duration::ZERO),
                None => return true,
            }
        };
        if indexer_idle {
            return true;
        }
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
            let inner = self.lock();
            if inner
                .opened
                .as_ref()
                .and_then(|opened| opened.indexer.as_ref())
                .is_none_or(|indexer| indexer.wait_until_idle(Duration::ZERO))
            {
                return true;
            }
        }
        false
    }

    /// Opens the library a new folder goes into, creating it if needed, and starts following the
    /// folder.
    pub fn add_root(&self, path: &Path) -> Result<(), String> {
        let mut inner = self.lock();
        inner.looked = true;
        if inner.opened.is_none() {
            let paths = self.new_paths()?;
            self.open_into(&mut inner, paths);
        }
        let opened = inner.opened.as_ref().ok_or_else(|| {
            inner
                .error
                .clone()
                .unwrap_or_else(|| "there is no library".to_owned())
        })?;
        opened
            .library
            .add_root(path, &[])
            .map_err(|error| error.to_string())?;
        if let Some(indexer) = &opened.indexer {
            indexer.reconcile_now();
        }
        if let Some(covers) = &opened.covers {
            covers.waker().wake();
        }
        Ok(())
    }

    /// Opens the library if needed, validates and adds each folder in `paths` independently, and
    /// returns the updated library state together with which folders were added or refused.
    pub fn add_roots(&self, paths: &[PathBuf]) -> Result<AddRootsResult, String> {
        let mut added = Vec::new();
        let mut rejected = Vec::new();
        {
            let mut inner = self.lock();
            inner.looked = true;
            if inner.opened.is_none() {
                let library_paths = self.new_paths()?;
                self.open_into(&mut inner, library_paths);
            }
            let opened = inner.opened.as_ref().ok_or_else(|| {
                inner
                    .error
                    .clone()
                    .unwrap_or_else(|| "there is no library".to_owned())
            })?;
            for path in paths {
                match opened.library.add_root(path, &[]) {
                    Ok(root) => {
                        let kept = root.path.to_string_lossy().into_owned();
                        if !added.contains(&kept) {
                            added.push(kept);
                        }
                    }
                    Err(error) => rejected.push(RejectedFolder {
                        path: path.to_string_lossy().into_owned(),
                        reason: error.to_string(),
                    }),
                }
            }
            if !added.is_empty() {
                if let Some(indexer) = &opened.indexer {
                    indexer.reconcile_now();
                }
                if let Some(covers) = &opened.covers {
                    covers.waker().wake();
                }
            }
        }
        Ok(AddRootsResult {
            info: self.info(),
            added,
            rejected,
        })
    }

    /// Stops following a root folder without touching its files on disk.
    pub fn remove_root(&self, id: i64) -> Result<(), String> {
        let library = self
            .library()
            .ok_or_else(|| "there is no library open".to_owned())?;
        library.remove_root(id).map_err(|error| error.to_string())
    }

    /// Closes the open library and opens the one in `directory`, which has to exist.
    pub fn switch_to(&self, directory: &Path) -> Result<(), String> {
        let paths = LibraryPaths::in_directory(directory);
        if !paths.library_db().exists() {
            return Err(format!("{} holds no library", directory.display()));
        }
        let mut inner = self.lock();
        inner.looked = true;
        // The old library's indexer and covers stop before anything else opens.
        inner.opened = None;
        self.open_into(&mut inner, paths);
        match &inner.opened {
            Some(_) => Ok(()),
            None => Err(inner.error.clone().unwrap_or_default()),
        }
    }

    /// What the page shows about the library.
    pub fn info(&self) -> LibraryInfo {
        let library = self.library();
        let inner = self.lock();
        let Some(opened) = &inner.opened else {
            return LibraryInfo {
                open: false,
                location: None,
                roots: Vec::new(),
                books: 0,
                indexing: false,
                error: inner.error.clone(),
            };
        };
        let roots = opened
            .library
            .roots()
            .unwrap_or_default()
            .into_iter()
            .filter(|root| root.status != "removed")
            .map(|root| RootSummary {
                id: root.id,
                path: root.path.to_string_lossy().into_owned(),
                status: root.status,
            })
            .collect();
        let books = library
            .and_then(|library| {
                library
                    .browse(&BrowseQuery {
                        limit: 0,
                        ..BrowseQuery::default()
                    })
                    .ok()
            })
            .map_or(0, |page| page.total as u64);
        LibraryInfo {
            open: true,
            location: Some(opened.location.to_string_lossy().into_owned()),
            roots,
            books,
            indexing: opened
                .indexer
                .as_ref()
                .is_some_and(|indexer| !indexer.wait_until_idle(Duration::ZERO)),
            error: None,
        }
    }

    /// Stops everything, for when the app exits.
    pub fn close(&self) {
        self.lock().opened = None;
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LibraryEvent {
    kind: &'static str,
}

fn forward(
    events: &mpsc::Receiver<IndexerEvent>,
    app: &AppHandle,
    covers: &CoverWaker,
    text: &FullTextWaker,
) {
    while let Ok(event) = events.recv() {
        let kind = match &event {
            IndexerEvent::Reconciled(_) => "reconciled",
            IndexerEvent::ContentProcessed(_) => "content",
            IndexerEvent::WatchOverflowed(_) => "overflowed",
            IndexerEvent::WatchLost(..) => "watch-lost",
            IndexerEvent::Failed(message) => {
                eprintln!("the library: {message}");
                "failed"
            }
        };
        if matches!(
            event,
            IndexerEvent::ContentProcessed(_) | IndexerEvent::Reconciled(_)
        ) {
            covers.wake();
            text.wake();
        }
        let _ = app.emit(LIBRARY_CHANGED, LibraryEvent { kind });
    }
}

/// A folder the library follows.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RootSummary {
    id: i64,
    path: String,
    status: String,
}

/// A folder refused when adding one or more roots.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RejectedFolder {
    pub path: String,
    pub reason: String,
}

/// Outcome of adding one or more folders chosen in the native dialog.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddRootsResult {
    pub info: LibraryInfo,
    pub added: Vec<String>,
    pub rejected: Vec<RejectedFolder>,
}

/// What the page shows about the library.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryInfo {
    open: bool,
    location: Option<String>,
    roots: Vec<RootSummary>,
    books: u64,
    /// Whether it is scanning or reading files right now.
    indexing: bool,
    error: Option<String>,
}

/// What the page asks the library for.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowseArgs {
    #[serde(default)]
    text: String,
    folder: Option<i64>,
    #[serde(default)]
    category: Option<i64>,
    #[serde(default)]
    tag: Option<i64>,
    #[serde(default)]
    collection: Option<i64>,
    #[serde(default)]
    smart: Option<i64>,
    #[serde(default)]
    works: Option<Vec<i64>>,
    #[serde(default)]
    offset: usize,
    limit: usize,
}

pub fn owner_of(window: &WebviewWindow) -> Option<isize> {
    window.hwnd().ok().map(|hwnd| hwnd.0 as isize)
}

fn library_of(state: &AppState) -> Result<Arc<Library>, String> {
    state
        .shelf
        .library()
        .ok_or_else(|| "there is no library open".to_owned())
}

/// What the page shows about the library.
#[tauri::command(async)]
pub fn library_info(state: State<'_, AppState>) -> LibraryInfo {
    state.shelf.info()
}

/// Asks for one or more folders of books and starts following them, creating the library if there is none.
#[tauri::command(async)]
pub fn library_choose_root(
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Option<AddRootsResult>, String> {
    let folders = pick_folders(owner_of(&window), &tr!(PickBooksFolder))?;
    if folders.is_empty() {
        return Ok(None);
    }
    state.shelf.add_roots(&folders).map(Some)
}

/// Stops following a root folder without touching files on disk.
#[tauri::command(async)]
pub fn library_remove_root(state: State<'_, AppState>, id: i64) -> Result<LibraryInfo, String> {
    state.shelf.remove_root(id)?;
    Ok(state.shelf.info())
}

/// Starts following a folder named by path. The same as choosing it, for tests that cannot use the
/// system's dialog, and only for them: a person chooses folders in the dialog, and a page that
/// names one is not trusted to.
#[tauri::command(async)]
pub fn library_add_root(state: State<'_, AppState>, path: String) -> Result<LibraryInfo, String> {
    if !state.is_e2e() {
        return Err("a folder is only given by path under a test".to_owned());
    }
    state.shelf.add_root(Path::new(&path))?;
    Ok(state.shelf.info())
}

/// Asks for the folder of a library made before, and opens it instead of the current one.
#[tauri::command(async)]
pub fn library_choose_existing(
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Option<LibraryInfo>, String> {
    let Some(folder) = pick_folder(owner_of(&window), &tr!(PickLibraryFolder))? else {
        return Ok(None);
    };
    state.stop_current_session();
    state.shelf.switch_to(&folder)?;
    Ok(Some(state.shelf.info()))
}

/// A page of books.
#[tauri::command(async)]
pub fn library_browse(state: State<'_, AppState>, query: BrowseArgs) -> Result<BrowsePage, String> {
    let library = library_of(&state)?;
    library
        .browse(&BrowseQuery {
            text: query.text,
            folder: query.folder,
            category: query.category,
            tag: query.tag,
            collection: query.collection,
            smart: query.smart,
            works: query.works,
            offset: query.offset,
            limit: query.limit.min(MAX_BROWSE),
        })
        .map_err(|error| error.to_string())
}

/// A book of a listing, as a selection holds it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectedBook {
    document_id: i64,
    work_id: i64,
}

/// Every book a listing has, not only the ones read so far: what selecting all selects.
#[tauri::command(async)]
pub fn library_select_all(
    state: State<'_, AppState>,
    query: BrowseArgs,
) -> Result<Vec<SelectedBook>, String> {
    let library = library_of(&state)?;
    let mut selected = Vec::new();
    loop {
        let page = library
            .browse(&BrowseQuery {
                text: query.text.clone(),
                folder: query.folder,
                category: query.category,
                tag: query.tag,
                collection: query.collection,
                smart: query.smart,
                works: query.works.clone(),
                offset: selected.len(),
                limit: MAX_BROWSE,
            })
            .map_err(|error| error.to_string())?;
        let got = page.rows.len();
        selected.extend(page.rows.into_iter().map(|row| SelectedBook {
            document_id: row.document_id,
            work_id: row.work_id,
        }));
        if got == 0 || selected.len() >= page.total || selected.len() >= musaeum_library::MAX_BATCH
        {
            break;
        }
    }
    selected.truncate(musaeum_library::MAX_BATCH);
    Ok(selected)
}

/// The folders of every root.
#[tauri::command(async)]
pub fn library_folders(state: State<'_, AppState>) -> Result<Vec<FolderEntry>, String> {
    library_of(&state)?
        .folders()
        .map_err(|error| error.to_string())
}

/// Names a setting may have: short, lowercase words joined by dashes.
fn is_setting_name(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 40
        && key
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
}

/// A setting of the reader's, if it was ever saved.
#[tauri::command(async)]
pub fn reader_setting(
    state: State<'_, AppState>,
    key: String,
) -> Result<Option<serde_json::Value>, String> {
    if !is_setting_name(&key) {
        return Err(format!("{key} is not a setting"));
    }
    let Some(library) = state.shelf.library() else {
        return Ok(None);
    };
    Ok(library
        .preference(&key)
        .map_err(|error| error.to_string())?
        .and_then(|text| serde_json::from_str(&text).ok()))
}

/// Keeps a setting of the reader's. Without a library, it lasts until the app closes.
#[tauri::command(async)]
pub fn set_reader_setting(
    state: State<'_, AppState>,
    key: String,
    value: serde_json::Value,
) -> Result<(), String> {
    if !is_setting_name(&key) {
        return Err(format!("{key} is not a setting"));
    }
    let Some(library) = state.shelf.library() else {
        return Ok(());
    };
    library
        .set_preference(&key, &value.to_string())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("musaeum-shelf-{}-{id}", std::process::id()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn only_plain_names_are_settings() {
        assert!(is_setting_name("typography"));
        assert!(is_setting_name("flow-mode"));
        assert!(!is_setting_name(""));
        assert!(!is_setting_name("Typography"));
        assert!(!is_setting_name("a.b"));
        assert!(!is_setting_name(&"a".repeat(41)));
    }

    #[test]
    fn add_roots_keeps_valid_folders_and_reports_refused_ones() {
        let dir = TempDir::new();
        let lib_dir = dir.0.join("lib");
        let valid_a = dir.0.join("books-a");
        let child_of_a = valid_a.join("sub");
        let valid_b = dir.0.join("books-b");
        std::fs::create_dir_all(&lib_dir).unwrap();
        std::fs::create_dir_all(&child_of_a).unwrap();
        std::fs::create_dir_all(&valid_b).unwrap();

        let shelf = Shelf::new(Some(lib_dir.clone()), true);
        let outcome = shelf
            .add_roots(&[
                valid_a.clone(),
                child_of_a.clone(),
                lib_dir.clone(),
                valid_b.clone(),
            ])
            .unwrap();

        assert_eq!(outcome.added.len(), 2);
        assert_eq!(outcome.rejected.len(), 2);
        assert_eq!(outcome.info.roots.len(), 2);
        assert!(
            outcome
                .rejected
                .iter()
                .any(|item| item.path == child_of_a.to_string_lossy())
        );
        assert!(
            outcome
                .rejected
                .iter()
                .any(|item| item.path == lib_dir.to_string_lossy())
        );

        let root_id = outcome.info.roots[0].id;
        shelf.remove_root(root_id).unwrap();
        assert_eq!(shelf.info().roots.len(), 1);
    }
}

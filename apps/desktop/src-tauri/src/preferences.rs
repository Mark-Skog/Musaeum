//! The person's preferences, as the app keeps and applies them: the interface's language for what
//! the Rust side writes, the name annotations are signed with, the OCR's languages, and the cache.
//!
//! They live in `preferences.db`: beside the library given with `--library` (so a test has its
//! own), or else in `%LOCALAPPDATA%\Musaeum`, never in a folder of books. A change is checked,
//! kept, applied here, and told to the page (`preferences-changed`), which applies the rest: the
//! words of the interface, its colors and its motion.
//!
//! | Command | Answer |
//! |---------|--------|
//! | `preferences_get` | every preference, the system's user name, where they are kept |
//! | `preferences_set` | changes some, and answers as `preferences_get` |
//! | `ocr_models` | the OCR's models in the program's folder, and which are verified |
//! | `cache_report` | what the cache holds, and how much of the disk |
//! | `cache_clear_lines` | forgets the pages' lines and what the OCR read on them |

use std::path::{Path, PathBuf};
use std::sync::{PoisonError, RwLock};

use musaeum_i18n::{set_locale, tr};
use musaeum_library::{CacheReport, Preferences, PreferencesPatch, PreferencesStore, Theme};
use musaeum_ocr::{OcrClient, OcrConfig};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::session::AppState;

/// Sent to the page whenever a preference changed.
pub const PREFERENCES_CHANGED: &str = "preferences-changed";

/// The preferences, kept and current.
pub struct PreferencesState {
    store: Option<PreferencesStore>,
    current: RwLock<Preferences>,
    /// Why the file could not be read, if it could not: every preference is then its default, and
    /// nothing is written over the file.
    problem: Option<String>,
}

impl PreferencesState {
    /// The preferences kept for this run: beside the library named on the command line, or in the
    /// usual place. Sets the language of what the Rust side writes.
    pub fn open(named_library: Option<&Path>) -> Self {
        let (store, problem) = match preferences_path(named_library) {
            Some(path) => match PreferencesStore::open(&path) {
                Ok(store) => (Some(store), None),
                Err(error) => (None, Some(format!("{}: {error}", path.display()))),
            },
            None => (None, Some("LOCALAPPDATA is not set".to_owned())),
        };
        let current = match store.as_ref().map(PreferencesStore::load) {
            Some(Ok(preferences)) => preferences,
            Some(Err(error)) => {
                eprintln!("the preferences could not be read: {error}");
                Preferences::default()
            }
            None => Preferences::default(),
        };
        if let Some(problem) = &problem {
            eprintln!("the preferences are the defaults: {problem}");
        }
        set_locale(current.locale());
        Self {
            store,
            current: RwLock::new(current),
            problem,
        }
    }

    /// Every preference as it is now.
    pub fn current(&self) -> Preferences {
        self.current
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The name annotations are signed with: the one chosen, or the system's user name.
    pub fn author(&self) -> Option<String> {
        let chosen = self.current().annotation_author;
        if chosen.is_empty() {
            system_user_name()
        } else {
            Some(chosen)
        }
    }

    /// The name chosen for annotations, if one was: a report names it only then.
    pub fn chosen_author(&self) -> Option<String> {
        Some(self.current().annotation_author).filter(|name| !name.is_empty())
    }

    fn update(&self, patch: &PreferencesPatch) -> Result<Preferences, String> {
        let store = self.store.as_ref().ok_or_else(|| {
            tr!(
                PreferencesNotSaved,
                error = self.problem.as_deref().unwrap_or_default()
            )
        })?;
        let updated = store.update(patch).map_err(|error| error.to_string())?;
        set_locale(updated.locale());
        *self.current.write().unwrap_or_else(PoisonError::into_inner) = updated.clone();
        Ok(updated)
    }
}

/// The window's own theme, for its title bar and frame: the system's unless one was chosen.
pub fn window_theme(theme: Theme) -> Option<tauri::Theme> {
    match theme {
        Theme::System => None,
        Theme::Light => Some(tauri::Theme::Light),
        Theme::Dark => Some(tauri::Theme::Dark),
    }
}

/// Where the preferences are kept: beside the library given on the command line, or in
/// `%LOCALAPPDATA%\Musaeum`.
pub fn preferences_path(named_library: Option<&Path>) -> Option<PathBuf> {
    match named_library {
        Some(directory) => Some(directory.join("preferences.db")),
        None => std::env::var_os("LOCALAPPDATA")
            .map(|base| PathBuf::from(base).join("Musaeum").join("preferences.db")),
    }
}

/// The system's name for the person using it.
pub fn system_user_name() -> Option<String> {
    std::env::var("USERNAME")
        .ok()
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreferencesView {
    preferences: Preferences,
    /// What annotations are signed with when no name is chosen.
    system_author: Option<String>,
    /// Where they are kept.
    path: Option<String>,
    /// Why they could not be read or kept, if so.
    problem: Option<String>,
}

fn view(state: &AppState) -> PreferencesView {
    let preferences = &state.preferences;
    PreferencesView {
        preferences: preferences.current(),
        system_author: system_user_name(),
        path: preferences
            .store
            .as_ref()
            .map(|store| store.path().to_string_lossy().into_owned()),
        problem: preferences.problem.clone(),
    }
}

/// Every preference.
#[tauri::command]
pub fn preferences_get(state: State<'_, AppState>) -> PreferencesView {
    view(&state)
}

/// Changes the preferences given, all or none, applies them, and tells the page.
#[tauri::command(async)]
pub fn preferences_set(
    app: AppHandle,
    state: State<'_, AppState>,
    patch: PreferencesPatch,
) -> Result<PreferencesView, String> {
    if let Some(languages) = &patch.ocr_languages {
        let trusted = state.ocr.trusted_languages(state.ocr_policy());
        if let Some(unknown) = languages.iter().find(|code| !trusted.contains(code)) {
            return Err(tr!(PreferenceOcrUnknown, code = unknown));
        }
    }
    let updated = state.preferences.update(&patch)?;
    if patch.ocr_languages.is_some() {
        state.ocr.choose_languages(&updated.ocr_languages);
    }
    if patch.theme.is_some()
        && let Some(window) = app.get_webview_window("main")
    {
        // The title bar follows the page's colors at once.
        let _ = window.set_theme(window_theme(updated.theme));
    }
    let answer = view(&state);
    let _ = app.emit(PREFERENCES_CHANGED, &answer);
    Ok(answer)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrModelView {
    /// Tesseract's name (`por`).
    code: String,
    /// The language as BCP 47 names it, for the interface to name it, if the model is known.
    bcp47: Option<String>,
    bytes: u64,
    /// Whether its bytes are the ones pinned for it, so that it can be used.
    verified: bool,
    /// Whether it is one of those shipped.
    shipped: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrModels {
    /// The folder they are read from, next to the program.
    folder: Option<String>,
    models: Vec<OcrModelView>,
    /// The models the program would trust but does not have, which `tools/fetch-ocr.ps1` fetches.
    fetchable: Vec<OcrModelView>,
    /// The languages read now, as Tesseract names them (`por+eng`).
    reading: Option<String>,
}

/// The OCR's models: those in the program's folder, verified or not, and those it could fetch.
#[tauri::command(async)]
pub fn ocr_models(state: State<'_, AppState>) -> OcrModels {
    let client = OcrClient::new(OcrConfig {
        policy: state.ocr_policy(),
        ..OcrConfig::default()
    });
    let found = client.models();
    let models: Vec<OcrModelView> = found
        .iter()
        .map(|model| OcrModelView {
            code: model.code.clone(),
            bcp47: model.known.map(|known| known.bcp47.to_owned()),
            bytes: model.bytes,
            verified: model.verified,
            shipped: model.known.is_some_and(|known| known.shipped),
        })
        .collect();
    let fetchable = musaeum_ocr::KNOWN_MODELS
        .iter()
        .filter(|known| !found.iter().any(|model| model.code == known.code))
        .map(|known| OcrModelView {
            code: known.code.to_owned(),
            bcp47: Some(known.bcp47.to_owned()),
            bytes: 0,
            verified: false,
            shipped: known.shipped,
        })
        .collect();
    OcrModels {
        folder: client
            .models_folder()
            .map(|folder| folder.to_string_lossy().into_owned()),
        models,
        fetchable,
        reading: state.ocr.reading(state.ocr_policy()),
    }
}

/// What the open library's cache holds.
#[tauri::command(async)]
pub fn cache_report(state: State<'_, AppState>) -> Result<Option<CacheReport>, String> {
    let Some(library) = state.library() else {
        return Ok(None);
    };
    library
        .cache_report()
        .map(Some)
        .map_err(|error| error.to_string())
}

/// Forgets the pages' lines and what the OCR read on them: both are found again when needed. The
/// library, its annotations and their quotes are not touched.
#[tauri::command(async)]
pub fn cache_clear_lines(state: State<'_, AppState>) -> Result<Option<CacheReport>, String> {
    let Some(library) = state.library() else {
        return Ok(None);
    };
    library
        .clear_lines_and_ocr()
        .map_err(|error| error.to_string())?;
    library
        .cache_report()
        .map(Some)
        .map_err(|error| error.to_string())
}

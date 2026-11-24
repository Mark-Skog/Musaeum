//! The person's preferences: the app's own, not a library's, in `preferences.db` beside the
//! libraries (never in a folder of books).
//!
//! Each preference is a row whose value is JSON. Reading never fails on what is there: a value this
//! version cannot read, or one out of bounds, gives the default, and a preference it does not know
//! (written by a newer version) stays untouched. Writing checks every value first and writes them
//! all in one transaction, or none.
//!
//! The file exists only once something was chosen: until then every preference is its default, and
//! nothing is written anywhere.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use musaeum_i18n::{Locale, tr};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::db::migrate::{self, Migration};
use crate::error::{Error, Result, io_error};
use crate::sql::now_ms;

/// `application_id` of `preferences.db`: "MUSP".
const PREFERENCES_APPLICATION_ID: i32 = 0x4D55_5350;

const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    sql: include_str!("../migrations/preferences/0001_initial.sql"),
}];

/// The longest name an annotation may be signed with, in characters.
pub const MAX_AUTHOR_CHARS: usize = 128;
/// The most languages the OCR reads at once: each one is a model it loads and tries.
pub const MAX_OCR_LANGUAGES: usize = 4;
/// What the OCR reads unless the person chose otherwise: the models shipped.
pub const DEFAULT_OCR_LANGUAGES: [&str; 2] = ["por", "eng"];

/// The interface's colors.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// Light or dark as the system is.
    #[default]
    System,
    Light,
    Dark,
}

/// How much the interface moves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Motion {
    /// As the system says (`prefers-reduced-motion`).
    #[default]
    System,
    /// Always as little as possible: no page-turn animation, no transitions.
    Reduce,
}

/// Every preference, each with a value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preferences {
    /// The interface's language, as a BCP 47 tag of one it speaks.
    pub language: String,
    pub theme: Theme,
    pub motion: Motion,
    /// The name annotations are signed with in a PDF, an XFDF file or a report; empty for the
    /// system's user name.
    pub annotation_author: String,
    /// The OCR's languages, as Tesseract names them, in the order it tries them.
    pub ocr_languages: Vec<String>,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            language: Locale::default().tag().to_owned(),
            theme: Theme::default(),
            motion: Motion::default(),
            annotation_author: String::new(),
            ocr_languages: DEFAULT_OCR_LANGUAGES.map(str::to_owned).to_vec(),
        }
    }
}

impl Preferences {
    /// The interface's language.
    pub fn locale(&self) -> Locale {
        Locale::from_tag(&self.language).unwrap_or_default()
    }
}

/// What to change: each preference given is checked and kept; the others stay as they are.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreferencesPatch {
    pub language: Option<String>,
    pub theme: Option<String>,
    pub motion: Option<String>,
    pub annotation_author: Option<String>,
    pub ocr_languages: Option<Vec<String>>,
}

/// A preference's value as it is kept, checked.
fn checked(patch: &PreferencesPatch) -> Result<Vec<(&'static str, Value)>> {
    let mut rows = Vec::new();
    if let Some(language) = &patch.language {
        let locale = Locale::from_tag(language)
            .ok_or_else(|| Error::Refused(tr!(PreferenceLanguageUnknown, value = language)))?;
        rows.push(("language", Value::from(locale.tag())));
    }
    if let Some(theme) = &patch.theme {
        let theme: Theme = serde_json::from_value(Value::from(theme.as_str()))
            .map_err(|_| Error::Refused(tr!(PreferenceThemeUnknown, value = theme)))?;
        rows.push(("theme", serde_json::to_value(theme)?));
    }
    if let Some(motion) = &patch.motion {
        let motion: Motion = serde_json::from_value(Value::from(motion.as_str()))
            .map_err(|_| Error::Refused(tr!(PreferenceMotionUnknown, value = motion)))?;
        rows.push(("motion", serde_json::to_value(motion)?));
    }
    if let Some(author) = &patch.annotation_author {
        rows.push(("annotationAuthor", Value::from(checked_author(author)?)));
    }
    if let Some(languages) = &patch.ocr_languages {
        rows.push((
            "ocrLanguages",
            Value::from(checked_ocr_languages(languages)?),
        ));
    }
    Ok(rows)
}

/// An author's name, trimmed: at most [`MAX_AUTHOR_CHARS`], no control characters.
pub fn checked_author(author: &str) -> Result<String> {
    let author = author.trim();
    if author.chars().count() > MAX_AUTHOR_CHARS {
        return Err(Error::Refused(tr!(
            PreferenceAuthorTooLong,
            max = MAX_AUTHOR_CHARS
        )));
    }
    if author.chars().any(char::is_control) {
        return Err(Error::Refused(tr!(PreferenceAuthorControl)));
    }
    Ok(author.to_owned())
}

/// Whether `code` looks like a Tesseract model's name (`por`, `chi_sim`), which is all this crate
/// can tell: which models there are, and whether they are the ones verified, the host knows.
pub fn is_ocr_language_code(code: &str) -> bool {
    let mut parts = code.split('_');
    let first = parts.next().unwrap_or_default();
    (3..=8).contains(&first.len())
        && first.bytes().all(|byte| byte.is_ascii_lowercase())
        && parts.all(|part| {
            (1..=8).contains(&part.len()) && part.bytes().all(|byte| byte.is_ascii_lowercase())
        })
}

/// The OCR's languages: one to [`MAX_OCR_LANGUAGES`] model names, each once, in the order given.
pub fn checked_ocr_languages(languages: &[String]) -> Result<Vec<String>> {
    let mut kept: Vec<String> = Vec::new();
    for code in languages {
        if !is_ocr_language_code(code) {
            return Err(Error::Refused(tr!(PreferenceOcrUnknown, code = code)));
        }
        if !kept.contains(code) {
            kept.push(code.clone());
        }
    }
    if kept.is_empty() {
        return Err(Error::Refused(tr!(PreferenceOcrNone)));
    }
    if kept.len() > MAX_OCR_LANGUAGES {
        return Err(Error::Refused(tr!(
            PreferenceOcrTooMany,
            max = MAX_OCR_LANGUAGES
        )));
    }
    Ok(kept)
}

/// The preferences' file, read and written by one connection at a time.
pub struct PreferencesStore {
    path: PathBuf,
    connection: Mutex<Option<Connection>>,
}

impl std::fmt::Debug for PreferencesStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreferencesStore")
            .field("path", &self.path)
            .finish()
    }
}

impl PreferencesStore {
    /// The preferences at `path`. Nothing is created until something is chosen; a file there that
    /// is not ours, or is newer than this program, is refused untouched.
    pub fn open(path: &Path) -> Result<Self> {
        let connection = if path.exists() {
            Some(Self::connect(path)?)
        } else {
            None
        };
        Ok(Self {
            path: path.to_owned(),
            connection: Mutex::new(connection),
        })
    }

    fn connect(path: &Path) -> Result<Connection> {
        let mut connection = Connection::open(path)?;
        // A small file written seldom: the classic journal, flushed fully, and no log left beside it.
        connection.pragma_update(None, "journal_mode", "DELETE")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        migrate::check_owner(&connection, PREFERENCES_APPLICATION_ID, path)?;
        migrate::apply(&mut connection, PREFERENCES_APPLICATION_ID, MIGRATIONS)?;
        Ok(connection)
    }

    /// Where the file is, or would be.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Every preference: what was chosen, and the default for the rest.
    pub fn load(&self) -> Result<Preferences> {
        let guard = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut preferences = Preferences::default();
        let Some(connection) = guard.as_ref() else {
            return Ok(preferences);
        };
        let mut statement = connection.prepare("SELECT key, value FROM preference")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (key, value) = row?;
            let Ok(value) = serde_json::from_str::<Value>(&value) else {
                continue;
            };
            apply_kept(&mut preferences, &key, value);
        }
        Ok(preferences)
    }

    /// Checks every preference in `patch`, keeps them all in one transaction, and returns every
    /// preference as it is now.
    pub fn update(&self, patch: &PreferencesPatch) -> Result<Preferences> {
        let rows = checked(patch)?;
        {
            let mut guard = self
                .connection
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if guard.is_none() {
                if let Some(folder) = self.path.parent() {
                    std::fs::create_dir_all(folder)
                        .map_err(io_error(format!("creating {}", folder.display())))?;
                }
                *guard = Some(Self::connect(&self.path)?);
            }
            let connection = guard.as_mut().ok_or(Error::WriterStopped)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let now = now_ms();
            for (key, value) in &rows {
                transaction.execute(
                    "INSERT INTO preference (key, value, updated_at) VALUES (?1, ?2, ?3)
                     ON CONFLICT (key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                    params![key, value.to_string(), now],
                )?;
            }
            transaction.commit()?;
        }
        self.load()
    }

    /// When a preference was last chosen, if it was.
    pub fn chosen_at(&self, key: &str) -> Result<Option<i64>> {
        let guard = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(connection) = guard.as_ref() else {
            return Ok(None);
        };
        Ok(connection
            .query_row(
                "SELECT updated_at FROM preference WHERE key = ?1",
                [key],
                |row| row.get(0),
            )
            .optional()?)
    }
}

/// Puts a kept value into `preferences`, if it is one this version can read and within bounds.
fn apply_kept(preferences: &mut Preferences, key: &str, value: Value) {
    match key {
        "language" => {
            if let Some(locale) = value.as_str().and_then(Locale::from_tag) {
                preferences.language = locale.tag().to_owned();
            }
        }
        "theme" => {
            if let Ok(theme) = serde_json::from_value(value) {
                preferences.theme = theme;
            }
        }
        "motion" => {
            if let Ok(motion) = serde_json::from_value(value) {
                preferences.motion = motion;
            }
        }
        "annotationAuthor" => {
            if let Some(author) = value
                .as_str()
                .and_then(|author| checked_author(author).ok())
            {
                preferences.annotation_author = author;
            }
        }
        "ocrLanguages" => {
            if let Ok(languages) = serde_json::from_value::<Vec<String>>(value)
                && let Ok(languages) = checked_ocr_languages(&languages)
            {
                preferences.ocr_languages = languages;
            }
        }
        // A newer version's: kept as it is, and not read.
        _ => {}
    }
}

//! The person's preferences: their defaults, what is kept, and what is refused.

mod common;

use common::TempDir;
use musaeum_i18n::{Locale, set_locale};
use musaeum_library::{
    COVER_DEFAULT_WIDTH, COVER_MAX_WIDTH, COVER_MIN_WIDTH, Error, Motion, PreferencesPatch,
    PreferencesStore, SIDEBAR_DEFAULT_WIDTH, SIDEBAR_MAX_WIDTH, SIDEBAR_MIN_WIDTH, Theme,
    clamp_cover_width, clamp_sidebar_width,
};

fn patch() -> PreferencesPatch {
    PreferencesPatch::default()
}

#[test]
fn nothing_chosen_is_every_default_and_writes_nothing() {
    let dir = TempDir::new();
    let path = dir.0.join("preferences.db");
    let store = PreferencesStore::open(&path).unwrap();
    let preferences = store.load().unwrap();
    assert_eq!(preferences.language, "pt-BR");
    assert_eq!(preferences.theme, Theme::System);
    assert_eq!(preferences.motion, Motion::System);
    assert_eq!(preferences.annotation_author, "");
    assert_eq!(preferences.ocr_languages, ["por", "eng"]);
    assert_eq!(preferences.sidebar_width, SIDEBAR_DEFAULT_WIDTH);
    assert!(!preferences.sidebar_collapsed);
    assert!(!preferences.show_empty_sidebar_sections);
    assert_eq!(preferences.cover_width, COVER_DEFAULT_WIDTH);
    assert!(!preferences.show_sidebar_counts);
    assert!(!path.exists(), "reading wrote nothing");
}

#[test]
fn what_is_chosen_is_kept_and_read_again_after_reopening() {
    let dir = TempDir::new();
    let path = dir.0.join("sub").join("preferences.db");
    let store = PreferencesStore::open(&path).unwrap();
    let after = store
        .update(&PreferencesPatch {
            language: Some("en".to_owned()),
            theme: Some("dark".to_owned()),
            motion: Some("reduce".to_owned()),
            annotation_author: Some("  Ana Leitora  ".to_owned()),
            ocr_languages: Some(vec!["eng".to_owned(), "por".to_owned(), "eng".to_owned()]),
            sidebar_width: Some(312),
            sidebar_collapsed: Some(true),
            show_empty_sidebar_sections: Some(true),
            cover_width: Some(216),
            show_sidebar_counts: Some(true),
        })
        .unwrap();
    assert_eq!(after.language, "en");
    assert_eq!(after.locale(), Locale::English);
    assert_eq!(after.annotation_author, "Ana Leitora", "trimmed");
    assert_eq!(after.ocr_languages, ["eng", "por"], "each once, in order");
    assert_eq!(after.sidebar_width, 312);
    assert!(after.sidebar_collapsed);
    assert!(after.show_empty_sidebar_sections);
    drop(store);

    let again = PreferencesStore::open(&path).unwrap().load().unwrap();
    assert_eq!(again, after);
    assert!(again.theme == Theme::Dark && again.motion == Motion::Reduce);

    // One preference changes; the others stay.
    let store = PreferencesStore::open(&path).unwrap();
    let changed = store
        .update(&PreferencesPatch {
            theme: Some("light".to_owned()),
            ..patch()
        })
        .unwrap();
    assert_eq!(changed.theme, Theme::Light);
    assert_eq!(changed.annotation_author, "Ana Leitora");
    assert!(store.chosen_at("theme").unwrap().is_some());
    assert!(store.chosen_at("nothing").unwrap().is_none());
}

#[test]
fn a_bad_value_is_refused_and_nothing_of_its_patch_is_kept() {
    let dir = TempDir::new();
    let path = dir.0.join("preferences.db");
    let store = PreferencesStore::open(&path).unwrap();
    store
        .update(&PreferencesPatch {
            annotation_author: Some("Ana".to_owned()),
            ..patch()
        })
        .unwrap();
    let bad = [
        PreferencesPatch {
            language: Some("es".to_owned()),
            ..patch()
        },
        PreferencesPatch {
            theme: Some("sepia".to_owned()),
            ..patch()
        },
        PreferencesPatch {
            motion: Some("fast".to_owned()),
            ..patch()
        },
        PreferencesPatch {
            annotation_author: Some("a\u{7}b".to_owned()),
            ..patch()
        },
        PreferencesPatch {
            annotation_author: Some("x".repeat(129)),
            ..patch()
        },
        PreferencesPatch {
            ocr_languages: Some(Vec::new()),
            ..patch()
        },
        PreferencesPatch {
            ocr_languages: Some(vec!["POR".to_owned()]),
            ..patch()
        },
        PreferencesPatch {
            ocr_languages: Some(vec!["../x".to_owned()]),
            ..patch()
        },
        PreferencesPatch {
            ocr_languages: Some(
                ["por", "eng", "spa", "fra", "deu"]
                    .map(str::to_owned)
                    .to_vec(),
            ),
            ..patch()
        },
        // A good value beside a bad one: neither is kept.
        PreferencesPatch {
            theme: Some("dark".to_owned()),
            language: Some("xx".to_owned()),
            ..patch()
        },
    ];
    for refused in bad {
        assert!(
            matches!(store.update(&refused), Err(Error::Refused(_))),
            "{refused:?}"
        );
    }
    let now = store.load().unwrap();
    assert_eq!(now.theme, Theme::System);
    assert_eq!(now.annotation_author, "Ana");
}

#[test]
fn a_refusal_is_written_in_the_language_of_the_interface() {
    let dir = TempDir::new();
    let store = PreferencesStore::open(&dir.0.join("preferences.db")).unwrap();
    let refused = |store: &PreferencesStore| match store.update(&PreferencesPatch {
        theme: Some("sepia".to_owned()),
        ..patch()
    }) {
        Err(Error::Refused(reason)) => reason,
        other => panic!("{other:?}"),
    };
    set_locale(Locale::English);
    let english = refused(&store);
    set_locale(Locale::PtBr);
    let portuguese = refused(&store);
    assert_eq!(english, "sepia is not a theme");
    assert_eq!(portuguese, "sepia não é um tema");
}

#[test]
fn what_cannot_be_read_falls_back_and_what_is_unknown_is_left_alone() {
    let dir = TempDir::new();
    let path = dir.0.join("preferences.db");
    let store = PreferencesStore::open(&path).unwrap();
    store
        .update(&PreferencesPatch {
            language: Some("en".to_owned()),
            annotation_author: Some("Ana".to_owned()),
            ..patch()
        })
        .unwrap();
    drop(store);
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                r#"UPDATE preference SET value = '"klingon"' WHERE key = 'language';
                   INSERT INTO preference VALUES ('ocrLanguages', '[]', 0);
                   INSERT INTO preference VALUES ('fromTheFuture', '{"a": 1}', 0);"#,
            )
            .unwrap();
    }
    let store = PreferencesStore::open(&path).unwrap();
    let read = store.load().unwrap();
    assert_eq!(read.language, "pt-BR", "unreadable: the default");
    assert_eq!(
        read.ocr_languages,
        ["por", "eng"],
        "out of bounds: the default"
    );
    assert_eq!(read.annotation_author, "Ana", "the rest as kept");
    store
        .update(&PreferencesPatch {
            theme: Some("dark".to_owned()),
            ..patch()
        })
        .unwrap();
    drop(store);
    let connection = rusqlite::Connection::open(&path).unwrap();
    let future: String = connection
        .query_row(
            "SELECT value FROM preference WHERE key = 'fromTheFuture'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(future, r#"{"a": 1}"#);
}

#[test]
fn a_file_that_is_not_ours_is_refused_untouched() {
    let dir = TempDir::new();
    let path = dir.0.join("preferences.db");
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch("CREATE TABLE theirs (x);")
            .unwrap();
    }
    assert!(matches!(
        PreferencesStore::open(&path),
        Err(Error::NotOurDatabase(_))
    ));
    let connection = rusqlite::Connection::open(&path).unwrap();
    let tables: i64 = connection
        .query_row("SELECT count(*) FROM sqlite_schema", [], |row| row.get(0))
        .unwrap();
    assert_eq!(tables, 1);
}

#[test]
fn sidebar_width_is_clamped_and_corrupted_values_fall_back() {
    assert_eq!(clamp_sidebar_width(50), SIDEBAR_MIN_WIDTH);
    assert_eq!(clamp_sidebar_width(900), SIDEBAR_MAX_WIDTH);
    assert_eq!(clamp_sidebar_width(240), SIDEBAR_DEFAULT_WIDTH);

    let dir = TempDir::new();
    let path = dir.0.join("preferences.db");
    let store = PreferencesStore::open(&path).unwrap();
    let low = store
        .update(&PreferencesPatch {
            sidebar_width: Some(50),
            ..patch()
        })
        .unwrap();
    assert_eq!(low.sidebar_width, SIDEBAR_MIN_WIDTH);
    let high = store
        .update(&PreferencesPatch {
            sidebar_width: Some(900),
            ..patch()
        })
        .unwrap();
    assert_eq!(high.sidebar_width, SIDEBAR_MAX_WIDTH);
    drop(store);

    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                r#"UPDATE preference SET value = '50' WHERE key = 'sidebarWidth';
                   INSERT INTO preference VALUES ('sidebarCollapsed', '"bad"', 0);
                   INSERT INTO preference VALUES ('showEmptySidebarSections', '123', 0);"#,
            )
            .unwrap();
    }
    let clamped = PreferencesStore::open(&path).unwrap().load().unwrap();
    assert_eq!(clamped.sidebar_width, SIDEBAR_MIN_WIDTH);
    assert!(!clamped.sidebar_collapsed);
    assert!(!clamped.show_empty_sidebar_sections);

    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(r#"UPDATE preference SET value = '"bad"' WHERE key = 'sidebarWidth';"#)
            .unwrap();
    }
    let fallback = PreferencesStore::open(&path).unwrap().load().unwrap();
    assert_eq!(fallback.sidebar_width, SIDEBAR_DEFAULT_WIDTH);
}

#[test]
fn cover_width_and_sidebar_counts_clamp_and_fallback() {
    assert_eq!(clamp_cover_width(40), COVER_MIN_WIDTH);
    assert_eq!(clamp_cover_width(999), COVER_MAX_WIDTH);
    assert_eq!(clamp_cover_width(168), COVER_DEFAULT_WIDTH);

    let dir = TempDir::new();
    let path = dir.0.join("preferences.db");
    let store = PreferencesStore::open(&path).unwrap();

    let saved = store
        .update(&PreferencesPatch {
            cover_width: Some(40),
            show_sidebar_counts: Some(true),
            ..patch()
        })
        .unwrap();
    assert_eq!(saved.cover_width, COVER_MIN_WIDTH);
    assert!(saved.show_sidebar_counts);

    let saved_high = store
        .update(&PreferencesPatch {
            cover_width: Some(500),
            ..patch()
        })
        .unwrap();
    assert_eq!(saved_high.cover_width, COVER_MAX_WIDTH);
    drop(store);

    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                r#"UPDATE preference SET value = '"bad"' WHERE key = 'coverWidth';
                   UPDATE preference SET value = '123' WHERE key = 'showSidebarCounts';"#,
            )
            .unwrap();
    }

    let fallback = PreferencesStore::open(&path).unwrap().load().unwrap();
    assert_eq!(fallback.cover_width, COVER_DEFAULT_WIDTH);
    assert!(!fallback.show_sidebar_counts);
}

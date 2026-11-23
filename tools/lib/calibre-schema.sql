-- The tables of a Calibre library's metadata.db that Musaeum reads, as Calibre makes them (names,
-- columns and constraints from Calibre's own schema; its other tables are left out). Both the
-- Rust tests (crates/musaeum-library/tests/beta.rs) and the window pass (tools/lib/calibre-library.mjs)
-- build their synthetic Calibre libraries from this file, so the two cannot drift apart.
--
-- The custom columns' own tables are made by each builder, by column, as Calibre does:
--   normalized:     custom_column_N (id, value, link) and books_custom_column_N_link (id, book, value[, extra])
--   not normalized: custom_column_N (id, book, value)
--
-- After the rows, `calibre-schema-after.sql`'s view and trigger are added: Calibre's call
-- functions only Calibre defines, and a reader that ran them would fail.

CREATE TABLE books (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    title TEXT NOT NULL DEFAULT 'Unknown' COLLATE NOCASE,
    sort TEXT COLLATE NOCASE,
    timestamp TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    pubdate TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    series_index REAL NOT NULL DEFAULT 1.0,
    author_sort TEXT COLLATE NOCASE,
    isbn TEXT DEFAULT '' COLLATE NOCASE,
    lccn TEXT DEFAULT '' COLLATE NOCASE,
    path TEXT NOT NULL DEFAULT '',
    flags INTEGER NOT NULL DEFAULT 1,
    uuid TEXT,
    has_cover BOOL DEFAULT 0,
    last_modified TIMESTAMP NOT NULL DEFAULT '2000-01-01 00:00:00+00:00'
);
CREATE TABLE authors (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL COLLATE NOCASE,
    sort TEXT COLLATE NOCASE,
    link TEXT NOT NULL DEFAULT '',
    UNIQUE (name)
);
CREATE TABLE books_authors_link (
    id INTEGER PRIMARY KEY,
    book INTEGER NOT NULL,
    author INTEGER NOT NULL,
    UNIQUE (book, author)
);
CREATE TABLE series (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL COLLATE NOCASE,
    sort TEXT COLLATE NOCASE,
    link TEXT NOT NULL DEFAULT '',
    UNIQUE (name)
);
CREATE TABLE books_series_link (
    id INTEGER PRIMARY KEY,
    book INTEGER NOT NULL,
    series INTEGER NOT NULL,
    UNIQUE (book)
);
CREATE TABLE tags (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL COLLATE NOCASE,
    link TEXT NOT NULL DEFAULT '',
    UNIQUE (name)
);
CREATE TABLE books_tags_link (
    id INTEGER PRIMARY KEY,
    book INTEGER NOT NULL,
    tag INTEGER NOT NULL,
    UNIQUE (book, tag)
);
CREATE TABLE ratings (
    id INTEGER PRIMARY KEY,
    rating INTEGER CHECK (rating > -1 AND rating < 11),
    link TEXT NOT NULL DEFAULT '',
    UNIQUE (rating)
);
CREATE TABLE books_ratings_link (
    id INTEGER PRIMARY KEY,
    book INTEGER NOT NULL,
    rating INTEGER NOT NULL,
    UNIQUE (book, rating)
);
CREATE TABLE languages (
    id INTEGER PRIMARY KEY,
    lang_code TEXT NOT NULL COLLATE NOCASE,
    link TEXT NOT NULL DEFAULT '',
    UNIQUE (lang_code)
);
CREATE TABLE books_languages_link (
    id INTEGER PRIMARY KEY,
    book INTEGER NOT NULL,
    lang_code INTEGER NOT NULL,
    item_order INTEGER NOT NULL DEFAULT 0,
    UNIQUE (book, lang_code)
);
CREATE TABLE identifiers (
    id INTEGER PRIMARY KEY,
    book INTEGER NOT NULL,
    type TEXT NOT NULL DEFAULT 'isbn' COLLATE NOCASE,
    val TEXT NOT NULL COLLATE NOCASE,
    UNIQUE (book, type)
);
CREATE TABLE comments (
    id INTEGER PRIMARY KEY,
    book INTEGER NOT NULL,
    text TEXT NOT NULL COLLATE NOCASE,
    UNIQUE (book)
);
CREATE TABLE data (
    id INTEGER PRIMARY KEY,
    book INTEGER NOT NULL,
    format TEXT NOT NULL COLLATE NOCASE,
    uncompressed_size INTEGER NOT NULL,
    name TEXT NOT NULL,
    UNIQUE (book, format)
);
CREATE TABLE custom_columns (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    label TEXT NOT NULL,
    name TEXT NOT NULL,
    datatype TEXT NOT NULL,
    mark_for_delete BOOL DEFAULT 0 NOT NULL,
    editable BOOL DEFAULT 1 NOT NULL,
    display TEXT DEFAULT '{}' NOT NULL,
    is_multiple BOOL DEFAULT 0 NOT NULL,
    normalized BOOL NOT NULL,
    UNIQUE (label)
);

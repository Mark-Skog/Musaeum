-- Phase 2: what the bytes of a revision are, a way back from taking a replacement for a new revision,
-- and where the reader is in each document.

-- What the first bytes of a revision say it is, whatever the file is called. NULL for revisions hashed
-- before this was recorded, for which the format the file's name gives is what there is to go by.
ALTER TABLE revision ADD COLUMN content_kind TEXT CHECK (
    content_kind IS NULL
    OR content_kind IN ('pdf', 'zip', 'djvu', 'rar', '7z', 'mobi', 'xml', 'unknown'));

-- A file that took another's place at its path, as a safe save does, is taken for a new revision of the
-- same document straight away. That stays provisional until its bytes and what it says about itself
-- have been read: a different kind of file, or a different /ID[0] or dc:identifier, means it is
-- something else. This keeps what taking it back needs, which is the instance and what it was before.
CREATE TABLE replaced_file (
    revision_id INTEGER PRIMARY KEY REFERENCES revision (id) ON DELETE CASCADE,
    instance_id INTEGER NOT NULL REFERENCES file_instance (id),
    folder_id INTEGER NOT NULL REFERENCES folder (id),
    name ANY NOT NULL,
    volume_serial INTEGER,
    file_id BLOB CHECK (file_id IS NULL OR length(file_id) = 16),
    mtime_ns INTEGER NOT NULL
) STRICT;
CREATE INDEX replaced_file_instance ON replaced_file (instance_id);

-- Where the reader is in a fixed-layout document, and how they read it.
CREATE TABLE reading_state (
    document_id INTEGER PRIMARY KEY REFERENCES document (id),
    -- The revision the position was taken on, which is what re-anchoring starts from if the file changes.
    revision_id INTEGER REFERENCES revision (id),
    -- The Locator: a page, and a point on it as fractions of its width and height. It means the same at
    -- any zoom, window size or reading mode, which is what keeps the position when any of them changes.
    page INTEGER NOT NULL CHECK (page >= 0),
    u REAL NOT NULL CHECK (u >= 0 AND u <= 1),
    v REAL NOT NULL CHECK (v >= 0 AND v <= 1),
    -- How far through the document, for showing it without opening the file.
    progress REAL NOT NULL CHECK (progress >= 0 AND progress <= 1),
    mode TEXT NOT NULL CHECK (mode IN ('paginated', 'continuous')),
    -- The paginated mode's page turn. Continuous mode has none, and this keeps the preference meanwhile.
    page_turn_animation INTEGER NOT NULL CHECK (page_turn_animation IN (0, 1)),
    -- A factor over what fits the window in the current mode: 1 is fitting.
    zoom REAL NOT NULL CHECK (zoom > 0 AND zoom <= 64),
    -- 'auto' follows what the document asks for, in its /PageLayout.
    spreads TEXT NOT NULL CHECK (spreads IN ('auto', 'single', 'double')),
    updated_at INTEGER NOT NULL
) STRICT;

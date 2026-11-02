-- What cannot be rebuilt from anything else: what the user organizes, what they wrote, and how
-- each file came to be recognized. Whatever can be rebuilt belongs in cache.db.
--
-- Nothing here cascades. Removing something that others point to fails, so losing data takes an
-- explicit decision somewhere in the code, and the triggers below refuse the decisions that must
-- never be taken at all.

CREATE TABLE library_root (
    id INTEGER PRIMARY KEY,
    -- TEXT, or the UTF-16 as a BLOB when it is not valid Unicode, which Windows paths can be.
    path ANY NOT NULL,
    policy TEXT NOT NULL DEFAULT 'reactive' CHECK (policy IN ('reactive')),
    -- A JSON array of name patterns.
    exclusions TEXT NOT NULL DEFAULT '[]',
    -- An unavailable root is one that could not be read, such as an unplugged disk. What is in it
    -- is left exactly as it was: it did not go anywhere, it just cannot be seen.
    status TEXT NOT NULL DEFAULT 'available' CHECK (status IN ('available', 'unavailable', 'removed')),
    added_at INTEGER NOT NULL,
    last_reconciled_at INTEGER
) STRICT;
CREATE UNIQUE INDEX library_root_path ON library_root (path);

-- Renaming or moving a folder changes one row here, and none in file_instance.
CREATE TABLE folder (
    id INTEGER PRIMARY KEY,
    root_id INTEGER NOT NULL REFERENCES library_root (id),
    -- NULL for the root's own folder, whose name is empty: its path is the root's.
    parent_id INTEGER REFERENCES folder (id),
    name ANY NOT NULL,
    volume_serial INTEGER,
    file_id BLOB CHECK (file_id IS NULL OR length(file_id) = 16),
    state TEXT NOT NULL CHECK (state IN ('present', 'absent'))
) STRICT;
CREATE UNIQUE INDEX folder_top ON folder (root_id) WHERE parent_id IS NULL;
CREATE UNIQUE INDEX folder_location ON folder (parent_id, name) WHERE state = 'present';
CREATE INDEX folder_children ON folder (parent_id);
CREATE INDEX folder_identity ON folder (volume_serial, file_id) WHERE file_id IS NOT NULL;

CREATE TABLE work (
    id INTEGER PRIMARY KEY,
    title TEXT NOT NULL,
    language TEXT,
    year INTEGER,
    series TEXT,
    series_position REAL,
    rating INTEGER CHECK (rating IS NULL OR rating BETWEEN 1 AND 5),
    status TEXT NOT NULL DEFAULT 'unread' CHECK (status IN ('unread', 'reading', 'read', 'abandoned')),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;

CREATE TABLE creator (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
) STRICT;

CREATE TABLE work_creator (
    work_id INTEGER NOT NULL REFERENCES work (id),
    creator_id INTEGER NOT NULL REFERENCES creator (id),
    role TEXT NOT NULL DEFAULT 'author',
    position INTEGER NOT NULL,
    PRIMARY KEY (work_id, position)
) STRICT, WITHOUT ROWID;
CREATE INDEX work_creator_creator ON work_creator (creator_id);

-- Where each field of a work came from. A field the user edited is never written over by what is
-- read from a file, however many times it is read again.
CREATE TABLE field_provenance (
    work_id INTEGER NOT NULL REFERENCES work (id),
    field TEXT NOT NULL,
    origin TEXT NOT NULL CHECK (origin IN ('filename', 'embedded', 'user')),
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (work_id, field)
) STRICT, WITHOUT ROWID;

CREATE TABLE document (
    id INTEGER PRIMARY KEY,
    work_id INTEGER NOT NULL REFERENCES work (id),
    layout TEXT NOT NULL CHECK (layout IN ('fixed', 'flow')),
    format TEXT NOT NULL,
    pdf_id0 TEXT,
    dc_identifier TEXT,
    isbn TEXT,
    orientation TEXT CHECK (orientation IS NULL OR orientation IN ('ltr', 'rtl', 'ttb')),
    has_text_layer INTEGER CHECK (has_text_layer IS NULL OR has_text_layer IN (0, 1)),
    created_at INTEGER NOT NULL
) STRICT;
CREATE INDEX document_work ON document (work_id);
CREATE INDEX document_pdf_id0 ON document (pdf_id0) WHERE pdf_id0 IS NOT NULL;
CREATE INDEX document_dc_identifier ON document (dc_identifier) WHERE dc_identifier IS NOT NULL;

-- One revision is one exact sequence of bytes. Until blake3 is known it is unconfirmed: the library
-- already works with it, but nothing has been concluded from it, and it may turn out to be the same
-- bytes as a revision that already exists.
CREATE TABLE revision (
    id INTEGER PRIMARY KEY,
    document_id INTEGER NOT NULL REFERENCES document (id),
    blake3 BLOB UNIQUE CHECK (blake3 IS NULL OR length(blake3) = 32),
    -- NULL while it has not been possible to read any bytes, as with a file only in the cloud.
    fast_fingerprint BLOB CHECK (fast_fingerprint IS NULL OR length(fast_fingerprint) = 32),
    size INTEGER NOT NULL,
    previous_revision_id INTEGER REFERENCES revision (id),
    origin TEXT NOT NULL CHECK (origin IN ('import', 'changed_in_place', 'replaced_at_path')),
    -- A revision among the missing that the fast fingerprint names as possibly the same bytes. It
    -- concludes nothing: only the full hash does. It puts this revision first in line for one.
    candidate_revision_id INTEGER REFERENCES revision (id),
    -- The version of the extractor that read this revision, once it has.
    ingested_version INTEGER,
    created_at INTEGER NOT NULL
) STRICT;
CREATE INDEX revision_document ON revision (document_id);
CREATE INDEX revision_fingerprint ON revision (fast_fingerprint) WHERE fast_fingerprint IS NOT NULL;
CREATE INDEX revision_previous ON revision (previous_revision_id) WHERE previous_revision_id IS NOT NULL;
CREATE INDEX revision_candidate ON revision (candidate_revision_id) WHERE candidate_revision_id IS NOT NULL;
CREATE INDEX revision_needs_content ON revision (id) WHERE blake3 IS NULL OR ingested_version IS NULL;

CREATE TABLE file_instance (
    id INTEGER PRIMARY KEY,
    revision_id INTEGER NOT NULL REFERENCES revision (id),
    folder_id INTEGER NOT NULL REFERENCES folder (id),
    name ANY NOT NULL,
    volume_serial INTEGER,
    file_id BLOB CHECK (file_id IS NULL OR length(file_id) = 16),
    mtime_ns INTEGER NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('present', 'absent', 'cloud_only', 'unreadable')),
    state_since INTEGER NOT NULL
) STRICT;
-- An absent file keeps the last place it was seen, and something else may be there now.
CREATE UNIQUE INDEX file_instance_location ON file_instance (folder_id, name) WHERE state <> 'absent';
CREATE INDEX file_instance_folder ON file_instance (folder_id);
CREATE INDEX file_instance_revision ON file_instance (revision_id);
CREATE INDEX file_instance_identity ON file_instance (volume_serial, file_id) WHERE file_id IS NOT NULL;

-- Only what Phase 1 has to be able to preserve. Phase 4 adds the anchors.
CREATE TABLE annotation (
    id INTEGER PRIMARY KEY,
    document_id INTEGER NOT NULL REFERENCES document (id),
    -- The revision it was made on, which is what re-anchoring starts from.
    revision_id INTEGER NOT NULL REFERENCES revision (id),
    note TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    deleted_at INTEGER
) STRICT;
CREATE INDEX annotation_document ON annotation (document_id);
CREATE INDEX annotation_revision ON annotation (revision_id);

-- Something the library noticed and did not act on, for the user to decide.
CREATE TABLE reconcile_suggestion (
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('same_internal_id', 'same_file_id', 'same_path', 'same_fingerprint')),
    document_id INTEGER NOT NULL REFERENCES document (id),
    candidate_document_id INTEGER NOT NULL REFERENCES document (id),
    evidence TEXT NOT NULL,
    -- Accepting one merges the two documents, which leaves nothing for it to point to.
    state TEXT NOT NULL DEFAULT 'open' CHECK (state IN ('open', 'dismissed')),
    created_at INTEGER NOT NULL,
    UNIQUE (kind, document_id, candidate_document_id)
) STRICT;
CREATE INDEX reconcile_suggestion_state ON reconcile_suggestion (state);
CREATE INDEX reconcile_suggestion_candidate ON reconcile_suggestion (candidate_document_id);

-- The journal of what the library itself does to the disk: the intent is committed before anything
-- is touched, so a crash at any point leaves enough here to find out what happened.
CREATE TABLE fs_operation (
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('move_file', 'move_folder', 'recycle', 'restore')),
    state TEXT NOT NULL CHECK (state IN ('intent', 'done', 'aborted', 'failed', 'undone')),
    instance_id INTEGER REFERENCES file_instance (id),
    folder_id INTEGER REFERENCES folder (id),
    source_path ANY NOT NULL,
    target_path ANY,
    -- A JSON object with what undoing this needs.
    undo_data TEXT NOT NULL DEFAULT '{}',
    undo_of INTEGER REFERENCES fs_operation (id),
    error TEXT,
    created_at INTEGER NOT NULL,
    finished_at INTEGER
) STRICT;
CREATE INDEX fs_operation_state ON fs_operation (state);

-- The guarantees that do not depend on any code being right. A confirmed revision is bytes that
-- existed, and a file instance of one is where they were last seen: both stay for good, whatever
-- becomes of the file. Only an unconfirmed revision, which is a guess that may prove to be a
-- duplicate, can ever go away. Annotations and the journal are deleted logically or not at all.
CREATE TRIGGER revision_stays BEFORE DELETE ON revision WHEN old.blake3 IS NOT NULL BEGIN
    SELECT RAISE(ABORT, 'a confirmed revision is never deleted');
END;
CREATE TRIGGER file_instance_stays BEFORE DELETE ON file_instance
WHEN (SELECT blake3 FROM revision WHERE id = old.revision_id) IS NOT NULL BEGIN
    SELECT RAISE(ABORT, 'a file instance of a confirmed revision is never deleted');
END;
CREATE TRIGGER annotation_stays BEFORE DELETE ON annotation BEGIN
    SELECT RAISE(ABORT, 'an annotation is only ever deleted logically');
END;
CREATE TRIGGER fs_operation_stays BEFORE DELETE ON fs_operation BEGIN
    SELECT RAISE(ABORT, 'the journal is never rewritten');
END;

-- Search. Both indexes hold only the words, not a second copy of the text, and they point to work
-- by rowid. Words find what starts with what was typed, in any language's accents. Trigrams find
-- text in the middle of a word.
CREATE VIRTUAL TABLE search_words USING fts5 (
    title, creators, series,
    content = '', contentless_delete = 1,
    tokenize = 'unicode61 remove_diacritics 2',
    prefix = '1 2 3'
);
CREATE VIRTUAL TABLE search_trigrams USING fts5 (
    title, creators, series,
    content = '', contentless_delete = 1,
    tokenize = 'trigram remove_diacritics 1'
);

CREATE TRIGGER work_search_insert AFTER INSERT ON work BEGIN
    INSERT INTO search_words (rowid, title, creators, series)
        VALUES (new.id, new.title, '', coalesce(new.series, ''));
    INSERT INTO search_trigrams (rowid, title, creators, series)
        VALUES (new.id, new.title, '', coalesce(new.series, ''));
END;

-- Fires for any statement naming these columns, so the condition is what keeps changing a rating from
-- rewriting the indexes. Changing only the creators goes through `refresh_search` in the code.
CREATE TRIGGER work_search_update AFTER UPDATE OF title, series ON work
WHEN old.title IS NOT new.title OR old.series IS NOT new.series BEGIN
    DELETE FROM search_words WHERE rowid = new.id;
    DELETE FROM search_trigrams WHERE rowid = new.id;
    INSERT INTO search_words (rowid, title, creators, series) VALUES (
        new.id, new.title,
        (SELECT coalesce(group_concat(c.name, ' '), '') FROM work_creator w
            JOIN creator c ON c.id = w.creator_id WHERE w.work_id = new.id),
        coalesce(new.series, ''));
    INSERT INTO search_trigrams (rowid, title, creators, series) VALUES (
        new.id, new.title,
        (SELECT coalesce(group_concat(c.name, ' '), '') FROM work_creator w
            JOIN creator c ON c.id = w.creator_id WHERE w.work_id = new.id),
        coalesce(new.series, ''));
END;

CREATE TRIGGER work_search_delete AFTER DELETE ON work BEGIN
    DELETE FROM search_words WHERE rowid = old.id;
    DELETE FROM search_trigrams WHERE rowid = old.id;
END;

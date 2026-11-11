-- Phase 4: annotations anchored in the text, in the shape of the W3C Web Annotation model.
--
-- annotation 1──N annotation_segment 1──N anchor_selector
--
-- An annotation is what the person made: a highlight, an underline or a note, its color, what it
-- says, and the text it quotes. A segment is one stretch of it in one place: a page of a fixed
-- layout, or a document of a flowing book (a selection across a page break, or across chapters, has
-- one segment per page or document). A selector is one way of finding a segment again, and each
-- segment has several, tried in order of priority, so a change to the file that breaks one leaves
-- the others: for flowing books the canonical CFI, then the position in the normalized text, then
-- the quote with what comes before and after it; for fixed layouts the range of characters on the
-- page, then their quads, then the quote.
--
-- When every selector fails, the annotation is an orphan: it stays, with its quote and its note,
-- found by search and listed with the others. Nothing here is ever deleted.

-- The table Phase 1 made, for notes on a whole document, is rebuilt with what an anchored annotation
-- needs. Its id stays the integer the search indexes point at; what identifies an annotation outside
-- this database (an export, a sync) is its UUIDv7.
CREATE TABLE annotation_new (
    id INTEGER PRIMARY KEY,
    -- A UUIDv7: its first 48 bits are when it was made, in Unix milliseconds.
    uuid BLOB NOT NULL UNIQUE CHECK (length(uuid) = 16 AND substr(hex(uuid), 13, 1) = '7'),
    document_id INTEGER NOT NULL REFERENCES document (id),
    -- The revision it was made on, which never changes: what re-anchoring starts from.
    revision_id INTEGER NOT NULL REFERENCES revision (id),
    kind TEXT NOT NULL CHECK (kind IN ('highlight', 'underline', 'note')),
    -- A name from the reader's palette, such as 'yellow'.
    color TEXT NOT NULL CHECK (length(color) BETWEEN 1 AND 32),
    -- What the person wrote, in Markdown. Empty for a mark with no note.
    note TEXT NOT NULL DEFAULT '',
    -- The text the annotation covers, as it was when it was made. NULL when there was none to read.
    quote TEXT,
    quote_source TEXT NOT NULL CHECK (quote_source IN ('text_layer', 'flow', 'ocr', 'none')),
    -- How far into the book it starts, from 0 to 1, for sorting and for showing it without the file.
    progression REAL CHECK (progression IS NULL OR progression BETWEEN 0 AND 1),
    anchor_state TEXT NOT NULL CHECK (anchor_state IN ('exact', 'approximate', 'orphan')),
    -- Where its anchors were last worked out: the revision, and the version of what read it (the
    -- normalizer for a flowing book, the text layer for a fixed one). A revision or a version that
    -- differs from the one open is what sends it through the cascade again.
    resolved_revision_id INTEGER REFERENCES revision (id),
    resolved_version INTEGER,
    -- The /NM of the PDF annotation it came from or went to, once annotations go in and out of PDFs.
    pdf_nm TEXT CHECK (pdf_nm IS NULL OR length(pdf_nm) <= 256),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    deleted_at INTEGER
) STRICT;

-- A Phase 1 note keeps its id and dates and gets a UUIDv7 from the moment it was made. It is about
-- the whole document, so it has no segment, and nothing in it can lose its place.
INSERT INTO annotation_new (
    id, uuid, document_id, revision_id, kind, color, note, quote, quote_source, progression,
    anchor_state, resolved_revision_id, resolved_version, pdf_nm, created_at, updated_at, deleted_at)
SELECT
    id,
    -- `||` makes text, so the bytes are put together as hex and turned into a BLOB once.
    unhex(printf('%012x%04x%02x', created_at & 281474976710655,
                 28672 + abs(random() % 4096), 128 + abs(random() % 64))
          || hex(randomblob(7))),
    document_id, revision_id, 'note', 'yellow', note, NULL, 'none', NULL,
    'exact', revision_id, NULL, NULL, created_at, updated_at, deleted_at
FROM annotation;

-- Dropping the table drops its trigger first, so the rows go with it without asking.
DROP TABLE annotation;
ALTER TABLE annotation_new RENAME TO annotation;
CREATE INDEX annotation_document ON annotation (document_id);
CREATE INDEX annotation_revision ON annotation (revision_id);
CREATE INDEX annotation_resolved ON annotation (resolved_revision_id)
    WHERE resolved_revision_id IS NOT NULL;

CREATE TABLE annotation_segment (
    id INTEGER PRIMARY KEY,
    annotation_id INTEGER NOT NULL REFERENCES annotation (id),
    -- Its place in the annotation, from 0, in reading order.
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    -- Where it is now: a page of a fixed layout, from 0, or the href of a flowing book's document.
    page INTEGER CHECK (page IS NULL OR page >= 0),
    href TEXT CHECK (href IS NULL OR length(href) BETWEEN 1 AND 2048),
    -- An optional clip on the page, [left, top, right, bottom] as fractions of the crop box before
    -- any rotation, as JSON: for a mark on a picture rather than on text.
    clip TEXT CHECK (clip IS NULL OR json_valid(clip)),
    -- Whether this stretch was found again exactly, near enough, or not at all.
    anchor_state TEXT NOT NULL CHECK (anchor_state IN ('exact', 'approximate', 'orphan')),
    CHECK ((page IS NULL) <> (href IS NULL)),
    UNIQUE (annotation_id, ordinal)
) STRICT;

CREATE TABLE anchor_selector (
    id INTEGER PRIMARY KEY,
    segment_id INTEGER NOT NULL REFERENCES annotation_segment (id),
    -- 'cfi': {"start", "end"}, canonical CFIs. 'text_position': {"start", "end"}, UTF-16 code units
    -- into the document's normalized text. 'text_quote': {"exact", "prefix", "suffix"}.
    -- 'page_char_range': {"page", "start", "end"}, characters of the page's text layer, end excluded.
    -- 'page_quads': {"page", "quads"}, each quad four corners as fractions of the unrotated crop box.
    kind TEXT NOT NULL CHECK (kind IN (
        'cfi', 'text_position', 'text_quote', 'page_char_range', 'page_quads')),
    data TEXT NOT NULL CHECK (json_valid(data)),
    -- Lower goes first.
    priority INTEGER NOT NULL,
    -- The version of what the selector counts in: the normalizer's for a flowing book, the text
    -- layer's for a fixed one. NULL for a quote, which counts in nothing.
    normalizer_version INTEGER,
    -- The revision it was worked out on.
    revision_id INTEGER REFERENCES revision (id),
    UNIQUE (segment_id, kind)
) STRICT;

-- What makes an annotation impossible to lose by mistake: none of it is ever deleted, its identity
-- never changes, and what it quotes is never erased. Re-anchoring rewrites where a selector points,
-- never the quote it is checked against.
CREATE TRIGGER annotation_stays BEFORE DELETE ON annotation BEGIN
    SELECT RAISE(ABORT, 'an annotation is only ever deleted logically');
END;
CREATE TRIGGER annotation_keeps_uuid BEFORE UPDATE OF uuid ON annotation
WHEN old.uuid IS NOT new.uuid BEGIN
    SELECT RAISE(ABORT, 'an annotation keeps its UUID');
END;
CREATE TRIGGER annotation_keeps_quote BEFORE UPDATE OF quote ON annotation
WHEN old.quote IS NOT new.quote BEGIN
    SELECT RAISE(ABORT, 'an annotation keeps what it quoted');
END;
CREATE TRIGGER annotation_segment_stays BEFORE DELETE ON annotation_segment BEGIN
    SELECT RAISE(ABORT, 'a segment of an annotation is never deleted');
END;
CREATE TRIGGER anchor_selector_stays BEFORE DELETE ON anchor_selector BEGIN
    SELECT RAISE(ABORT, 'a selector of an annotation is never deleted');
END;
CREATE TRIGGER anchor_quote_stays BEFORE UPDATE ON anchor_selector
WHEN old.kind = 'text_quote' AND (new.kind IS NOT old.kind OR new.data IS NOT old.data) BEGIN
    SELECT RAISE(ABORT, 'a quote selector keeps what was quoted');
END;

-- Search in what was quoted and what was written. Like the search of works, the indexes hold only the
-- words and point at the annotation by rowid. An orphan stays in them; a deleted annotation does not.
CREATE VIRTUAL TABLE annotation_words USING fts5 (
    quote, note,
    content = '', contentless_delete = 1,
    tokenize = 'unicode61 remove_diacritics 2',
    prefix = '1 2 3'
);
CREATE VIRTUAL TABLE annotation_trigrams USING fts5 (
    quote, note,
    content = '', contentless_delete = 1,
    tokenize = 'trigram remove_diacritics 1'
);
INSERT INTO annotation_words (rowid, quote, note)
    SELECT id, coalesce(quote, ''), note FROM annotation WHERE deleted_at IS NULL;
INSERT INTO annotation_trigrams (rowid, quote, note)
    SELECT id, coalesce(quote, ''), note FROM annotation WHERE deleted_at IS NULL;

CREATE TRIGGER annotation_search_insert AFTER INSERT ON annotation WHEN new.deleted_at IS NULL BEGIN
    INSERT INTO annotation_words (rowid, quote, note) VALUES (new.id, coalesce(new.quote, ''), new.note);
    INSERT INTO annotation_trigrams (rowid, quote, note) VALUES (new.id, coalesce(new.quote, ''), new.note);
END;

-- Fires for any statement naming these columns; the condition keeps a change of color from
-- rewriting the indexes.
CREATE TRIGGER annotation_search_update AFTER UPDATE OF note, deleted_at ON annotation
WHEN old.note IS NOT new.note OR old.deleted_at IS NOT new.deleted_at BEGIN
    DELETE FROM annotation_words WHERE rowid = old.id;
    DELETE FROM annotation_trigrams WHERE rowid = old.id;
    INSERT INTO annotation_words (rowid, quote, note)
        SELECT new.id, coalesce(new.quote, ''), new.note WHERE new.deleted_at IS NULL;
    INSERT INTO annotation_trigrams (rowid, quote, note)
        SELECT new.id, coalesce(new.quote, ''), new.note WHERE new.deleted_at IS NULL;
END;

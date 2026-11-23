-- Phase 6, Part 2: what came from a Calibre library, what a book is about, and the pairs of works a
-- person said are different books.

-- Calibre is a source of its own. What a person curated there ranks above what a file says about
-- itself and below what the person sets here: a file read again never writes over it, a second
-- import from Calibre does, and an edit here makes the field the person's. SQLite cannot change a
-- CHECK, so the table is made again with the same rows.
CREATE TABLE field_provenance_new (
    work_id INTEGER NOT NULL REFERENCES work (id),
    field TEXT NOT NULL,
    origin TEXT NOT NULL CHECK (origin IN ('filename', 'embedded', 'calibre', 'user')),
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (work_id, field)
) STRICT, WITHOUT ROWID;
INSERT INTO field_provenance_new (work_id, field, origin, updated_at)
    SELECT work_id, field, origin, updated_at FROM field_provenance;
DROP TABLE field_provenance;
ALTER TABLE field_provenance_new RENAME TO field_provenance;

-- What the book is about, as plain text: Calibre's comments, or what the person writes.
ALTER TABLE work ADD COLUMN description TEXT
    CHECK (description IS NULL OR length(description) BETWEEN 1 AND 100000);

-- Two works a person said are different books, the smaller id first: never suggested as one again.
CREATE TABLE work_pair_dismissed (
    work_a INTEGER NOT NULL REFERENCES work (id),
    work_b INTEGER NOT NULL REFERENCES work (id),
    created_at INTEGER NOT NULL,
    PRIMARY KEY (work_a, work_b),
    CHECK (work_a < work_b)
) STRICT, WITHOUT ROWID;
CREATE INDEX work_pair_dismissed_b ON work_pair_dismissed (work_b);

-- The books brought in from a Calibre library, by that library's folder and its own id for each:
-- a second import of the same library finds the same works.
CREATE TABLE calibre_book (
    source TEXT NOT NULL CHECK (length(source) BETWEEN 1 AND 4096),
    calibre_id INTEGER NOT NULL,
    calibre_uuid TEXT CHECK (calibre_uuid IS NULL OR length(calibre_uuid) <= 64),
    work_id INTEGER NOT NULL REFERENCES work (id),
    imported_at INTEGER NOT NULL,
    PRIMARY KEY (source, calibre_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX calibre_book_work ON calibre_book (work_id);

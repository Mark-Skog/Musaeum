-- Phase 6, Part 2: which files' text is in the full-text index (`cache/fulltext`), by their bytes,
-- so that every copy of the same file shares one entry and a file that changed is read again.
CREATE TABLE fulltext_content (
    blake3 BLOB PRIMARY KEY CHECK (length(blake3) = 32),
    -- The version of what read the text and of the index's form: another one reads it again.
    version INTEGER NOT NULL CHECK (version >= 1),
    -- `done`, with what it found; `empty` when the file gives no text (a comic, a scan nobody has
    -- read yet); `failed` when it could not be read, with why.
    state TEXT NOT NULL CHECK (state IN ('done', 'empty', 'failed')),
    units INTEGER NOT NULL CHECK (units >= 0),
    chars INTEGER NOT NULL CHECK (chars >= 0),
    -- The lines read by OCR that were in it: more since then and its scanned pages are read again.
    ocr_lines INTEGER NOT NULL DEFAULT 0 CHECK (ocr_lines >= 0),
    error TEXT CHECK (error IS NULL OR length(error) <= 2000),
    indexed_at INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

-- The text the OCR read on the lines of scanned pages, line by line: each line of the detector's
-- model, straightened and read by Tesseract in its sandboxed worker. A selection over lines read
-- once is answered at once, and an annotation on them finds its quote without reading again.
--
-- Kept by the file's exact bytes, as everything in this cache, the page and the line as the model of
-- `lines_version` numbers it; what read it is `ocr_version` and its `languages` (`por+eng`). A line
-- read by another version, in other languages or counted by another model counts as unread and is
-- read again.
CREATE TABLE ocr_line (
    blake3 BLOB NOT NULL CHECK (length(blake3) = 32),
    page INTEGER NOT NULL CHECK (page >= 0),
    line INTEGER NOT NULL CHECK (line >= 0),
    lines_version INTEGER NOT NULL CHECK (lines_version >= 1),
    ocr_version INTEGER NOT NULL CHECK (ocr_version >= 1),
    languages TEXT NOT NULL CHECK (length(languages) BETWEEN 2 AND 64),
    -- {"words": [[text, from, to, confidence], ...], "confidence": mean, "flipped": bool}, the words
    -- in reading order, from and to as fractions of the line's baseline.
    words TEXT NOT NULL CHECK (json_valid(words) AND length(words) <= 262144),
    created_at INTEGER NOT NULL,
    PRIMARY KEY (blake3, page, line)
) STRICT, WITHOUT ROWID;

-- The size and shape of every page of a fixed-layout file, by its exact bytes: read once without
-- drawing anything, and from then on known before a worker has even opened the file, so that the
-- whole layout is right from the first frame.
CREATE TABLE page_geometry (
    blake3 BLOB NOT NULL CHECK (length(blake3) = 32),
    page INTEGER NOT NULL CHECK (page >= 0),
    -- In points, with the page's /Rotate applied.
    width_pt REAL NOT NULL,
    height_pt REAL NOT NULL,
    orientation TEXT NOT NULL CHECK (orientation IN ('portrait', 'landscape', 'square')),
    PRIMARY KEY (blake3, page)
) STRICT, WITHOUT ROWID;

-- What the whole file is like, and what it asks of a reader. The pages above are complete exactly when
-- this row exists, since both are written in the same transaction.
CREATE TABLE document_geometry (
    blake3 BLOB PRIMARY KEY CHECK (length(blake3) = 32),
    page_count INTEGER NOT NULL CHECK (page_count >= 0),
    -- 'portrait', 'landscape' or 'square' if every page is, 'mixed' otherwise, 'empty' with no pages.
    summary TEXT NOT NULL CHECK (summary IN ('empty', 'portrait', 'landscape', 'square', 'mixed')),
    portrait INTEGER NOT NULL,
    landscape INTEGER NOT NULL,
    square INTEGER NOT NULL,
    -- The catalog's /PageLayout, as the name it gives, and /ViewerPreferences /Direction: 'L2R' or 'R2L'.
    page_layout TEXT,
    direction TEXT,
    created_at INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

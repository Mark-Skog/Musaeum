-- The lines of text found on the picture of a scanned or hybrid page: the line detector's model, as
-- the `MLIN` bytes of musaeum-vision, with baselines as polylines in fractions of the unrotated crop
-- box. Traced in the background after the page is classified, so that selecting on the page snaps to
-- its lines at once, and kept by the file's exact bytes, as everything in this cache.
--
-- `version` says which detector made the model; a model of another version counts as missing and is
-- traced again. `page_hash` is the 64-bit difference hash of the whole page's picture: an annotation
-- made over the page keeps it, and the page it lands on later is compared with it.
CREATE TABLE line_model (
    blake3 BLOB NOT NULL CHECK (length(blake3) = 32),
    page INTEGER NOT NULL CHECK (page >= 0),
    version INTEGER NOT NULL CHECK (version >= 1),
    -- The hash's 64 bits, as SQLite's signed integer holds them.
    page_hash INTEGER NOT NULL,
    model BLOB NOT NULL CHECK (length(model) BETWEEN 1 AND 4194304),
    created_at INTEGER NOT NULL,
    PRIMARY KEY (blake3, page)
) STRICT, WITHOUT ROWID;

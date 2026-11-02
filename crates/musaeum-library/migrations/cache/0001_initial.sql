-- What can be rebuilt. Losing this whole file costs some reading and nothing else, so it is never
-- backed up, and it is simply replaced whenever it will not open.

-- What the isolated worker read out of some exact bytes, so that reading it again does not take
-- another trip to the worker.
CREATE TABLE extraction (
    blake3 BLOB PRIMARY KEY CHECK (length(blake3) = 32),
    extractor_version INTEGER NOT NULL,
    -- A JSON object, or NULL if the file could not be read as what it claimed to be.
    data TEXT
) STRICT, WITHOUT ROWID;

-- What past scans found, for inspecting the library.
CREATE TABLE reconcile_run (
    id INTEGER PRIMARY KEY,
    root_id INTEGER NOT NULL,
    finished_at INTEGER NOT NULL,
    duration_ms INTEGER NOT NULL,
    -- A JSON object with what happened.
    report TEXT NOT NULL
) STRICT;

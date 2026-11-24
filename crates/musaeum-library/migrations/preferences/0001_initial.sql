-- Phase 8, Part 1: the person's preferences, the app's own and not a library's. One row per
-- preference, its value as JSON, so that a preference this version does not know (from a newer one)
-- is kept and left alone, and one it cannot read falls back to its default.
CREATE TABLE preference (
    key TEXT PRIMARY KEY CHECK (length(key) BETWEEN 1 AND 64),
    value TEXT NOT NULL CHECK (json_valid(value) AND length(value) <= 4096),
    updated_at INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

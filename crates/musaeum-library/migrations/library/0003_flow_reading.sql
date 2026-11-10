-- Phase 3: where the reader is in a flowing book, and the reader's own settings.

-- A flowing book has no pages to point at, so its position is a Locator, in Readium's shape, as JSON:
-- a canonical CFI, the text quoted around it, and how far into the document and the book it is. Its
-- row keeps page 0 at (0, 0), zoom 1 and spreads 'auto', which mean nothing for it.
ALTER TABLE reading_state ADD COLUMN locator TEXT CHECK (locator IS NULL OR json_valid(locator));

-- Settings that belong to the reader and not to one book, such as the typography of flowing books.
-- Each value is JSON, so a setting can grow fields without a migration.
CREATE TABLE preference (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL CHECK (json_valid(value)),
    updated_at INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

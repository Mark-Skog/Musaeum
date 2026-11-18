-- Phase 6: how a person organizes the library, beside the folders on the disk. All of it is the
-- person's own, so none of it has a provenance: nothing read from a file ever writes here.
--
-- Categories are a tree. Each keeps the ids of the path from the top down to itself, `/3/14/15/`,
-- so a category and everything under it are the rows whose path starts with its own, and a
-- subtree moves by rewriting that prefix in one statement. Siblings keep the order the person gave
-- them by a fractional key (`sort_key`): a key between two others always exists, so moving one
-- changes one row. A work may be in any number of categories.

CREATE TABLE category (
    id INTEGER PRIMARY KEY,
    parent_id INTEGER REFERENCES category (id),
    name TEXT NOT NULL CHECK (length(trim(name)) > 0 AND length(name) <= 200),
    -- The name folded (lower case, no accents, one space between words): two siblings may not
    -- share it.
    name_key TEXT NOT NULL,
    path TEXT NOT NULL CHECK (path GLOB '/*/' AND path NOT GLOB '*[^0-9/]*'),
    sort_key TEXT NOT NULL CHECK (length(sort_key) > 0 AND sort_key NOT GLOB '*[^0-9A-Za-z]*'),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;
CREATE UNIQUE INDEX category_path ON category (path);
CREATE UNIQUE INDEX category_sibling_name ON category (coalesce(parent_id, 0), name_key);
CREATE INDEX category_children ON category (parent_id, sort_key);

CREATE TABLE work_category (
    work_id INTEGER NOT NULL REFERENCES work (id),
    category_id INTEGER NOT NULL REFERENCES category (id),
    added_at INTEGER NOT NULL,
    PRIMARY KEY (work_id, category_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX work_category_category ON work_category (category_id, work_id);

CREATE TABLE tag (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL CHECK (length(trim(name)) > 0 AND length(name) <= 100),
    name_key TEXT NOT NULL UNIQUE,
    color TEXT CHECK (color IS NULL OR color IN ('yellow', 'green', 'blue', 'pink', 'purple', 'orange', 'gray')),
    created_at INTEGER NOT NULL
) STRICT;

CREATE TABLE work_tag (
    work_id INTEGER NOT NULL REFERENCES work (id),
    tag_id INTEGER NOT NULL REFERENCES tag (id),
    added_at INTEGER NOT NULL,
    PRIMARY KEY (work_id, tag_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX work_tag_tag ON work_tag (tag_id, work_id);

-- A collection is a list the person keeps by hand, in an order of their own.
CREATE TABLE collection (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL CHECK (length(trim(name)) > 0 AND length(name) <= 200),
    description TEXT NOT NULL DEFAULT '',
    sort_key TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;

CREATE TABLE collection_item (
    collection_id INTEGER NOT NULL REFERENCES collection (id),
    work_id INTEGER NOT NULL REFERENCES work (id),
    sort_key TEXT NOT NULL,
    added_at INTEGER NOT NULL,
    PRIMARY KEY (collection_id, work_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX collection_item_order ON collection_item (collection_id, sort_key);
CREATE INDEX collection_item_work ON collection_item (work_id);

-- A smart collection is a rule: the works that match it now. The rule is JSON, checked by the code
-- against the fields it may name (`smart.rs`); it never holds SQL.
CREATE TABLE smart_collection (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL CHECK (length(trim(name)) > 0 AND length(name) <= 200),
    rules TEXT NOT NULL CHECK (json_valid(rules)),
    sort_key TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;

-- The identifiers of a work: what the files say (the document's isbn, dc:identifier) and what the
-- person adds. Their provenance is the work's field `identifiers`.
CREATE TABLE work_identifier (
    work_id INTEGER NOT NULL REFERENCES work (id),
    position INTEGER NOT NULL,
    scheme TEXT NOT NULL CHECK (scheme IN ('isbn', 'issn', 'doi', 'oclc', 'lccn', 'asin', 'uri', 'other')),
    value TEXT NOT NULL CHECK (length(trim(value)) > 0 AND length(value) <= 400),
    PRIMARY KEY (work_id, position)
) STRICT, WITHOUT ROWID;
CREATE INDEX work_identifier_value ON work_identifier (scheme, value);

-- Fields the person defines, each of one kind, and their values on works.
CREATE TABLE custom_field (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL CHECK (length(trim(name)) > 0 AND length(name) <= 100),
    name_key TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL CHECK (kind IN ('text', 'number', 'date', 'boolean', 'choice', 'url')),
    -- For `choice`: a JSON array of the choices, in order.
    choices TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(choices)),
    sort_key TEXT NOT NULL,
    created_at INTEGER NOT NULL
) STRICT;

CREATE TABLE work_custom_value (
    work_id INTEGER NOT NULL REFERENCES work (id),
    field_id INTEGER NOT NULL REFERENCES custom_field (id),
    -- TEXT for text, date (YYYY-MM-DD), choice and url; REAL for number; INTEGER 0/1 for boolean.
    value ANY NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (work_id, field_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX work_custom_value_field ON work_custom_value (field_id);

-- `field_provenance` names one more field, `identifiers`, and the editor writes `user` for any
-- field the person changes.

-- The identifiers the files already gave, as the works' first ones: each document's ISBN, and its
-- dc:identifier by its scheme (urn:isbn:, urn:doi: and doi: lose the prefix). The same value twice
-- is kept once, in the order the documents came.
INSERT INTO work_identifier (work_id, position, scheme, value)
SELECT work_id, row_number() OVER (PARTITION BY work_id ORDER BY first_seen, scheme, value) - 1,
       scheme, value
FROM (
    SELECT work_id, scheme, value, min(document_id * 2 + kind) AS first_seen
    FROM (
        SELECT work_id, id AS document_id, 0 AS kind, 'isbn' AS scheme, trim(isbn) AS value
        FROM document WHERE isbn IS NOT NULL AND trim(isbn) <> ''
        UNION ALL
        SELECT work_id, id, 1,
            CASE WHEN lower(dc_identifier) LIKE 'urn:isbn:%' THEN 'isbn'
                 WHEN lower(dc_identifier) LIKE 'urn:doi:%' OR lower(dc_identifier) LIKE 'doi:%'
                      OR dc_identifier LIKE '10.%/%' THEN 'doi'
                 WHEN dc_identifier LIKE '%:%' THEN 'uri'
                 ELSE 'other' END,
            trim(CASE WHEN lower(dc_identifier) LIKE 'urn:isbn:%' THEN substr(dc_identifier, 10)
                      WHEN lower(dc_identifier) LIKE 'urn:doi:%' THEN substr(dc_identifier, 9)
                      WHEN lower(dc_identifier) LIKE 'doi:%' THEN substr(dc_identifier, 5)
                      ELSE dc_identifier END)
        FROM document WHERE dc_identifier IS NOT NULL AND trim(dc_identifier) <> ''
    )
    WHERE value <> '' AND length(value) <= 400
    GROUP BY work_id, scheme, value
);

-- Marks on the picture of a page rather than on its text, for scanned books: lines the detector found
-- on the scan, a box, or a stroke drawn by hand. Their segment keeps one more kind of selector:
--
-- 'page_region': {"page", "tool", "quads", "clip", "lines", "path", "pageHash", "regionHash"}. `tool`
-- is 'lines', 'rect' or 'free'; `quads` what the mark is drawn with, as fractions of the unrotated
-- crop box, as 'page_quads' has them; `clip` its box, which the segment's clip repeats; `lines` the
-- detector's lines it covers, each {"index", "from", "to"} with the ends as fractions along the line;
-- `path` the stroke's points, u and v in turn; `pageHash` and `regionHash` the 64-bit difference hashes
-- of the whole page and of the region, as sixteen hex digits, which say whether a later revision still
-- shows the same picture there. The selector's version is the line detector's for 'lines'.
--
-- SQLite cannot change a CHECK, so the table is made again with the same rows, ids included, and its
-- triggers with it. Nothing else refers to its rows.
DROP TRIGGER anchor_quote_stays;
DROP TRIGGER anchor_selector_stays;

CREATE TABLE anchor_selector_new (
    id INTEGER PRIMARY KEY,
    segment_id INTEGER NOT NULL REFERENCES annotation_segment (id),
    -- 'cfi': {"start", "end"}, canonical CFIs. 'text_position': {"start", "end"}, UTF-16 code units
    -- into the document's normalized text. 'text_quote': {"exact", "prefix", "suffix"}.
    -- 'page_char_range': {"page", "start", "end"}, characters of the page's text layer, end excluded.
    -- 'page_quads': {"page", "quads"}, each quad four corners as fractions of the unrotated crop box.
    -- 'page_region': a mark on the picture of the page, as above.
    kind TEXT NOT NULL CHECK (kind IN (
        'cfi', 'text_position', 'text_quote', 'page_char_range', 'page_quads', 'page_region')),
    data TEXT NOT NULL CHECK (json_valid(data)),
    -- Lower goes first.
    priority INTEGER NOT NULL,
    -- The version of what the selector counts in: the normalizer's for a flowing book, the text
    -- layer's for a fixed one, the line detector's for lines found on a scan. NULL for a quote, a
    -- box or a stroke, which count in nothing.
    normalizer_version INTEGER,
    -- The revision it was worked out on.
    revision_id INTEGER REFERENCES revision (id),
    UNIQUE (segment_id, kind)
) STRICT;
INSERT INTO anchor_selector_new (id, segment_id, kind, data, priority, normalizer_version, revision_id)
    SELECT id, segment_id, kind, data, priority, normalizer_version, revision_id FROM anchor_selector;
DROP TABLE anchor_selector;
ALTER TABLE anchor_selector_new RENAME TO anchor_selector;

CREATE TRIGGER anchor_selector_stays BEFORE DELETE ON anchor_selector BEGIN
    SELECT RAISE(ABORT, 'a selector of an annotation is never deleted');
END;
CREATE TRIGGER anchor_quote_stays BEFORE UPDATE ON anchor_selector
WHEN old.kind = 'text_quote' AND (new.kind IS NOT old.kind OR new.data IS NOT old.data) BEGIN
    SELECT RAISE(ABORT, 'a quote selector keeps what was quoted');
END;

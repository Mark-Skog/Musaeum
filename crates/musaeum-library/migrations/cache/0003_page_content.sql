-- What each page is made of: text the document describes, a picture of the page, or a picture with a
-- text layer over it, as OCR leaves one. Measured in the background after a document opens, page by
-- page, so a page can be classified while others are not yet. This is what later decides whether the
-- line detector has to find a scanned page's lines from its pixels.
--
-- The measurements are kept beside the class, so a change of thresholds can be judged against them;
-- `content_version` says which rules made the class, and pages classified by older rules are
-- classified again.
ALTER TABLE page_geometry ADD COLUMN content TEXT
    CHECK (content IN ('text', 'scanned', 'hybrid'));
-- Characters in the text layer, white space not counted.
ALTER TABLE page_geometry ADD COLUMN text_chars INTEGER CHECK (text_chars >= 0);
-- Fraction of the page that raster images cover, from 0 to 1.
ALTER TABLE page_geometry ADD COLUMN image_coverage REAL
    CHECK (image_coverage BETWEEN 0 AND 1);
ALTER TABLE page_geometry ADD COLUMN content_version INTEGER;

-- The whole file: the class of at least 95% of its pages, 'mixed' if no class has that many, NULL until
-- every page is classified. A text book with a scanned cover and a few plates is still 'text'.
-- Whether every page has that one class is `is_strictly_uniform`, and the counts say how far from it.
ALTER TABLE document_geometry ADD COLUMN content TEXT
    CHECK (content IN ('text', 'scanned', 'hybrid', 'mixed'));
ALTER TABLE document_geometry ADD COLUMN is_strictly_uniform INTEGER
    CHECK (is_strictly_uniform IN (0, 1));
ALTER TABLE document_geometry ADD COLUMN text_pages INTEGER;
ALTER TABLE document_geometry ADD COLUMN scanned_pages INTEGER;
ALTER TABLE document_geometry ADD COLUMN hybrid_pages INTEGER;
ALTER TABLE document_geometry ADD COLUMN classified_at INTEGER;

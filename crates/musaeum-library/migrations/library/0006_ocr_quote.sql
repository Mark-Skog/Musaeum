-- A mark on a scanned page has no text when it is made: the OCR reads the lines under it afterwards,
-- in the background. Its quote may therefore be filled once, from nothing; once it says something,
-- it never changes again, as before. The search indexes follow a quote that is filled, so a mark is
-- found by the words it covers as soon as they are read.
DROP TRIGGER annotation_keeps_quote;
CREATE TRIGGER annotation_keeps_quote BEFORE UPDATE OF quote ON annotation
WHEN old.quote IS NOT NULL AND old.quote IS NOT new.quote BEGIN
    SELECT RAISE(ABORT, 'an annotation keeps what it quoted');
END;

DROP TRIGGER annotation_search_update;
-- Fires for any statement naming these columns; the condition keeps a change of color from
-- rewriting the indexes.
CREATE TRIGGER annotation_search_update AFTER UPDATE OF note, deleted_at, quote ON annotation
WHEN old.note IS NOT new.note OR old.deleted_at IS NOT new.deleted_at OR old.quote IS NOT new.quote
BEGIN
    DELETE FROM annotation_words WHERE rowid = old.id;
    DELETE FROM annotation_trigrams WHERE rowid = old.id;
    INSERT INTO annotation_words (rowid, quote, note)
        SELECT new.id, coalesce(new.quote, ''), new.note WHERE new.deleted_at IS NULL;
    INSERT INTO annotation_trigrams (rowid, quote, note)
        SELECT new.id, coalesce(new.quote, ''), new.note WHERE new.deleted_at IS NULL;
END;

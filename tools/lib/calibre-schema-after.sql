-- Added after the rows of a synthetic Calibre library (see calibre-schema.sql): a view and a trigger
-- like Calibre's, which call `title_sort`, a function only Calibre defines. Musaeum reads the
-- tables with `trusted_schema` off and never these, so a reader that runs them is caught.
CREATE VIEW meta AS SELECT id, title, title_sort(title) AS sort FROM books;
CREATE TRIGGER books_insert_trg AFTER INSERT ON books
BEGIN
    UPDATE books SET sort = title_sort(NEW.title) WHERE id = NEW.id;
END;

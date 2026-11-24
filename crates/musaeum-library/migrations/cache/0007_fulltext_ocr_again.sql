-- Phase 7, Part 2: until now the full-text index took no line the OCR had read (it looked for
-- another form of a line than the one `ocr_line` keeps), so a scanned file indexed after some of its
-- lines were read has none of them. Counting those lines as never seen reads again those files'
-- pages, and only theirs.
UPDATE fulltext_content SET ocr_lines = 0 WHERE ocr_lines > 0;

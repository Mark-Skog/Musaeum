// A synthetic Calibre library, written as Calibre writes one: metadata.db with the tables of
// calibre-schema.sql, custom columns with their own tables, a folder per book with its files, the
// metadata.opf Calibre keeps beside them and a cover. After the rows come Calibre's view and
// trigger (calibre-schema-after.sql), which call a function only Calibre has: a reader that ran
// them would fail. The Rust tests build theirs from the same schema (crates/musaeum-library/tests/beta.rs).

import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { DatabaseSync } from 'node:sqlite';

const here = import.meta.dirname;

/** The custom columns: label, name, datatype, normalized, display. */
export const CALIBRE_COLUMNS = [
  ['lido', 'Lido', 'bool', false, '{}'],
  ['genero', 'Gênero', 'enumeration', true, '{"enum_values": ["Romance", "Ensaio", "Poesia"]}'],
  ['paginas', 'Páginas', 'int', false, '{}'],
  ['nota', 'Nota', 'comments', false, '{}'],
  ['emprestado', 'Emprestado em', 'datetime', false, '{}'],
  ['estrelas', 'Estrelas', 'rating', true, '{}'],
  ['resumo', 'Resumo', 'composite', false, '{"composite_template": "{title}"}'],
];

const xml = (text) => String(text).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');

function opfOf(book, id) {
  let metadata = `<dc:identifier opf:scheme="calibre" id="calibre_id">${id}</dc:identifier><dc:title>${xml(book.title)}</dc:title>`;
  for (const author of book.authors) {
    metadata += `<dc:creator opf:role="aut">${xml(author)}</dc:creator>`;
  }
  for (const tag of book.tags ?? []) {
    metadata += `<dc:subject>${xml(tag)}</dc:subject>`;
  }
  for (const [kind, value] of book.identifiers ?? []) {
    metadata += `<dc:identifier opf:scheme="${xml(kind.toUpperCase())}">${xml(value)}</dc:identifier>`;
  }
  if (book.comments) {
    metadata += `<dc:description>${xml(book.comments)}</dc:description>`;
  }
  if (book.language) {
    metadata += `<dc:language>${xml(book.language)}</dc:language>`;
  }
  metadata += `<dc:date>${xml(book.pubdate)}</dc:date>`;
  if (book.series) {
    metadata += `<meta name="calibre:series" content="${xml(book.series[0])}"/><meta name="calibre:series_index" content="${book.series[1]}"/>`;
  }
  if (book.rating !== undefined && book.rating !== null) {
    metadata += `<meta name="calibre:rating" content="${book.rating}"/>`;
  }
  return `<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" unique-identifier="uuid_id" version="2.0">
<metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">${metadata}</metadata>
</package>`;
}

/**
 * Writes a Calibre library at `dir`. Each book: { title, authors, series: [name, index] | null,
 * tags, rating (0-10), identifiers: [[kind, value]], comments (HTML), language (ISO 639-2), pubdate,
 * folder ('Author/Title (id)'), files: [[format, bytes | null]] (null: listed, not on the disk),
 * custom: [[label, value]] }.
 */
export function writeCalibreLibrary(dir, books) {
  mkdirSync(dir, { recursive: true });
  const db = new DatabaseSync(join(dir, 'metadata.db'));
  try {
    db.exec(readFileSync(join(here, 'calibre-schema.sql'), 'utf8'));
    const columns = new Map();
    for (const [label, name, datatype, normalized, display] of CALIBRE_COLUMNS) {
      const { lastInsertRowid } = db
        .prepare('INSERT INTO custom_columns (label, name, datatype, display, normalized) VALUES (?, ?, ?, ?, ?)')
        .run(label, name, datatype, display, normalized ? 1 : 0);
      const id = Number(lastInsertRowid);
      columns.set(label, { id, datatype, normalized });
      if (datatype === 'composite') {
        continue;
      }
      if (normalized) {
        db.exec(
          `CREATE TABLE custom_column_${id} (id INTEGER PRIMARY KEY, value TEXT NOT NULL, link TEXT NOT NULL DEFAULT '', UNIQUE (value));
           CREATE TABLE books_custom_column_${id}_link (id INTEGER PRIMARY KEY, book INTEGER NOT NULL, value INTEGER NOT NULL, UNIQUE (book, value));`,
        );
      } else {
        db.exec(`CREATE TABLE custom_column_${id} (id INTEGER PRIMARY KEY, book INTEGER, value, UNIQUE (book));`);
      }
    }
    books.forEach((book, at) => {
      const { lastInsertRowid } = db
        .prepare('INSERT INTO books (title, sort, pubdate, series_index, path, uuid) VALUES (?, ?, ?, ?, ?, ?)')
        .run(book.title, book.title, book.pubdate, book.series ? book.series[1] : 1.0, book.folder, `00000000-0000-4000-8000-${String(at + 1).padStart(12, '0')}`);
      const id = Number(lastInsertRowid);
      for (const author of book.authors) {
        db.prepare('INSERT OR IGNORE INTO authors (name, sort) VALUES (?, ?)').run(author, author);
        db.prepare('INSERT INTO books_authors_link (book, author) SELECT ?, id FROM authors WHERE name = ?').run(id, author);
      }
      if (book.series) {
        db.prepare('INSERT OR IGNORE INTO series (name, sort) VALUES (?, ?)').run(book.series[0], book.series[0]);
        db.prepare('INSERT INTO books_series_link (book, series) SELECT ?, id FROM series WHERE name = ?').run(id, book.series[0]);
      }
      for (const tag of book.tags ?? []) {
        db.prepare('INSERT OR IGNORE INTO tags (name) VALUES (?)').run(tag);
        db.prepare('INSERT INTO books_tags_link (book, tag) SELECT ?, id FROM tags WHERE name = ?').run(id, tag);
      }
      if (book.rating !== undefined && book.rating !== null) {
        db.prepare('INSERT OR IGNORE INTO ratings (rating) VALUES (?)').run(book.rating);
        db.prepare('INSERT INTO books_ratings_link (book, rating) SELECT ?, id FROM ratings WHERE rating = ?').run(id, book.rating);
      }
      if (book.language) {
        db.prepare('INSERT OR IGNORE INTO languages (lang_code) VALUES (?)').run(book.language);
        db.prepare('INSERT INTO books_languages_link (book, lang_code) SELECT ?, id FROM languages WHERE lang_code = ?').run(id, book.language);
      }
      for (const [kind, value] of book.identifiers ?? []) {
        db.prepare('INSERT INTO identifiers (book, type, val) VALUES (?, ?, ?)').run(id, kind, value);
      }
      if (book.comments) {
        db.prepare('INSERT INTO comments (book, text) VALUES (?, ?)').run(id, book.comments);
      }
      const folder = join(dir, ...book.folder.split('/'));
      mkdirSync(folder, { recursive: true });
      for (const [format, bytes] of book.files) {
        const name = `${book.title} - ${book.authors[0] ?? 'Desconhecido'}`;
        db.prepare('INSERT INTO data (book, format, uncompressed_size, name) VALUES (?, ?, ?, ?)').run(id, format.toUpperCase(), bytes === null ? 0 : bytes.length, name);
        if (bytes !== null) {
          writeFileSync(join(folder, `${name}.${format}`), bytes);
        }
      }
      // Calibre's backup of the book's metadata, and a cover: neither is a book.
      writeFileSync(join(folder, 'metadata.opf'), opfOf(book, id));
      writeFileSync(join(folder, 'cover.jpg'), Buffer.from([0xff, 0xd8, 0xff, 0xd9]));
      for (const [label, value] of book.custom ?? []) {
        const column = columns.get(label);
        const stored = typeof value === 'boolean' ? (value ? 1 : 0) : value;
        if (column.normalized) {
          db.prepare(`INSERT OR IGNORE INTO custom_column_${column.id} (value) VALUES (?)`).run(stored);
          db.prepare(`INSERT INTO books_custom_column_${column.id}_link (book, value) SELECT ?, id FROM custom_column_${column.id} WHERE value = ?`).run(id, stored);
        } else {
          db.prepare(`INSERT INTO custom_column_${column.id} (book, value) VALUES (?, ?)`).run(id, stored);
        }
      }
    });
    db.exec(readFileSync(join(here, 'calibre-schema-after.sql'), 'utf8'));
  } finally {
    db.close();
  }
}

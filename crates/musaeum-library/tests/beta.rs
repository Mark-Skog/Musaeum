//! Phase 6, Part 2: many books changed at once, two works that are one book, a Calibre library
//! brought in without a byte of it changing, and the books' text searched.

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use common::{Fixture, epub, pdf_with_id};
use musaeum_library::{
    BatchEdit, Credit, DuplicateReason, Error, Identifier, Place, TextSource, TextUnit, UnitKind,
    WorkEdit, WorkId, read_calibre,
};
use rusqlite::{Connection, params};

fn refused<T: std::fmt::Debug>(result: musaeum_library::Result<T>) -> bool {
    matches!(result, Err(Error::Refused(_) | Error::NotFound(_)))
}

fn work_at(fixture: &Fixture, path: &Path) -> WorkId {
    fixture.instance(path).work_id
}

#[test]
fn many_works_change_at_once_and_only_what_was_set_becomes_the_person_s() {
    let fixture = Fixture::new();
    let paths: Vec<PathBuf> = (0..3)
        .map(|n| {
            fixture.write(
                &format!("lote-{n}.epub"),
                &epub(&format!("Livro {n}"), &["Autora"], None, 40 + n as u64),
            )
        })
        .collect();
    fixture.settle();
    let library = fixture.library();
    let works: Vec<WorkId> = paths.iter().map(|path| work_at(&fixture, path)).collect();
    let category = library
        .create_category(None, "Ciências", Place::Last)
        .unwrap();
    let other = library.create_category(None, "Artes", Place::Last).unwrap();
    let collection = library
        .create_collection("Para ler", "", Place::Last)
        .unwrap();
    library.set_category(&works[..1], other, true).unwrap();

    let changed = library
        .edit_works(
            &works,
            BatchEdit {
                status: Some("read".into()),
                rating: Some(Some(4)),
                language: Some(Some("PT-br".into())),
                add_categories: vec![category],
                remove_categories: vec![other],
                add_tags: vec!["Lote".into(), "  lote ".into()],
                add_collections: vec![collection],
                ..BatchEdit::default()
            },
        )
        .unwrap();
    assert_eq!(changed, 3);
    for (n, &work) in works.iter().enumerate() {
        let metadata = library.work_metadata(work).unwrap();
        assert_eq!(metadata.status, "read");
        assert_eq!(metadata.rating, Some(4));
        assert_eq!(metadata.language.as_deref(), Some("pt-BR"));
        assert_eq!(metadata.categories, [category]);
        assert_eq!(metadata.tag_names, ["Lote"], "one tag, however written");
        assert_eq!(metadata.collections, [collection]);
        assert_eq!(
            metadata.title,
            format!("Livro {n}"),
            "the title is not touched"
        );
        for field in ["status", "rating", "language"] {
            assert_eq!(
                metadata.provenance.get(field).map(String::as_str),
                Some("user"),
                "{field}"
            );
        }
        assert_ne!(
            metadata.provenance.get("title").map(String::as_str),
            Some("user")
        );
        assert_ne!(
            metadata.provenance.get("creators").map(String::as_str),
            Some("user")
        );
    }
    // In the collection, in the order they were given.
    let listed = library
        .browse(&musaeum_library::BrowseQuery {
            collection: Some(collection),
            limit: 10,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        listed
            .rows
            .iter()
            .map(|row| row.work_id)
            .collect::<Vec<_>>(),
        works
    );

    let summary = library.works_summary(&works).unwrap();
    assert_eq!(summary.works, 3);
    assert_eq!(summary.same.get("status"), Some(&serde_json::json!("read")));
    assert_eq!(
        summary.same.get("creators"),
        Some(&serde_json::json!([{"name": "Autora", "role": "author"}]))
    );
    assert_eq!(summary.categories.len(), 1);
    assert_eq!(summary.categories[0].works, 3);

    // Everything or nothing: a category that is not there undoes the rest.
    let before = library.work_metadata(works[0]).unwrap();
    assert!(refused(library.edit_works(
        &works,
        BatchEdit {
            status: Some("abandoned".into()),
            add_categories: vec![9999],
            ..BatchEdit::default()
        },
    )));
    assert_eq!(library.work_metadata(works[0]).unwrap(), before);
    assert!(refused(library.edit_works(&[], BatchEdit::default())));

    // People and the series for all, and a tag taken from all.
    let tag = library
        .tags()
        .unwrap()
        .into_iter()
        .find(|tag| tag.name == "Lote")
        .unwrap()
        .id;
    library
        .edit_works(
            &works[1..],
            BatchEdit {
                creators: Some(vec![Credit::author("Outra Autora")]),
                series: Some(Some("A Série".into())),
                remove_tags: vec![tag],
                ..BatchEdit::default()
            },
        )
        .unwrap();
    let second = library.work_metadata(works[1]).unwrap();
    assert_eq!(second.creators, [Credit::author("Outra Autora")]);
    assert_eq!(second.series.as_deref(), Some("A Série"));
    assert!(second.tags.is_empty());
    assert_eq!(
        library.work_metadata(works[0]).unwrap().tags,
        [tag],
        "the work left out keeps it"
    );
    let summary = library.works_summary(&works).unwrap();
    assert!(!summary.same.contains_key("creators"), "they differ now");
}

#[test]
fn two_works_that_are_one_book_become_one_with_all_that_was_on_them() {
    let fixture = Fixture::new();
    let a = fixture.write(
        "a/vida.epub",
        &epub(
            "Vida Maravilhosa",
            &["Stephen Jay Gould"],
            Some("urn:isbn:9788535902778"),
            1,
        ),
    );
    let b = fixture.write(
        "b/vida.pdf",
        &pdf_with_id("0123456789abcdef0123456789abcdef", 2),
    );
    let c = fixture.write(
        "c/vida-de-novo.epub",
        &epub("vida  maravilhosa", &["STEPHEN JAY GOULD"], None, 3),
    );
    let d = fixture.write(
        "d/outro.epub",
        &epub("Outro Livro", &["Outra Pessoa"], None, 4),
    );
    fixture.settle();
    let library = fixture.library();
    let (work_a, work_b, work_c, work_d) = (
        work_at(&fixture, &a),
        work_at(&fixture, &b),
        work_at(&fixture, &c),
        work_at(&fixture, &d),
    );
    // The PDF says nothing of itself; the person gives it the same ISBN.
    library
        .edit_work(
            work_b,
            WorkEdit {
                identifiers: Some(vec![Identifier {
                    scheme: "isbn".into(),
                    value: "978-85-359-0277-8".into(),
                }]),
                year: Some(Some(1990)),
                ..WorkEdit::default()
            },
        )
        .unwrap();
    let duplicates = library.duplicate_works(100).unwrap();
    let pair_ab = duplicates
        .iter()
        .find(|pair| pair.works == [work_a.min(work_b), work_a.max(work_b)])
        .expect("the same ISBN");
    assert_eq!(
        pair_ab.reason,
        DuplicateReason::SameIdentifier {
            scheme: "isbn".into(),
            value: "9788535902778".into()
        }
    );
    let pair_ac = duplicates
        .iter()
        .find(|pair| pair.works == [work_a.min(work_c), work_a.max(work_c)])
        .expect("the same title and author");
    assert!(matches!(
        pair_ac.reason,
        DuplicateReason::SameTitleAndAuthor { .. }
    ));
    assert!(!duplicates.iter().any(|pair| pair.works.contains(&work_d)));

    // A pair said to be two books is not suggested again.
    library.dismiss_duplicate(work_c, work_a).unwrap();
    assert!(
        !library
            .duplicate_works(100)
            .unwrap()
            .iter()
            .any(|pair| pair.works.contains(&work_c))
    );

    // What is on each: an annotation, categories, tags, a collection.
    let document_b = fixture.instance(&b).document_id;
    let note = library
        .add_annotation(document_b, "Uma nota no PDF")
        .unwrap();
    let science = library
        .create_category(None, "Ciência", Place::Last)
        .unwrap();
    let biology = library
        .create_category(Some(science), "Biologia", Place::Last)
        .unwrap();
    library.set_category(&[work_a], science, true).unwrap();
    library.set_category(&[work_b], biology, true).unwrap();
    let tag = library.create_tag("Fósseis", None).unwrap();
    library.set_tag(&[work_b], tag, true).unwrap();
    let collection = library
        .create_collection("Paleontologia", "", Place::Last)
        .unwrap();
    library
        .add_to_collection(collection, &[work_d, work_b], Place::Last)
        .unwrap();

    library.merge_works(work_a, &[work_b]).unwrap();
    let merged = library.work_metadata(work_a).unwrap();
    assert_eq!(merged.documents.len(), 2, "both documents, each whole");
    assert_eq!(merged.formats, ["epub", "pdf"]);
    assert_eq!(merged.title, "Vida Maravilhosa", "the kept work's");
    assert_eq!(merged.year, Some(1990), "what only the other had");
    assert_eq!(merged.categories, [science, biology]);
    assert_eq!(merged.tags, [tag]);
    assert_eq!(merged.collections, [collection]);
    assert_eq!(
        merged.provenance.get("identifiers").map(String::as_str),
        Some("user"),
        "the person's list came along"
    );
    assert!(
        library.work_metadata(work_b).is_err(),
        "the other work is gone"
    );
    let annotations = library.annotations(document_b, false).unwrap();
    assert!(
        annotations.iter().any(|annotation| annotation.id == note),
        "the note stays with its document"
    );
    assert_eq!(fixture.instance(&b).work_id, work_a);
    // In the collection, where the other was.
    let listed = library
        .browse(&musaeum_library::BrowseQuery {
            collection: Some(collection),
            limit: 10,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        listed
            .rows
            .iter()
            .map(|row| row.work_id)
            .collect::<Vec<_>>()
            .first(),
        Some(&work_d)
    );
    assert!(listed.rows.iter().any(|row| row.work_id == work_a));
    assert!(refused(library.merge_works(work_a, &[work_a])));
    assert!(refused(library.merge_works(work_a, &[9999])));
}

// --- A synthetic Calibre library ---

struct CalibreSpec<'a> {
    title: &'a str,
    authors: &'a [&'a str],
    series: Option<(&'a str, f64)>,
    tags: &'a [&'a str],
    rating: Option<i64>,
    identifiers: &'a [(&'a str, &'a str)],
    comments: Option<&'a str>,
    language: Option<&'a str>,
    pubdate: &'a str,
    /// The book's folder under the library, and its files: format and contents (None: listed, not on the disk).
    folder: &'a str,
    files: Vec<(&'a str, Option<Vec<u8>>)>,
    custom: &'a [(&'a str, serde_json::Value)],
}

/// The custom columns: label, name, datatype, normalized, display.
const COLUMNS: &[(&str, &str, &str, bool, &str)] = &[
    ("lido", "Lido", "bool", false, "{}"),
    (
        "genero",
        "Gênero",
        "enumeration",
        true,
        r#"{"enum_values": ["Romance", "Ensaio", "Poesia"]}"#,
    ),
    ("paginas", "Páginas", "int", false, "{}"),
    ("nota", "Nota", "comments", false, "{}"),
    ("emprestado", "Emprestado em", "datetime", false, "{}"),
    ("estrelas", "Estrelas", "rating", true, "{}"),
    (
        "resumo",
        "Resumo",
        "composite",
        false,
        r#"{"composite_template": "{title}"}"#,
    ),
];

fn write_calibre(dir: &Path, books: &[CalibreSpec<'_>]) {
    fs::create_dir_all(dir).unwrap();
    let connection = Connection::open(dir.join("metadata.db")).unwrap();
    connection
        .execute_batch(include_str!("../../../tools/lib/calibre-schema.sql"))
        .unwrap();
    for (label, name, datatype, normalized, display) in COLUMNS {
        connection
            .execute(
                "INSERT INTO custom_columns (label, name, datatype, display, normalized) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![label, name, datatype, display, normalized],
            )
            .unwrap();
        let id = connection.last_insert_rowid();
        if *datatype == "composite" {
            continue;
        }
        if *normalized {
            connection
                .execute_batch(&format!(
                    "CREATE TABLE custom_column_{id} (id INTEGER PRIMARY KEY, value TEXT NOT NULL, link TEXT NOT NULL DEFAULT '', UNIQUE (value));
                     CREATE TABLE books_custom_column_{id}_link (id INTEGER PRIMARY KEY, book INTEGER NOT NULL, value INTEGER NOT NULL, UNIQUE (book, value));"
                ))
                .unwrap();
        } else {
            connection
                .execute_batch(&format!("CREATE TABLE custom_column_{id} (id INTEGER PRIMARY KEY, book INTEGER, value, UNIQUE (book));"))
                .unwrap();
        }
    }
    for book in books {
        connection
            .execute(
                "INSERT INTO books (title, sort, pubdate, series_index, path, uuid) VALUES (?1, ?1, ?2, ?3, ?4, ?5)",
                params![book.title, book.pubdate, book.series.map_or(1.0, |(_, index)| index), book.folder, format!("uuid-{}", book.title.len())],
            )
            .unwrap();
        let id = connection.last_insert_rowid();
        for author in book.authors {
            connection
                .execute(
                    "INSERT OR IGNORE INTO authors (name, sort) VALUES (?1, ?1)",
                    [author],
                )
                .unwrap();
            connection
                .execute("INSERT INTO books_authors_link (book, author) SELECT ?1, id FROM authors WHERE name = ?2", params![id, author])
                .unwrap();
        }
        if let Some((series, _)) = book.series {
            connection
                .execute(
                    "INSERT OR IGNORE INTO series (name, sort) VALUES (?1, ?1)",
                    [series],
                )
                .unwrap();
            connection.execute("INSERT INTO books_series_link (book, series) SELECT ?1, id FROM series WHERE name = ?2", params![id, series]).unwrap();
        }
        for tag in book.tags {
            connection
                .execute("INSERT OR IGNORE INTO tags (name) VALUES (?1)", [tag])
                .unwrap();
            connection.execute("INSERT INTO books_tags_link (book, tag) SELECT ?1, id FROM tags WHERE name = ?2", params![id, tag]).unwrap();
        }
        if let Some(rating) = book.rating {
            connection
                .execute(
                    "INSERT OR IGNORE INTO ratings (rating) VALUES (?1)",
                    [rating],
                )
                .unwrap();
            connection.execute("INSERT INTO books_ratings_link (book, rating) SELECT ?1, id FROM ratings WHERE rating = ?2", params![id, rating]).unwrap();
        }
        if let Some(language) = book.language {
            connection
                .execute(
                    "INSERT OR IGNORE INTO languages (lang_code) VALUES (?1)",
                    [language],
                )
                .unwrap();
            connection
                .execute("INSERT INTO books_languages_link (book, lang_code) SELECT ?1, id FROM languages WHERE lang_code = ?2", params![id, language])
                .unwrap();
        }
        for (kind, value) in book.identifiers {
            connection
                .execute(
                    "INSERT INTO identifiers (book, type, val) VALUES (?1, ?2, ?3)",
                    params![id, kind, value],
                )
                .unwrap();
        }
        if let Some(comments) = book.comments {
            connection
                .execute(
                    "INSERT INTO comments (book, text) VALUES (?1, ?2)",
                    params![id, comments],
                )
                .unwrap();
        }
        let folder = book
            .folder
            .split('/')
            .fold(dir.to_path_buf(), |path, part| path.join(part));
        fs::create_dir_all(&folder).unwrap();
        for (format, contents) in &book.files {
            let name = format!(
                "{} - {}",
                book.title,
                book.authors.first().unwrap_or(&"Desconhecido")
            );
            connection
                .execute(
                    "INSERT INTO data (book, format, uncompressed_size, name) VALUES (?1, ?2, ?3, ?4)",
                    params![id, format.to_uppercase(), contents.as_ref().map_or(0, Vec::len) as i64, name],
                )
                .unwrap();
            if let Some(contents) = contents {
                fs::write(folder.join(format!("{name}.{format}")), contents).unwrap();
            }
        }
        // Calibre's backup of the book's metadata, and a cover: neither is a book.
        fs::write(folder.join("metadata.opf"), opf_of(book, id)).unwrap();
        fs::write(folder.join("cover.jpg"), b"\xFF\xD8\xFF\xD9").unwrap();
        for (label, value) in book.custom {
            let (column, datatype, normalized) = connection
                .query_row(
                    "SELECT id, datatype, normalized FROM custom_columns WHERE label = ?1",
                    [label],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, bool>(2)?,
                        ))
                    },
                )
                .unwrap();
            let sql_value = match value {
                serde_json::Value::Bool(flag) => rusqlite::types::Value::Integer(i64::from(*flag)),
                serde_json::Value::Number(number) => {
                    rusqlite::types::Value::Integer(number.as_i64().unwrap())
                }
                serde_json::Value::String(text) => rusqlite::types::Value::Text(text.clone()),
                _ => unreachable!(),
            };
            if normalized {
                connection
                    .execute(
                        &format!(
                            "INSERT OR IGNORE INTO custom_column_{column} (value) VALUES (?1)"
                        ),
                        [&sql_value],
                    )
                    .unwrap();
                connection
                    .execute(
                        &format!("INSERT INTO books_custom_column_{column}_link (book, value) SELECT ?1, id FROM custom_column_{column} WHERE value = ?2"),
                        params![id, sql_value],
                    )
                    .unwrap();
            } else {
                connection
                    .execute(
                        &format!(
                            "INSERT INTO custom_column_{column} (book, value) VALUES (?1, ?2)"
                        ),
                        params![id, sql_value],
                    )
                    .unwrap();
            }
            let _ = datatype;
        }
    }
    connection
        .execute_batch(include_str!("../../../tools/lib/calibre-schema-after.sql"))
        .unwrap();
    drop(connection);
    for leftover in ["metadata.db-journal", "metadata.db-wal", "metadata.db-shm"] {
        assert!(!dir.join(leftover).exists());
    }
}

fn opf_of(book: &CalibreSpec<'_>, id: i64) -> String {
    let mut metadata = format!(
        r#"<dc:identifier opf:scheme="calibre" id="calibre_id">{id}</dc:identifier><dc:title>{}</dc:title>"#,
        book.title
    );
    for author in book.authors {
        metadata.push_str(&format!(
            r#"<dc:creator opf:role="aut">{author}</dc:creator>"#
        ));
    }
    for tag in book.tags {
        metadata.push_str(&format!("<dc:subject>{tag}</dc:subject>"));
    }
    for (kind, value) in book.identifiers {
        metadata.push_str(&format!(
            r#"<dc:identifier opf:scheme="{}">{value}</dc:identifier>"#,
            kind.to_uppercase()
        ));
    }
    if let Some(comments) = book.comments {
        metadata.push_str(&format!(
            "<dc:description>{}</dc:description>",
            comments
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
        ));
    }
    if let Some(language) = book.language {
        metadata.push_str(&format!("<dc:language>{language}</dc:language>"));
    }
    metadata.push_str(&format!("<dc:date>{}</dc:date>", book.pubdate));
    if let Some((series, index)) = book.series {
        metadata.push_str(&format!(r#"<meta name="calibre:series" content="{series}"/><meta name="calibre:series_index" content="{index}"/>"#));
    }
    if let Some(rating) = book.rating {
        metadata.push_str(&format!(
            r#"<meta name="calibre:rating" content="{rating}"/>"#
        ));
    }
    for (label, value) in book.custom {
        let (_, name, datatype, _, display) =
            COLUMNS.iter().find(|column| column.0 == *label).unwrap();
        let meta = serde_json::json!({"datatype": datatype, "name": name, "display": serde_json::from_str::<serde_json::Value>(display).unwrap(), "#value#": value, "is_multiple": {}});
        metadata.push_str(&format!(
            r#"<meta name="calibre:user_metadata:#{label}" content="{}"/>"#,
            meta.to_string()
                .replace('&', "&amp;")
                .replace('"', "&quot;")
        ));
    }
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" unique-identifier="uuid_id" version="2.0">
<metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">{metadata}</metadata>
</package>"#
    )
}

/// Every file under a folder: its size, when it changed and its bytes' hash.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, (u64, std::time::SystemTime, [u8; 32])> {
    let mut found = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(folder) = stack.pop() {
        for entry in fs::read_dir(&folder).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let metadata = entry.metadata().unwrap();
            if metadata.is_dir() {
                stack.push(path);
            } else {
                let bytes = fs::read(&path).unwrap();
                found.insert(
                    path.strip_prefix(dir).unwrap().to_path_buf(),
                    (
                        metadata.len(),
                        metadata.modified().unwrap(),
                        *blake3::hash(&bytes).as_bytes(),
                    ),
                );
            }
        }
    }
    found
}

fn cortico(epub_bytes: Vec<u8>, pdf_bytes: Vec<u8>) -> CalibreSpec<'static> {
    CalibreSpec {
        title: "O Cortiço",
        authors: &["Aluísio Azevedo"],
        series: Some(("Naturalismo", 2.0)),
        tags: &["Romance", "Clássico"],
        rating: Some(7),
        identifiers: &[("isbn", "978-85-359-0277-8"), ("goodreads", "12345")],
        comments: Some("<div><p>Um <b>cortiço</b> no Rio.</p><p>Segunda parte.</p></div>"),
        language: Some("por"),
        pubdate: "1890-01-01 00:00:00+00:00",
        folder: "Aluisio Azevedo/O Cortico (1)",
        files: vec![("epub", Some(epub_bytes)), ("pdf", Some(pdf_bytes))],
        custom: &[],
    }
}

#[test]
fn a_calibre_library_comes_in_and_not_a_byte_of_it_changes() {
    let fixture = Fixture::new();
    let calibre = fixture.root.join("Calibre");
    // A book whose bytes the library already has, elsewhere.
    let memorias = epub("memorias-arquivo", &["Ninguém"], None, 77);
    let loose = fixture.write("soltos/memorias.epub", &memorias);
    fixture.settle();
    let library = fixture.library();
    let loose_work = work_at(&fixture, &loose);
    let documents_before = fixture.document_count();
    library.create_custom_field("Páginas", "text", &[]).unwrap();

    write_calibre(
        &calibre,
        &[
            CalibreSpec {
                custom: &[
                    ("lido", serde_json::json!(true)),
                    ("genero", serde_json::json!("Romance")),
                    ("paginas", serde_json::json!(312)),
                    ("nota", serde_json::json!("<p>Lido na <i>escola</i>.</p>")),
                    ("emprestado", serde_json::json!("2024-03-01 10:00:00+00:00")),
                    ("estrelas", serde_json::json!(8)),
                ],
                ..cortico(
                    epub("cortico-arquivo", &["Autor do Arquivo"], None, 10),
                    pdf_with_id("00112233445566778899aabbccddeeff", 11),
                )
            },
            CalibreSpec {
                title: "Memórias Póstumas de Brás Cubas",
                authors: &["Machado de Assis"],
                series: None,
                tags: &["Romance"],
                rating: Some(10),
                identifiers: &[],
                comments: None,
                language: Some("por"),
                pubdate: "0101-01-01 00:00:00+00:00",
                folder: "Machado de Assis/Memorias Postumas (2)",
                files: vec![("epub", Some(memorias.clone()))],
                custom: &[],
            },
            CalibreSpec {
                title: "Livro Perdido",
                authors: &["Ninguém"],
                series: None,
                tags: &[],
                rating: None,
                identifiers: &[],
                comments: None,
                language: None,
                pubdate: "2001-01-01 00:00:00+00:00",
                folder: "Ninguem/Livro Perdido (3)",
                files: vec![("epub", None)],
                custom: &[],
            },
        ],
    );
    let before = snapshot(&calibre);

    let read = read_calibre(&calibre).unwrap();
    assert_eq!(read.read_from, "metadata.db");
    assert_eq!(read.books.len(), 3);
    assert_eq!(read.present_files(), 3);
    let resumo = read
        .columns
        .iter()
        .find(|column| column.label == "resumo")
        .unwrap();
    assert_eq!(
        resumo.kind, None,
        "a composite column is worked out by Calibre, not brought in"
    );

    // Before the files are read, the books wait.
    let report = library.import_calibre(&read).unwrap();
    assert_eq!((report.imported, report.waiting, report.missing), (0, 2, 1));
    // The columns' fields are made at once, books waiting or not.
    assert_eq!(
        report.fields_created, 6,
        "Páginas exists with another kind, so it is Páginas (Calibre); the composite is none"
    );
    fixture.settle();
    let report = library.import_calibre(&read).unwrap();
    assert_eq!(
        (
            report.books,
            report.imported,
            report.waiting,
            report.missing
        ),
        (3, 2, 0, 1)
    );
    assert_eq!(report.merged, 1, "the EPUB and the PDF are one book");
    assert_eq!(report.fields_created, 0);

    let epub_path = calibre.join("Aluisio Azevedo/O Cortico (1)/O Cortiço - Aluísio Azevedo.epub");
    let work = work_at(&fixture, &epub_path);
    assert_eq!(
        work_at(
            &fixture,
            &calibre.join("Aluisio Azevedo/O Cortico (1)/O Cortiço - Aluísio Azevedo.pdf")
        ),
        work
    );
    let metadata = library.work_metadata(work).unwrap();
    assert_eq!(metadata.title, "O Cortiço");
    assert_eq!(metadata.creators, [Credit::author("Aluísio Azevedo")]);
    assert_eq!(metadata.series.as_deref(), Some("Naturalismo"));
    assert_eq!(metadata.series_position, Some(2.0));
    assert_eq!(
        metadata.rating,
        Some(4),
        "seven half stars round up to four"
    );
    assert_eq!(metadata.language.as_deref(), Some("pt"));
    assert_eq!(metadata.year, Some(1890));
    assert_eq!(
        metadata.description.as_deref(),
        Some("Um cortiço no Rio.\n\nSegunda parte.")
    );
    assert_eq!(
        metadata.identifiers,
        [
            Identifier {
                scheme: "isbn".into(),
                value: "9788535902778".into()
            },
            Identifier {
                scheme: "other".into(),
                value: "goodreads:12345".into()
            },
        ]
    );
    assert_eq!(metadata.tag_names, ["Clássico", "Romance"]);
    for field in [
        "title",
        "creators",
        "series",
        "rating",
        "language",
        "year",
        "description",
        "identifiers",
    ] {
        assert_eq!(
            metadata.provenance.get(field).map(String::as_str),
            Some("calibre"),
            "{field}"
        );
    }
    let fields = library.custom_fields().unwrap();
    let field = |name: &str| {
        fields
            .iter()
            .find(|field| field.name == name)
            .unwrap_or_else(|| panic!("{name}"))
    };
    assert_eq!(field("Gênero").kind, "choice");
    assert_eq!(field("Gênero").choices, ["Romance", "Ensaio", "Poesia"]);
    assert_eq!(field("Páginas (Calibre)").kind, "number");
    let value = |name: &str| {
        metadata
            .custom
            .iter()
            .find(|value| value.field_id == field(name).id)
            .map(|value| value.value.clone())
    };
    assert_eq!(value("Lido"), Some(serde_json::json!(true)));
    assert_eq!(value("Gênero"), Some(serde_json::json!("Romance")));
    assert_eq!(value("Páginas (Calibre)"), Some(serde_json::json!(312.0)));
    assert_eq!(value("Nota"), Some(serde_json::json!("Lido na escola.")));
    assert_eq!(
        value("Emprestado em"),
        Some(serde_json::json!("2024-03-01"))
    );
    assert_eq!(value("Estrelas"), Some(serde_json::json!(4.0)));

    // The same bytes the library had: that book, not a second one.
    let memorias_path = calibre.join("Machado de Assis/Memorias Postumas (2)/Memórias Póstumas de Brás Cubas - Machado de Assis.epub");
    assert_eq!(work_at(&fixture, &memorias_path), loose_work);
    assert_eq!(
        fixture.document_count(),
        documents_before + 2,
        "the Cortiço's two files are documents; Memórias is not new"
    );
    let memorias_work = library.work_metadata(loose_work).unwrap();
    assert_eq!(memorias_work.title, "Memórias Póstumas de Brás Cubas");
    // Calibre's undefined date sets no year: the one the file gives stays, and stays the file's.
    assert_eq!(memorias_work.year, Some(1899));
    assert_eq!(
        memorias_work.provenance.get("year").map(String::as_str),
        Some("embedded")
    );

    // Again: nothing doubles.
    let again = library.import_calibre(&read).unwrap();
    assert_eq!(
        (again.imported, again.merged, again.fields_created),
        (2, 0, 0)
    );
    assert_eq!(
        library.work_metadata(work).unwrap().tag_names,
        ["Clássico", "Romance"]
    );
    assert_eq!(library.calibre_books(&calibre).unwrap().len(), 2);

    // What the person sets stays, a second import included.
    library
        .edit_work(
            work,
            WorkEdit {
                title: Some("Meu Cortiço".into()),
                ..WorkEdit::default()
            },
        )
        .unwrap();
    library.import_calibre(&read).unwrap();
    assert_eq!(library.work_metadata(work).unwrap().title, "Meu Cortiço");

    assert_eq!(
        snapshot(&calibre),
        before,
        "the Calibre library is as it was"
    );
    for leftover in ["metadata.db-journal", "metadata.db-wal", "metadata.db-shm"] {
        assert!(!calibre.join(leftover).exists());
    }

    // A new edition of the file, with its own title, people, year and language, writes over
    // nothing that came from Calibre (the test changes the file itself, after the check above).
    fs::write(&epub_path, epub("Outro Título", &["Outro Autor"], None, 99)).unwrap();
    fixture.settle();
    let after = library.work_metadata(work).unwrap();
    assert_eq!(after.title, "Meu Cortiço");
    assert_eq!(after.creators, [Credit::author("Aluísio Azevedo")]);
    assert_eq!(after.year, Some(1890));
    assert_eq!(after.language.as_deref(), Some("pt"));
}

#[test]
fn a_calibre_library_being_written_is_not_read() {
    let fixture = Fixture::new();
    let calibre = fixture.root.join("Calibre");
    write_calibre(
        &calibre,
        &[cortico(epub("x", &["y"], None, 1), pdf_with_id("00", 2))],
    );
    fs::write(calibre.join("metadata.db-journal"), b"").unwrap();
    assert!(refused(read_calibre(&calibre)));
    fs::remove_file(calibre.join("metadata.db-journal")).unwrap();
    assert!(read_calibre(&calibre).is_ok());
    assert!(refused(read_calibre(&fixture.root.join("nada"))));
}

#[test]
fn without_a_database_the_books_opf_files_are_read() {
    let fixture = Fixture::new();
    let calibre = fixture.root.join("Calibre");
    write_calibre(
        &calibre,
        &[CalibreSpec {
            custom: &[
                ("genero", serde_json::json!("Romance")),
                ("lido", serde_json::json!(false)),
            ],
            ..cortico(
                epub("cortico-arquivo", &["Autor do Arquivo"], None, 10),
                pdf_with_id("00112233445566778899aabbccddeeff", 11),
            )
        }],
    );
    fs::remove_file(calibre.join("metadata.db")).unwrap();
    let read = read_calibre(&calibre).unwrap();
    assert_eq!(read.read_from, "metadata.opf");
    let book = &read.books[0];
    assert_eq!(book.id, 1);
    assert_eq!(book.title, "O Cortiço");
    assert_eq!(book.authors, ["Aluísio Azevedo"]);
    assert_eq!(book.series.as_deref(), Some("Naturalismo"));
    assert_eq!(book.series_index, Some(2.0));
    assert_eq!(book.rating, Some(4));
    assert_eq!(book.language.as_deref(), Some("pt"));
    assert_eq!(
        book.description.as_deref(),
        Some("Um cortiço no Rio.\n\nSegunda parte.")
    );
    assert_eq!(
        book.files.len(),
        2,
        "the EPUB and the PDF; not the cover nor the OPF"
    );
    assert_eq!(
        book.custom.get("genero"),
        Some(&serde_json::json!("Romance"))
    );
    assert_eq!(book.custom.get("lido"), Some(&serde_json::json!(false)));
    fixture.settle();
    let report = fixture.library().import_calibre(&read).unwrap();
    assert_eq!((report.imported, report.merged), (1, 1));
}

#[test]
fn the_books_text_is_found_and_only_where_a_file_has_it_now() {
    let fixture = Fixture::new();
    let a = fixture.write("a.epub", &epub("Vida", &["Gould"], None, 1));
    let b = fixture.write("b.epub", &epub("Outro", &["Outra"], None, 2));
    fixture.settle();
    let library = fixture.library();
    let pending = library.fulltext_pending(100).unwrap();
    assert_eq!(pending.len(), 2);
    let unit = |index: u32, text: &str| TextUnit {
        kind: UnitKind::Document,
        index,
        label: Some(format!("Capítulo {}", index + 1)),
        source: TextSource::Flow,
        text: text.to_owned(),
    };
    let hash_a = pending
        .iter()
        .find(|item| item.file.path == a)
        .unwrap()
        .blake3;
    let hash_b = pending
        .iter()
        .find(|item| item.file.path == b)
        .unwrap()
        .blake3;
    library
        .fulltext_store(
            &hash_a,
            &[
                unit(0, "O xisto de Burgess."),
                unit(1, "Hallucigenia andava sobre espinhos, dizem."),
            ],
            0,
        )
        .unwrap();
    library.fulltext_store(&hash_b, &[], 0).unwrap();
    assert!(library.fulltext_pending(100).unwrap().is_empty());
    let status = library.fulltext_status().unwrap();
    assert_eq!(
        (status.indexed, status.empty, status.pending, status.units),
        (1, 1, 0, 2)
    );

    let found = library
        .fulltext_search("HALLUCIGENIA espinhos", 10)
        .unwrap();
    assert_eq!(found.books.len(), 1);
    let hit = &found.books[0];
    assert_eq!(hit.works, [work_at(&fixture, &a)]);
    assert_eq!(hit.units[0].index, 1);
    assert_eq!(hit.units[0].label.as_deref(), Some("Capítulo 2"));
    assert_eq!(hit.units[0].position, 0);
    assert!(hit.units[0].snippet.contains("espinhos"));
    assert!(
        library
            .fulltext_search("hallucigenia baleia", 10)
            .unwrap()
            .books
            .is_empty(),
        "every word"
    );
    assert!(library.fulltext_search("   ", 10).unwrap().books.is_empty());

    // Gone from the disk, gone from the results; then from the index.
    fs::remove_file(&a).unwrap();
    fixture.settle();
    assert!(
        library
            .fulltext_search("hallucigenia", 10)
            .unwrap()
            .books
            .is_empty()
    );
    assert_eq!(library.fulltext_forget_gone().unwrap(), 1);
    assert_eq!(library.fulltext_status().unwrap().units, 0);
}

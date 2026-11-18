//! How a person organizes the library beside its folders (Phase 6): the category tree, tags,
//! collections kept by hand and by a rule, custom fields, and a work's metadata as the editor
//! changes it, kept from whatever is read from its files afterwards.

mod common;

use std::fs;

use common::{Fixture, epub, pdf};
use musaeum_library::{
    BrowseQuery, Credit, CustomValue, Error, Identifier, MAX_CATEGORY_DEPTH, Place, SmartMatch,
    SmartRule, SmartRules, WorkEdit, WorkId,
};
use serde_json::json;

fn titles(fixture: &Fixture, query: BrowseQuery) -> Vec<String> {
    fixture
        .library()
        .browse(&BrowseQuery {
            limit: 100,
            ..query
        })
        .unwrap()
        .rows
        .into_iter()
        .map(|row| row.title)
        .collect()
}

fn work_of(fixture: &Fixture, title: &str) -> WorkId {
    fixture
        .library()
        .browse(&BrowseQuery {
            limit: 100,
            ..BrowseQuery::default()
        })
        .unwrap()
        .rows
        .into_iter()
        .find(|row| row.title == title)
        .unwrap_or_else(|| panic!("no book {title}"))
        .work_id
}

/// Four books in two folders.
fn shelf() -> Fixture {
    let fixture = Fixture::new();
    fixture.write(
        "ciencia/vida.epub",
        &epub(
            "Vida Maravilhosa",
            &["Stephen Jay Gould"],
            Some("urn:isbn:9788535902778"),
            1,
        ),
    );
    fixture.write(
        "ciencia/cosmos.epub",
        &epub("Cosmos", &["Carl Sagan"], None, 2),
    );
    fixture.write(
        "literatura/dom.epub",
        &epub("Dom Casmurro", &["Machado de Assis"], None, 3),
    );
    fixture.write(
        "literatura/historia.epub",
        &epub("História da Arte", &["Ernst Gombrich"], None, 4),
    );
    fixture.settle();
    fixture
}

fn refused<T: std::fmt::Debug>(result: musaeum_library::Result<T>) -> bool {
    matches!(result, Err(Error::Refused(_)))
}

#[test]
fn categories_are_a_tree_in_the_person_s_order() {
    let fixture = shelf();
    let library = fixture.library();
    let sciences = library
        .create_category(None, "Ciências", Place::Last)
        .unwrap();
    let letters = library
        .create_category(None, "Literatura", Place::Last)
        .unwrap();
    // Before Literatura, but after Ciências.
    let arts = library
        .create_category(None, "Artes", Place::Before(letters))
        .unwrap();
    let physics = library
        .create_category(Some(sciences), "Física", Place::Last)
        .unwrap();
    let biology = library
        .create_category(Some(sciences), "Biologia", Place::First)
        .unwrap();
    let quantum = library
        .create_category(Some(physics), "Quântica", Place::Last)
        .unwrap();
    let tree: Vec<(String, usize)> = library
        .categories()
        .unwrap()
        .into_iter()
        .map(|category| (category.name, category.depth))
        .collect();
    assert_eq!(
        tree,
        [
            ("Ciências".to_owned(), 0),
            ("Biologia".to_owned(), 1),
            ("Física".to_owned(), 1),
            ("Quântica".to_owned(), 2),
            ("Artes".to_owned(), 0),
            ("Literatura".to_owned(), 0),
        ]
    );
    // Siblings may not share a name, however it is written; cousins may.
    assert!(refused(library.create_category(
        Some(sciences),
        "  fisica ",
        Place::Last
    )));
    assert!(refused(library.rename_category(biology, "FÍSICA")));
    library
        .create_category(Some(letters), "Física", Place::Last)
        .unwrap();
    assert!(refused(library.create_category(None, "   ", Place::Last)));
    assert!(matches!(
        library.create_category(Some(9999), "x", Place::Last),
        Err(Error::NotFound(_))
    ));
    library.rename_category(arts, "Artes Visuais").unwrap();

    // A category never goes under itself, or under one of its own.
    assert!(refused(library.move_category(
        sciences,
        Some(quantum),
        Place::Last
    )));
    assert!(refused(library.move_category(
        physics,
        Some(physics),
        Place::Last
    )));
    let _ = biology;
    // Física, with Quântica under it, moves under Artes Visuais.
    library
        .move_category(physics, Some(arts), Place::First)
        .unwrap();
    // A sibling named Física already exists under Literatura: moving there is refused.
    assert!(refused(library.move_category(
        physics,
        Some(letters),
        Place::Last
    )));
    let moved = library.categories().unwrap();
    let quantum_now = moved
        .iter()
        .find(|category| category.id == quantum)
        .unwrap();
    assert_eq!(quantum_now.depth, 2);
    let physics_now = moved
        .iter()
        .find(|category| category.id == physics)
        .unwrap();
    assert_eq!(physics_now.parent_id, Some(arts));
}

#[test]
fn the_tree_goes_sixteen_levels_deep_and_no_further() {
    let fixture = shelf();
    let library = fixture.library();
    let mut chain = vec![
        library
            .create_category(None, "Nível 0", Place::Last)
            .unwrap(),
    ];
    for level in 1..MAX_CATEGORY_DEPTH {
        let parent = *chain.last().unwrap();
        let id = library
            .create_category(Some(parent), &format!("Nível {level}"), Place::Last)
            .unwrap();
        chain.push(id);
    }
    assert_eq!(chain.len(), 16);
    // A seventeenth level is refused.
    let deepest = *chain.last().unwrap();
    assert!(refused(library.create_category(
        Some(deepest),
        "Nível 16",
        Place::Last
    )));
    // A category with one under it fits below the fourteenth level, not below the fifteenth: its
    // own would be the seventeenth. The refused move changes nothing.
    let loose = library.create_category(None, "Solta", Place::Last).unwrap();
    let under = library
        .create_category(Some(loose), "Filha", Place::Last)
        .unwrap();
    assert!(refused(library.move_category(
        loose,
        Some(chain[14]),
        Place::Last
    )));
    let after = library.categories().unwrap();
    let depth = |id| {
        after
            .iter()
            .find(|category| category.id == id)
            .unwrap()
            .depth
    };
    assert_eq!((depth(loose), depth(under)), (0, 1));
    library
        .move_category(loose, Some(chain[13]), Place::Last)
        .unwrap();
    let moved = library.categories().unwrap();
    let depth = |id| {
        moved
            .iter()
            .find(|category| category.id == id)
            .unwrap()
            .depth
    };
    assert_eq!((depth(loose), depth(under)), (14, 15));
    assert!(
        moved
            .iter()
            .all(|category| category.depth < MAX_CATEGORY_DEPTH)
    );
}

#[test]
fn a_category_lists_the_books_of_those_under_it_too() {
    let fixture = shelf();
    let library = fixture.library();
    let sciences = library
        .create_category(None, "Ciências", Place::Last)
        .unwrap();
    let biology = library
        .create_category(Some(sciences), "Biologia", Place::Last)
        .unwrap();
    let vida = work_of(&fixture, "Vida Maravilhosa");
    let cosmos = work_of(&fixture, "Cosmos");
    library.set_category(&[vida], biology, true).unwrap();
    library.set_category(&[cosmos], sciences, true).unwrap();
    let in_sciences = titles(
        &fixture,
        BrowseQuery {
            category: Some(sciences),
            ..BrowseQuery::default()
        },
    );
    assert_eq!(in_sciences, ["Cosmos", "Vida Maravilhosa"]);
    let counts = library.categories().unwrap();
    assert_eq!(counts[0].works, 1);
    assert_eq!(counts[0].total, 2);
    // With a search, and with a folder: both have to hold.
    let both = titles(
        &fixture,
        BrowseQuery {
            text: "vida".into(),
            category: Some(sciences),
            ..BrowseQuery::default()
        },
    );
    assert_eq!(both, ["Vida Maravilhosa"]);

    // Taken out of the tree, the categories go; the books stay, in the library.
    library.delete_category(sciences).unwrap();
    assert!(library.categories().unwrap().is_empty());
    assert_eq!(
        fixture
            .library()
            .browse(&BrowseQuery {
                limit: 10,
                ..BrowseQuery::default()
            })
            .unwrap()
            .total,
        4
    );
    assert!(library.work_metadata(vida).unwrap().categories.is_empty());
}

#[test]
fn tags_are_shared_by_name_and_collections_keep_their_order() {
    let fixture = shelf();
    let library = fixture.library();
    let read = library.create_tag("A reler", Some("green")).unwrap();
    assert!(refused(library.create_tag("a  RELER", None)));
    assert!(refused(library.create_tag("x", Some("magenta"))));
    let vida = work_of(&fixture, "Vida Maravilhosa");
    let dom = work_of(&fixture, "Dom Casmurro");
    let cosmos = work_of(&fixture, "Cosmos");
    library.set_tag(&[vida, dom], read, true).unwrap();
    assert_eq!(
        titles(
            &fixture,
            BrowseQuery {
                tag: Some(read),
                ..BrowseQuery::default()
            }
        ),
        ["Dom Casmurro", "Vida Maravilhosa"]
    );
    library.update_tag(read, Some("Reler"), Some(None)).unwrap();
    let tags = library.tags().unwrap();
    assert_eq!(
        (
            tags[0].name.as_str(),
            tags[0].color.as_deref(),
            tags[0].works
        ),
        ("Reler", None, 2)
    );

    let shelf = library
        .create_collection("Para as férias", "Leituras leves", Place::Last)
        .unwrap();
    library
        .add_to_collection(shelf, &[vida, dom], Place::Last)
        .unwrap();
    library
        .add_to_collection(shelf, &[cosmos], Place::Before(dom))
        .unwrap();
    // Listed in the collection's order, not by title.
    assert_eq!(
        titles(
            &fixture,
            BrowseQuery {
                collection: Some(shelf),
                ..BrowseQuery::default()
            }
        ),
        ["Vida Maravilhosa", "Cosmos", "Dom Casmurro"]
    );
    // Moving one changes only its place.
    library
        .add_to_collection(shelf, &[dom], Place::First)
        .unwrap();
    assert_eq!(
        library.collection_works(shelf).unwrap(),
        [dom, vida, cosmos]
    );
    library.remove_from_collection(shelf, &[vida]).unwrap();
    assert_eq!(library.collection_works(shelf).unwrap(), [dom, cosmos]);
    library.delete_tag(read).unwrap();
    assert!(library.work_metadata(vida).unwrap().tags.is_empty());
    library.delete_collection(shelf).unwrap();
    assert!(library.collections().unwrap().is_empty());
}

fn rules(matching: SmartMatch, rules: &[(&str, &str, serde_json::Value)]) -> SmartRules {
    SmartRules {
        matching,
        rules: rules
            .iter()
            .map(|(field, op, value)| SmartRule {
                field: (*field).to_owned(),
                op: (*op).to_owned(),
                value: value.clone(),
            })
            .collect(),
    }
}

#[test]
fn a_smart_collection_finds_what_its_rule_says_now() {
    let fixture = shelf();
    fixture.write("avulso/manual.pdf", &pdf(9));
    fixture.settle();
    let library = fixture.library();
    let lent = library
        .create_custom_field(
            "Situação",
            "choice",
            &["em casa".into(), "emprestado".into()],
        )
        .unwrap();
    let vida = work_of(&fixture, "Vida Maravilhosa");
    let historia = work_of(&fixture, "História da Arte");
    library
        .edit_work(
            vida,
            WorkEdit {
                year: Some(Some(1990)),
                rating: Some(Some(5)),
                custom: Some(vec![CustomValue {
                    field_id: lent,
                    value: json!("emprestado"),
                }]),
                ..WorkEdit::default()
            },
        )
        .unwrap();
    library
        .edit_work(
            historia,
            WorkEdit {
                year: Some(Some(1950)),
                ..WorkEdit::default()
            },
        )
        .unwrap();
    let find = |rule: SmartRules| -> Vec<String> {
        let id = library
            .create_smart_collection("teste", &rule, Place::Last)
            .unwrap();
        let found = titles(
            &fixture,
            BrowseQuery {
                smart: Some(id),
                ..BrowseQuery::default()
            },
        );
        library.delete_smart_collection(id).unwrap();
        found
    };
    // Folded: no case, no accents.
    assert_eq!(
        find(rules(
            SmartMatch::All,
            &[("title", "contains", json!("historia"))]
        )),
        ["História da Arte"]
    );
    assert_eq!(
        find(rules(
            SmartMatch::All,
            &[("creator", "contains", json!("GOULD"))]
        )),
        ["Vida Maravilhosa"]
    );
    assert_eq!(
        find(rules(
            SmartMatch::All,
            &[("year", "between", json!([1900, 2000]))]
        )),
        ["História da Arte", "Vida Maravilhosa"]
    );
    assert_eq!(
        find(rules(
            SmartMatch::All,
            &[
                ("year", "between", json!([1900, 2000])),
                ("rating", "ge", json!(4))
            ]
        )),
        ["Vida Maravilhosa"]
    );
    assert_eq!(
        find(rules(
            SmartMatch::Any,
            &[
                ("year", "between", json!([1940, 1960])),
                (&format!("custom:{lent}"), "is", json!("EMPRESTADO"))
            ]
        )),
        ["História da Arte", "Vida Maravilhosa"]
    );
    assert_eq!(
        find(rules(SmartMatch::All, &[("format", "is", json!("pdf"))])).len(),
        1
    );
    assert_eq!(
        find(rules(SmartMatch::All, &[("year", "empty", json!(null))])).len(),
        1,
        "the EPUBs say a year, the PDF does not"
    );
    assert_eq!(
        find(rules(
            SmartMatch::All,
            &[("identifier", "is", json!("9788535902778"))]
        )),
        ["Vida Maravilhosa"],
        "the ISBN the file gave"
    );
    assert_eq!(
        find(rules(SmartMatch::All, &[("status", "is", json!("unread"))])).len(),
        5
    );
    // A tag and a category by id; the folder takes the folders below it.
    let tag = library.create_tag("Favoritos", None).unwrap();
    library.set_tag(&[historia], tag, true).unwrap();
    assert_eq!(
        find(rules(
            SmartMatch::All,
            &[
                ("tag", "not_in", json!(tag)),
                ("format", "is", json!("epub"))
            ]
        )),
        ["Cosmos", "Dom Casmurro", "Vida Maravilhosa"]
    );
    let folder = library
        .folders()
        .unwrap()
        .into_iter()
        .find(|folder| folder.name == "literatura")
        .unwrap()
        .id;
    assert_eq!(
        find(rules(SmartMatch::All, &[("folder", "in", json!(folder))])),
        ["Dom Casmurro", "História da Arte"]
    );
    // A rule that does not hold together is refused before it is kept.
    assert!(refused(library.create_smart_collection(
        "x",
        &rules(SmartMatch::All, &[("year", "contains", json!("19"))]),
        Place::Last
    )));
    assert!(refused(library.create_smart_collection(
        "x",
        &rules(SmartMatch::All, &[("custom:999", "is", json!("a"))]),
        Place::Last
    )));
    // A field a rule names is removed: the rule says so, and finds nothing.
    let kept = library
        .create_smart_collection(
            "Emprestados",
            &rules(
                SmartMatch::All,
                &[(&format!("custom:{lent}"), "is", json!("emprestado"))],
            ),
            Place::Last,
        )
        .unwrap();
    assert_eq!(library.smart_collections().unwrap()[0].works, 1);
    library.delete_custom_field(lent).unwrap();
    let broken = library.smart_collections().unwrap();
    assert!(broken[0].broken.is_some(), "{broken:?}");
    assert!(
        fixture
            .library()
            .browse(&BrowseQuery {
                smart: Some(kept),
                limit: 10,
                ..BrowseQuery::default()
            })
            .is_err()
    );
}

#[test]
fn custom_fields_hold_values_of_their_kind() {
    let fixture = shelf();
    let library = fixture.library();
    let pages = library
        .create_custom_field("Páginas", "number", &[])
        .unwrap();
    let bought = library
        .create_custom_field("Comprado em", "date", &[])
        .unwrap();
    let signed = library
        .create_custom_field("Autografado", "boolean", &[])
        .unwrap();
    let link = library.create_custom_field("Resenha", "url", &[]).unwrap();
    let shelf = library
        .create_custom_field("Estante", "choice", &["sala".into(), "quarto".into()])
        .unwrap();
    assert!(refused(library.create_custom_field("páginas", "text", &[])));
    assert!(refused(library.create_custom_field("x", "color", &[])));
    assert!(refused(library.create_custom_field("y", "choice", &[])));
    let vida = work_of(&fixture, "Vida Maravilhosa");
    let set = |field, value: serde_json::Value| {
        library.edit_work(
            vida,
            WorkEdit {
                custom: Some(vec![CustomValue {
                    field_id: field,
                    value,
                }]),
                ..WorkEdit::default()
            },
        )
    };
    set(pages, json!(402)).unwrap();
    set(bought, json!("2024-02-29")).unwrap();
    set(signed, json!(true)).unwrap();
    set(link, json!("https://example.org/resenha")).unwrap();
    set(shelf, json!("sala")).unwrap();
    assert!(refused(set(pages, json!("muitas"))));
    assert!(refused(set(bought, json!("2023-02-29"))));
    assert!(refused(set(link, json!("javascript:alert(1)"))));
    assert!(refused(set(shelf, json!("cozinha"))));
    let values = library.work_metadata(vida).unwrap().custom;
    assert_eq!(values.len(), 5);
    assert!(values.contains(&CustomValue {
        field_id: signed,
        value: json!(true)
    }));
    assert!(values.contains(&CustomValue {
        field_id: pages,
        value: json!(402.0)
    }));
    // A choice a work has stays among the choices.
    assert!(refused(library.update_custom_field(
        shelf,
        None,
        Some(&["quarto".into()])
    )));
    library
        .update_custom_field(
            shelf,
            Some("Onde está"),
            Some(&["sala".into(), "escritório".into()]),
        )
        .unwrap();
    set(signed, json!(null)).unwrap();
    assert_eq!(library.work_metadata(vida).unwrap().custom.len(), 4);
    library.delete_custom_field(pages).unwrap();
    assert_eq!(library.work_metadata(vida).unwrap().custom.len(), 3);
}

#[test]
fn what_a_person_edits_stays_however_the_file_is_read_again() {
    let fixture = Fixture::new();
    let path = fixture.write(
        "livro.epub",
        &epub(
            "Título do Arquivo",
            &["Autora Um"],
            Some("urn:isbn:9788535902778"),
            1,
        ),
    );
    fixture.settle();
    let document = fixture.document_at(&path);
    let work = document.work_id;
    let library = fixture.library();
    let before = library.work_metadata(work).unwrap();
    assert_eq!(
        before.identifiers,
        [Identifier {
            scheme: "isbn".into(),
            value: "9788535902778".into()
        }],
        "the file's ISBN, written as one"
    );
    assert_eq!(
        before.provenance.get("title").map(String::as_str),
        Some("embedded")
    );

    let category = library
        .create_category(None, "Ensaios", Place::Last)
        .unwrap();
    let collection = library.create_collection("Clube", "", Place::Last).unwrap();
    library
        .edit_work(
            work,
            WorkEdit {
                title: Some("  Meu Título  ".into()),
                creators: Some(vec![
                    Credit {
                        name: "Tradutora".into(),
                        role: "translator".into(),
                    },
                    Credit::author("Autora Um"),
                ]),
                series: Some(Some("Uma Série".into())),
                series_position: Some(Some(2.5)),
                year: Some(Some(1999)),
                language: Some(Some("PT-br".into())),
                identifiers: Some(vec![
                    Identifier {
                        scheme: "isbn".into(),
                        value: "0-306-40615-2".into(),
                    },
                    Identifier {
                        scheme: "doi".into(),
                        value: "10.1000/xyz".into(),
                    },
                ]),
                rating: Some(Some(4)),
                status: Some("reading".into()),
                categories: Some(vec![category]),
                tags: Some(vec!["Clássico".into(), "clássico".into(), "Ensaio".into()]),
                collections: Some(vec![collection]),
                custom: None,
            },
        )
        .unwrap();
    let check = |library: &musaeum_library::Library| {
        let after = library.work_metadata(work).unwrap();
        assert_eq!(after.title, "Meu Título");
        assert_eq!(after.creators[0].role, "translator");
        assert_eq!(after.creators[1], Credit::author("Autora Um"));
        assert_eq!(after.series.as_deref(), Some("Uma Série"));
        assert_eq!(after.series_position, Some(2.5));
        assert_eq!(after.year, Some(1999));
        assert_eq!(after.language.as_deref(), Some("pt-BR"));
        assert_eq!(after.identifiers[0].value, "0306406152");
        assert_eq!(after.identifiers[1].scheme, "doi");
        assert_eq!(after.rating, Some(4));
        assert_eq!(after.status, "reading");
        assert_eq!(after.categories, [category]);
        assert_eq!(after.tags.len(), 2, "one tag per name, however written");
        assert_eq!(after.collections, [collection]);
        for field in [
            "title",
            "creators",
            "series",
            "series_position",
            "year",
            "language",
            "identifiers",
            "rating",
            "status",
        ] {
            assert_eq!(
                after.provenance.get(field).map(String::as_str),
                Some("user"),
                "{field}"
            );
        }
    };
    check(library);
    // Searching finds the edited names.
    assert_eq!(
        titles(
            &fixture,
            BrowseQuery {
                text: "tradutora".into(),
                ..BrowseQuery::default()
            }
        ),
        ["Meu Título"]
    );

    // Read again from the same file, and from a new edition of it with another title, other
    // creators, another identifier: nothing the person set moves.
    library.reimport(document.id).unwrap();
    check(fixture.library());
    fs::write(
        &path,
        epub(
            "Outro Título",
            &["Autor Novo"],
            Some("urn:isbn:9780306406157"),
            2,
        ),
    )
    .unwrap();
    fixture.settle();
    check(fixture.library());

    // An edit that cannot be made changes nothing at all.
    assert!(refused(fixture.library().edit_work(
        work,
        WorkEdit {
            title: Some("Não fica".into()),
            rating: Some(Some(9)),
            ..WorkEdit::default()
        }
    )));
    assert_eq!(
        fixture.library().work_metadata(work).unwrap().title,
        "Meu Título"
    );
    // Cleared, a field stays the person's: empty.
    fixture
        .library()
        .edit_work(
            work,
            WorkEdit {
                series: Some(None),
                rating: Some(None),
                ..WorkEdit::default()
            },
        )
        .unwrap();
    let cleared = fixture.library().work_metadata(work).unwrap();
    assert_eq!((cleared.series, cleared.rating), (None, None));
}

#[test]
fn what_nobody_edited_follows_the_file() {
    let fixture = Fixture::new();
    let path = fixture.write(
        "livro.epub",
        &epub("Primeiro", &["Autora"], Some("urn:isbn:9788535902778"), 1),
    );
    fixture.settle();
    let work = fixture.document_at(&path).work_id;
    fixture
        .library()
        .edit_work(
            work,
            WorkEdit {
                rating: Some(Some(3)),
                ..WorkEdit::default()
            },
        )
        .unwrap();
    fs::write(
        &path,
        epub("Segundo", &["Autora"], Some("urn:isbn:9780306406157"), 2),
    )
    .unwrap();
    fixture.settle();
    let after = fixture.library().work_metadata(work).unwrap();
    assert_eq!(after.title, "Segundo");
    assert_eq!(after.rating, Some(3));
    // Nobody edited the identifiers: the new edition's ISBN takes the old one's place.
    assert_eq!(
        after.identifiers,
        [Identifier {
            scheme: "isbn".into(),
            value: "9780306406157".into()
        }]
    );
    // A third edition that gives no identifier leaves the one before.
    fs::write(&path, epub("Terceiro", &["Autora"], None, 3)).unwrap();
    fixture.settle();
    let third = fixture.library().work_metadata(work).unwrap();
    assert_eq!(third.title, "Terceiro");
    assert_eq!(third.identifiers[0].value, "9780306406157");
}

#[test]
fn a_work_that_turns_out_to_be_another_takes_its_place_with_it() {
    let fixture = Fixture::new();
    let contents = pdf(1);
    let path = fixture.write("book.pdf", &contents);
    fixture.settle();
    let identity = fixture.mark_as_cared_for(&path);

    // The same book reappears as another file, and is put in a category before its hash is known.
    fs::remove_file(&path).unwrap();
    let again = fixture.write("again.pdf", &contents);
    fixture.reconcile();
    let provisional = fixture.identity(&again);
    assert_ne!(provisional.work, identity.work);
    let library = fixture.library();
    let category = library
        .create_category(None, "Manuais", Place::Last)
        .unwrap();
    let collection = library
        .create_collection("Oficina", "", Place::Last)
        .unwrap();
    let tag = library.create_tag("técnico", None).unwrap();
    library
        .set_category(&[provisional.work], category, true)
        .unwrap();
    library.set_tag(&[provisional.work], tag, true).unwrap();
    library
        .add_to_collection(collection, &[provisional.work], Place::Last)
        .unwrap();

    fixture.process();
    let document = fixture.document_at(&again);
    assert_eq!(document.work_id, identity.work);
    let merged = fixture.library().work_metadata(identity.work).unwrap();
    assert_eq!(merged.categories, [category]);
    assert_eq!(merged.tags, [tag]);
    assert_eq!(merged.collections, [collection]);
    assert_eq!(
        fixture.library().collection_works(collection).unwrap(),
        [identity.work]
    );
}

#[test]
fn a_large_tree_lists_and_filters_quickly() {
    let fixture = shelf();
    let library = fixture.library();
    let works: Vec<WorkId> = [
        "Vida Maravilhosa",
        "Cosmos",
        "Dom Casmurro",
        "História da Arte",
    ]
    .into_iter()
    .map(|title| work_of(&fixture, title))
    .collect();
    // Twenty top categories, each with five branches five levels deep: 520 in all, every work in
    // every one.
    let mut all = Vec::new();
    for top in 0..20 {
        let root = library
            .create_category(None, &format!("Área {top}"), Place::Last)
            .unwrap();
        all.push(root);
        for branch in 0..5 {
            let mut parent = root;
            for level in 0..5 {
                let id = library
                    .create_category(Some(parent), &format!("Ramo {branch}.{level}"), Place::Last)
                    .unwrap();
                all.push(id);
                parent = id;
            }
        }
    }
    assert_eq!(all.len(), 520);
    for &category in &all {
        library.set_category(&works, category, true).unwrap();
    }
    let started = std::time::Instant::now();
    let tree = library.categories().unwrap();
    let listing = started.elapsed();
    assert_eq!(tree.len(), 520);
    assert!(
        tree.iter()
            .all(|category| category.total == 4 && category.works == 4)
    );
    let started = std::time::Instant::now();
    let found = titles(
        &fixture,
        BrowseQuery {
            category: Some(all[0]),
            ..BrowseQuery::default()
        },
    );
    let filtering = started.elapsed();
    assert_eq!(found.len(), 4);
    eprintln!(
        "520 categories, 2080 links: tree in {listing:?}, one category's books in {filtering:?}"
    );
    // Debug builds, on a shared machine: generous bounds, an order of magnitude above what it took.
    assert!(listing.as_millis() < 500, "{listing:?}");
    assert!(filtering.as_millis() < 500, "{filtering:?}");
}

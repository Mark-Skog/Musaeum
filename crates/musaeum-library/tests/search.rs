//! Finding works as someone types: accents, the middle of words, and whatever gets typed.

mod common;

use common::{Fixture, epub};
use musaeum_library::{MatchKind, WorkEdit};

/// A library with these works, by title and authors.
fn library_of(books: &[(&str, &[&str])]) -> Fixture {
    let fixture = Fixture::new();
    for (index, (title, creators)) in books.iter().enumerate() {
        fixture.write(
            &format!("book{index}.epub"),
            &epub(title, creators, None, index as u64 + 1),
        );
    }
    fixture.settle();
    fixture
}

fn titles(fixture: &Fixture, query: &str) -> Vec<String> {
    fixture
        .library()
        .search(query, 50)
        .unwrap()
        .into_iter()
        .map(|hit| hit.title)
        .collect()
}

#[test]
fn accents_do_not_matter_in_either_direction() {
    let fixture = library_of(&[
        ("Crime e Castigo", &["Fiódor Dostoiévski"]),
        ("São Bernardo", &["Graciliano Ramos"]),
        ("Critique de la raison pure", &["Immanuel Kant"]),
    ]);
    assert_eq!(titles(&fixture, "dostoievski"), ["Crime e Castigo"]);
    assert_eq!(titles(&fixture, "dostoiévski"), ["Crime e Castigo"]);
    assert_eq!(titles(&fixture, "DOSTOIEVSKI"), ["Crime e Castigo"]);
    assert_eq!(titles(&fixture, "sao bernardo"), ["São Bernardo"]);
    assert_eq!(titles(&fixture, "são"), ["São Bernardo"]);
    assert_eq!(titles(&fixture, "crítique"), ["Critique de la raison pure"]);
}

#[test]
fn text_in_the_middle_of_a_word_is_found() {
    let fixture = library_of(&[
        ("Dom Casmurro", &["Machado de Assis"]),
        ("Memórias Póstumas", &["Machado de Assis"]),
    ]);
    assert_eq!(titles(&fixture, "smur"), ["Dom Casmurro"]);
    assert_eq!(titles(&fixture, "ostum"), ["Memórias Póstumas"]);
    assert_eq!(titles(&fixture, "óstum"), ["Memórias Póstumas"]);
    assert_eq!(titles(&fixture, "chado").len(), 2);
    assert_eq!(titles(&fixture, "ssis").len(), 2);
}

#[test]
fn every_word_typed_has_to_be_there() {
    let fixture = library_of(&[
        ("Dom Casmurro", &["Machado de Assis"]),
        ("Dom Quixote", &["Miguel de Cervantes"]),
    ]);
    assert_eq!(titles(&fixture, "dom").len(), 2);
    assert_eq!(titles(&fixture, "dom machado"), ["Dom Casmurro"]);
    assert_eq!(titles(&fixture, "miguel dom"), ["Dom Quixote"]);
    assert!(titles(&fixture, "dom kant").is_empty());
}

#[test]
fn each_keystroke_finds_what_the_finished_word_will() {
    let fixture = library_of(&[("Dom Quixote", &["Miguel de Cervantes"])]);
    for typed in [
        "d",
        "do",
        "dom",
        "dom ",
        "dom q",
        "dom qu",
        "dom qui",
        "dom quix",
        "dom quixote",
    ] {
        assert_eq!(
            titles(&fixture, typed),
            ["Dom Quixote"],
            "typed so far: {typed:?}"
        );
    }
    for typed in ["c", "ce", "cer", "cerv", "cervantes"] {
        assert_eq!(
            titles(&fixture, typed),
            ["Dom Quixote"],
            "typed so far: {typed:?}"
        );
    }
}

#[test]
fn a_title_that_starts_with_what_was_typed_comes_first() {
    let fixture = library_of(&[
        ("Histórias da Meia-Noite", &["Machado de Assis"]),
        ("A Meia-Noite", &["Outra Pessoa"]),
        ("Meia-Noite em Paris", &["Alguém"]),
        ("Semeia a Noite", &["Alguém Mais"]),
    ]);
    let hits = fixture.library().search("meia", 50).unwrap();
    assert_eq!(hits[0].title, "Meia-Noite em Paris");
    assert_eq!(hits[0].matched, MatchKind::TitleStart);
    assert_eq!(hits[1].matched, MatchKind::WordStart);
    assert_eq!(hits[2].matched, MatchKind::WordStart);
    assert_eq!(hits[3].title, "Semeia a Noite");
    assert_eq!(hits[3].matched, MatchKind::Substring);
}

#[test]
fn a_work_comes_up_once_however_many_ways_it_matches() {
    let fixture = library_of(&[("Casa Casa Casa", &["Casa Grande"])]);
    assert_eq!(titles(&fixture, "casa").len(), 1);
}

#[test]
fn the_limit_is_respected_across_kinds_of_match() {
    let fixture = library_of(&[
        ("Alfa Um", &[]),
        ("Alfa Dois", &[]),
        ("Um Alfa", &[]),
        ("Dois Alfa", &[]),
        ("Malfada", &[]),
    ]);
    assert_eq!(fixture.library().search("alfa", 50).unwrap().len(), 5);
    let two = fixture.library().search("alfa", 2).unwrap();
    assert_eq!(two.len(), 2);
    assert!(two.iter().all(|hit| hit.matched == MatchKind::TitleStart));
    assert_eq!(fixture.library().search("alfa", 0).unwrap().len(), 0);
}

#[test]
fn whatever_gets_typed_is_never_an_error() {
    let fixture = library_of(&[("Um Livro", &["Alguém"])]);
    for hostile in [
        "\"",
        "\"\"",
        "\"\"\"",
        "*",
        "**",
        "(",
        ")",
        "()",
        "^",
        "-",
        "--",
        "+",
        ":",
        "::",
        "AND",
        "OR",
        "NOT",
        "NEAR",
        "NEAR(",
        "AND OR NOT",
        "a AND",
        "title:",
        "title : x",
        "{title}",
        "x\"y",
        "x*y",
        "'",
        "''; DROP TABLE work; --",
        "\\",
        "%",
        "_",
        "\0",
        "a\0b",
        "\u{FFFD}",
        "🧪",
        "   ",
        "\t\n",
        "ａｂｃ",
        "ß",
        "İ",
        "ﬁ",
        "(((((((",
        "\"a\" \"b\"",
        "a\"* OR \"b",
    ] {
        let result = fixture.library().search(hostile, 10);
        assert!(result.is_ok(), "{hostile:?}: {result:?}");
    }
    let long = "a".repeat(100_000);
    assert!(fixture.library().search(&long, 10).is_ok());
    let many = "word ".repeat(10_000);
    assert!(fixture.library().search(&many, 10).is_ok());
    assert_eq!(
        fixture.library().status().unwrap().works,
        1,
        "nothing happened to the table"
    );
}

#[test]
fn punctuation_around_a_word_does_not_hide_it() {
    let fixture = library_of(&[("Um Livro", &["Alguém"])]);
    assert_eq!(titles(&fixture, "livro!"), ["Um Livro"]);
    assert_eq!(titles(&fixture, "(livro)"), ["Um Livro"]);
    assert_eq!(titles(&fixture, "um -- livro"), ["Um Livro"]);
}

#[test]
fn nothing_typed_finds_nothing() {
    let fixture = library_of(&[("Um Livro", &["Alguém"])]);
    assert!(titles(&fixture, "").is_empty());
    assert!(titles(&fixture, "   ").is_empty());
}

#[test]
fn other_scripts_are_found() {
    let fixture = library_of(&[
        ("Преступление и наказание", &["Фёдор Достоевский"]),
        ("吾輩は猫である", &["夏目漱石"]),
    ]);
    assert_eq!(titles(&fixture, "наказание"), ["Преступление и наказание"]);
    assert_eq!(titles(&fixture, "Преступ"), ["Преступление и наказание"]);
    assert_eq!(titles(&fixture, "досто"), ["Преступление и наказание"]);
    assert_eq!(
        titles(&fixture, "猫である"),
        ["吾輩は猫である"],
        "inside a run of Japanese"
    );
    assert_eq!(
        titles(&fixture, "吾輩"),
        ["吾輩は猫である"],
        "from its start, in fewer characters"
    );
}

#[test]
fn an_edit_is_searchable_right_away_and_what_it_replaced_is_not() {
    let fixture = library_of(&[("Título Antigo", &["Autora Antiga"])]);
    let work = fixture.library().search("antigo", 1).unwrap()[0].work_id;
    fixture
        .library()
        .edit_work(
            work,
            WorkEdit {
                title: Some("Título Novo".to_owned()),
                creators: Some(vec!["Autora Nova".to_owned()]),
                series: Some("Uma Série".to_owned()),
                ..WorkEdit::default()
            },
        )
        .unwrap();

    assert_eq!(titles(&fixture, "novo"), ["Título Novo"]);
    assert_eq!(titles(&fixture, "autora nova"), ["Título Novo"]);
    assert_eq!(titles(&fixture, "série"), ["Título Novo"]);
    assert!(titles(&fixture, "antigo").is_empty());
    assert!(titles(&fixture, "antiga").is_empty());
    let hit = &fixture.library().search("novo", 1).unwrap()[0];
    assert_eq!(hit.creators.as_deref(), Some("Autora Nova"));
    assert_eq!(hit.series.as_deref(), Some("Uma Série"));
}

#[test]
fn creators_come_back_in_the_books_order() {
    let fixture = library_of(&[("Um Livro", &["Zeca Primeiro", "Ana Segunda"])]);
    let hit = &fixture.library().search("livro", 1).unwrap()[0];
    assert_eq!(hit.creators.as_deref(), Some("Zeca Primeiro; Ana Segunda"));
}

#[test]
fn a_file_name_is_searchable_until_the_book_says_something_better() {
    let fixture = Fixture::new();
    fixture.write(
        "nome-do-arquivo.epub",
        &epub("Título de Dentro", &["Alguém"], None, 1),
    );
    fixture.reconcile();
    assert_eq!(titles(&fixture, "arquivo"), ["nome-do-arquivo"]);

    fixture.process();
    assert!(titles(&fixture, "arquivo").is_empty());
    assert_eq!(titles(&fixture, "dentro"), ["Título de Dentro"]);
}

#[test]
fn a_work_whose_files_are_missing_is_still_found_and_says_so() {
    let fixture = library_of(&[("Um Livro", &["Alguém"])]);
    assert!(fixture.library().search("livro", 1).unwrap()[0].available);

    std::fs::remove_file(fixture.path("book0.epub")).unwrap();
    fixture.settle();

    let hit = &fixture.library().search("livro", 1).unwrap()[0];
    assert_eq!(hit.title, "Um Livro");
    assert!(!hit.available);
}

#[test]
fn a_duplicate_merged_away_is_not_found_twice() {
    let fixture = Fixture::new();
    let contents = epub("Um Livro", &["Alguém"], None, 1);
    fixture.write("one.epub", &contents);
    fixture.write("two.epub", &contents);
    fixture.settle();
    assert_eq!(titles(&fixture, "livro").len(), 1);
    assert!(titles(&fixture, "one").is_empty() || titles(&fixture, "two").is_empty());
}

#[test]
fn the_indexes_can_be_rebuilt_from_the_works() {
    let fixture = library_of(&[("Dom Casmurro", &["Machado de Assis"])]);
    fixture.library().rebuild_search().unwrap();
    assert_eq!(titles(&fixture, "casmurro"), ["Dom Casmurro"]);
    assert_eq!(titles(&fixture, "smur"), ["Dom Casmurro"]);
    assert_eq!(titles(&fixture, "machado"), ["Dom Casmurro"]);
}

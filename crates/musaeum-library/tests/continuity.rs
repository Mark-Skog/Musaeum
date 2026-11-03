//! Path continuity, refined in Phase 2: a file that takes another's place at its path stays a new
//! revision of the same document only if it is the same kind of file and names no other `/ID[0]` or
//! `dc:identifier`. Otherwise it is a document of its own, the file it replaced is missing, and a
//! person decides through a suggestion.

mod common;

use std::path::Path;

use common::{Fixture, Identity, epub, pdf, pdf_with_id, safe_save};
use musaeum_library::{InstanceState, RevisionOrigin, SuggestionKind};

const FIRST_ID: &str = "aabbccddeeff00112233445566778899";
const SECOND_ID: &str = "99887766554433221100ffeeddccbbaa";

fn cared_for(fixture: &Fixture, relative: &str, contents: &[u8]) -> (std::path::PathBuf, Identity) {
    let path = fixture.write(relative, contents);
    fixture.settle();
    let identity = fixture.mark_as_cared_for(&path);
    (path, identity)
}

fn provisional_replacements(fixture: &Fixture) -> i64 {
    fixture
        .raw()
        .query_row("SELECT count(*) FROM replaced_file", [], |row| row.get(0))
        .unwrap()
}

/// The checks that a replacement turned out to be something else, and nothing was lost.
fn assert_separated(fixture: &Fixture, path: &Path, old: &Identity, evidence: &[&str]) {
    let new = fixture.identity(path);
    assert_ne!(new.document, old.document, "a document of its own");
    assert_ne!(new.instance, old.instance, "and an instance of its own");
    assert_eq!(fixture.state_of(path), InstanceState::Present);

    let old_instance = fixture.library().instance(old.instance).unwrap();
    assert_eq!(
        old_instance.state,
        InstanceState::Absent,
        "the old file is missing"
    );
    assert_eq!(old_instance.revision_id, old.revision, "and is what it was");
    assert_eq!(old_instance.path, path, "last seen where it was");

    let old_document = fixture.library().document(old.document).unwrap();
    assert_eq!(
        old_document.revisions.len(),
        1,
        "no revision of something else"
    );
    let new_document = fixture.library().document(new.document).unwrap();
    assert_eq!(new_document.revisions.len(), 1);
    assert_eq!(new_document.revisions[0].origin, RevisionOrigin::Import);
    assert_eq!(new_document.revisions[0].previous_revision_id, None);

    let suggestions = fixture.library().suggestions(true).unwrap();
    assert_eq!(suggestions.len(), 1, "{suggestions:?}");
    assert_eq!(suggestions[0].kind, SuggestionKind::SamePath);
    assert_eq!(suggestions[0].document_id, new.document);
    assert_eq!(suggestions[0].candidate_document_id, old.document);
    for word in evidence {
        assert!(
            suggestions[0].evidence.contains(word),
            "{word} is not in: {}",
            suggestions[0].evidence
        );
    }
    assert_eq!(provisional_replacements(fixture), 0);
    fixture.assert_still_cared_for(old);
}

#[test]
fn a_replacement_of_another_kind_is_a_document_of_its_own() {
    let fixture = Fixture::new();
    let (path, identity) = cared_for(&fixture, "book.pdf", &pdf(1));

    // A download that failed and saved the server's error page under the book's name.
    safe_save(
        &path,
        b"<!DOCTYPE html><html><body>502 Bad Gateway</body></html>",
    );
    let (scan, content) = fixture.settle();

    assert_eq!(scan.replaced_at_path, 1, "it looked like a safe save");
    assert_eq!(content.replacements_separated, 1);
    assert_eq!(fixture.document_count(), 2);
    assert_separated(&fixture, &path, &identity, &["pdf", "unknown"]);
}

#[test]
fn a_replacement_with_another_pdf_id_is_a_document_of_its_own() {
    let fixture = Fixture::new();
    let (path, identity) = cared_for(&fixture, "book.pdf", &pdf_with_id(FIRST_ID, 1));

    safe_save(&path, &pdf_with_id(SECOND_ID, 2));
    let (_, content) = fixture.settle();

    assert_eq!(content.replacements_separated, 1);
    assert_separated(&fixture, &path, &identity, &[FIRST_ID, SECOND_ID]);
    assert_eq!(
        fixture.document_at(&path).pdf_id0.as_deref(),
        Some(SECOND_ID),
        "what it says about itself went to its own document"
    );
    assert_eq!(
        fixture
            .library()
            .document(identity.document)
            .unwrap()
            .pdf_id0
            .as_deref(),
        Some(FIRST_ID)
    );
}

#[test]
fn a_replacement_with_another_dc_identifier_is_a_document_of_its_own() {
    let fixture = Fixture::new();
    let (path, identity) = cared_for(
        &fixture,
        "book.epub",
        &epub("Um Livro", &["Autora"], Some("urn:isbn:9780000000001"), 1),
    );

    safe_save(
        &path,
        &epub("Outro Livro", &["Outra"], Some("urn:isbn:9780000000002"), 2),
    );
    fixture.settle();

    assert_separated(
        &fixture,
        &path,
        &identity,
        &[
            "dc:identifier",
            "urn:isbn:9780000000001",
            "urn:isbn:9780000000002",
        ],
    );
}

#[test]
fn a_replacement_with_the_same_pdf_id_is_a_new_revision() {
    let fixture = Fixture::new();
    let (path, identity) = cared_for(&fixture, "book.pdf", &pdf_with_id(FIRST_ID, 1));

    safe_save(&path, &pdf_with_id(FIRST_ID, 2));
    let (_, content) = fixture.settle();

    assert_eq!(content.replacements_separated, 0);
    assert_eq!(content.new_revisions, 1);
    let document = fixture.document_at(&path);
    assert_eq!(document.id, identity.document);
    assert_eq!(document.revisions.len(), 2);
    assert_eq!(document.revisions[1].origin, RevisionOrigin::ReplacedAtPath);
    assert_eq!(fixture.identity(&path).instance, identity.instance);
    assert!(fixture.library().suggestions(true).unwrap().is_empty());
    assert_eq!(
        provisional_replacements(&fixture),
        0,
        "no longer provisional"
    );
    fixture.assert_still_cared_for(&identity);
}

#[test]
fn an_id_on_only_one_side_is_no_conflict() {
    let fixture = Fixture::new();
    let (path, identity) = cared_for(&fixture, "book.pdf", &pdf(1));

    safe_save(&path, &pdf_with_id(FIRST_ID, 2));
    fixture.settle();
    assert_eq!(fixture.identity(&path).document, identity.document);

    safe_save(&path, &pdf(3));
    fixture.settle();
    let document = fixture.document_at(&path);
    assert_eq!(document.id, identity.document);
    assert_eq!(document.revisions.len(), 3);
    assert_eq!(document.pdf_id0.as_deref(), Some(FIRST_ID));
    assert!(fixture.library().suggestions(true).unwrap().is_empty());
    fixture.assert_still_cared_for(&identity);
}

#[test]
fn a_replacement_stays_provisional_until_its_bytes_are_read() {
    let fixture = Fixture::new();
    let (path, identity) = cared_for(&fixture, "book.pdf", &pdf_with_id(FIRST_ID, 1));

    safe_save(&path, &pdf_with_id(SECOND_ID, 2));
    fixture.reconcile();
    assert_eq!(
        fixture.identity(&path).instance,
        identity.instance,
        "until then, it is taken for what a safe save would be"
    );
    assert_eq!(provisional_replacements(&fixture), 1);

    fixture.process();
    assert_ne!(fixture.identity(&path).document, identity.document);
    assert_eq!(provisional_replacements(&fixture), 0);
}

#[test]
fn a_replacement_that_is_another_documents_bytes_leaves_the_old_file_missing() {
    let fixture = Fixture::new();
    let (path, identity) = cared_for(&fixture, "book.pdf", &pdf(1));
    let other_bytes = pdf(2);
    let other = fixture.write("shelf/other.pdf", &other_bytes);
    fixture.settle();
    let other_identity = fixture.identity(&other);

    safe_save(&path, &other_bytes);
    let (_, content) = fixture.settle();

    assert_eq!(content.duplicates, 1, "the bytes are the other book's");
    assert_eq!(content.replacements_separated, 1);
    assert_eq!(fixture.identity(&path).revision, other_identity.revision);
    let old_instance = fixture.library().instance(identity.instance).unwrap();
    assert_eq!(old_instance.state, InstanceState::Absent);
    assert_eq!(old_instance.revision_id, identity.revision);
    assert_eq!(fixture.document_count(), 2);
    assert_eq!(provisional_replacements(&fixture), 0);
    fixture.assert_still_cared_for(&identity);
}

#[test]
fn a_note_made_while_the_replacement_was_provisional_follows_its_bytes() {
    let fixture = Fixture::new();
    let (path, identity) = cared_for(&fixture, "book.pdf", &pdf_with_id(FIRST_ID, 1));

    safe_save(&path, &pdf_with_id(SECOND_ID, 2));
    fixture.reconcile();
    // Read and annotated before the hash came in: the note is on the new bytes.
    fixture
        .library()
        .add_annotation(identity.document, "about the new file")
        .unwrap();
    fixture.process();

    let new = fixture.document_at(&path);
    assert_eq!(new.annotations.len(), 1);
    assert_eq!(new.annotations[0].note, "about the new file");
    fixture.assert_still_cared_for(&identity);
}

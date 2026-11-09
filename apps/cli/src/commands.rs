//! Does what the command line asked for.

use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use musaeum_library::{
    ContentReport, DocumentId, DocumentInfo, Error, Indexer, IndexerConfig, IndexerEvent,
    InstanceState, Library, LibraryOptions, LibraryPaths, ReconcileReport, ReconcileScope, Result,
    WorkEdit,
};
use serde::Serialize;

use crate::args::{Command, Edit, Invocation, Target};

/// Prints either way. People get the short form, and programs get all of it.
struct Output {
    json: bool,
}

impl Output {
    fn show<T: Serialize>(&self, value: &T, human: impl FnOnce(&T)) {
        if self.json {
            match serde_json::to_string_pretty(value) {
                Ok(text) => println!("{text}"),
                Err(error) => eprintln!("musaeum: {error}"),
            }
        } else {
            human(value);
        }
    }
}

pub fn run(invocation: Invocation) -> Result<()> {
    let paths = match &invocation.library {
        Some(directory) => LibraryPaths::in_directory(directory),
        None => LibraryPaths::default_location()?,
    };
    let output = Output {
        json: invocation.json,
    };
    match invocation.command {
        Command::RootAdd { path, exclude } => {
            let library = open(paths)?;
            let root = library.add_root(&path, &exclude)?;
            output.show(&root, |root| {
                println!("root {}: {}", root.id, root.path.display());
            });
        }
        Command::RootList => {
            let library = look(paths)?;
            output.show(&library.roots()?, |roots| {
                for root in roots {
                    println!(
                        "{:>4}  {:<12}  {}",
                        root.id,
                        root.status,
                        root.path.display()
                    );
                }
            });
        }
        Command::RootRemove { id } => open(paths)?.remove_root(id)?,
        Command::RootRelocate { id, path } => {
            let root = open(paths)?.relocate_root(id, &path)?;
            output.show(&root, |root| {
                println!("root {}: {}", root.id, root.path.display());
            });
        }
        Command::Reconcile { root, hash } => reconcile(paths, &output, root, hash)?,
        Command::Hash => {
            let report = open(paths)?.process_content()?;
            output.show(&report, show_content);
        }
        Command::Watch { interval } => watch(paths, interval)?,
        Command::Status => {
            let library = look(paths)?;
            output.show(&library.status()?, |status| {
                println!("works:      {}", status.works);
                println!("documents:  {}", status.documents);
                println!(
                    "revisions:  {} ({} waiting for their hash)",
                    status.revisions, status.unconfirmed_revisions
                );
                println!(
                    "files:      {} present, {} absent, {} only in the cloud, {} unreadable",
                    status.present, status.absent, status.cloud_only, status.unreadable
                );
                println!("folders:    {}", status.folders);
                println!("notes:      {}", status.annotations);
                println!("suggestions: {} open", status.open_suggestions);
                for root in &status.roots {
                    println!(
                        "root {}:     {} ({})",
                        root.id,
                        root.path.display(),
                        root.status
                    );
                }
            });
        }
        Command::List { state, limit } => {
            let library = look(paths)?;
            let state = state.as_deref().map(parse_state).transpose()?;
            output.show(&library.instances(state, limit)?, |instances| {
                for instance in instances {
                    println!(
                        "{:>8}  {:<10}  doc {:<8} {}",
                        instance.id,
                        instance.state.as_str(),
                        instance.document_id,
                        instance.path.display()
                    );
                }
            });
        }
        Command::Inspect { target } => inspect(paths, &output, target)?,
        Command::Revisions { target } => {
            let library = look(paths)?;
            let document = document_of(&library, target)?;
            output.show(&document.revisions, |_| show_revisions(&document));
        }
        Command::Search {
            query,
            limit,
            keystrokes,
        } => search(paths, &output, &query, limit, keystrokes)?,
        Command::Suggestions { all } => {
            let library = look(paths)?;
            output.show(&library.suggestions(!all)?, |suggestions| {
                for suggestion in suggestions {
                    println!(
                        "{:>4}  doc {} may be doc {}: {} [{}]",
                        suggestion.id,
                        suggestion.document_id,
                        suggestion.candidate_document_id,
                        suggestion.evidence,
                        suggestion.state,
                    );
                }
            });
        }
        Command::SuggestionAccept { id } => {
            let document = open(paths)?.accept_suggestion(id)?;
            println!("merged into document {document}");
        }
        Command::SuggestionDismiss { id } => open(paths)?.dismiss_suggestion(id)?,
        Command::Edit { path, edit } => {
            let library = open(paths)?;
            let work = instance_at(&library, &path)?.work_id;
            library.edit_work(work, work_edit(edit))?;
        }
        Command::NoteAdd { path, text } => {
            let library = open(paths)?;
            let document = instance_at(&library, &path)?.document_id;
            let id = library.add_annotation(document, &text)?;
            println!("note {id} on document {document}");
        }
        Command::Normalize { book, out } => normalize(&book, &out)?,
        Command::Reimport { path } => {
            let library = open(paths)?;
            let document = instance_at(&library, &path)?.document_id;
            library.reimport(document)?;
        }
        Command::Move { source, target } => {
            let library = open(paths)?;
            let operation = if let Some(folder) = library.folder_at(&source)? {
                library.move_folder(folder, &target)?
            } else {
                library.move_file(instance_at(&library, &source)?.id, &target)?
            };
            println!("operation {operation}");
        }
        Command::Recycle { path } => {
            let library = open(paths)?;
            let operation = library.recycle_file(instance_at(&library, &path)?.id)?;
            println!("operation {operation}: sent to the Recycle Bin");
        }
        Command::Undo { operation } => {
            let library = open(paths)?;
            let operation = match operation {
                Some(operation) => operation,
                None => library
                    .operations(100)?
                    .into_iter()
                    .find(|operation| operation.state == "done" && operation.undo_of.is_none())
                    .map(|operation| operation.id)
                    .ok_or_else(|| Error::NotFound("there is nothing to undo".to_owned()))?,
            };
            let undo = library.undo(operation)?;
            println!("operation {undo} took back operation {operation}");
        }
        Command::Journal { limit } => {
            let library = look(paths)?;
            output.show(&library.operations(limit)?, |operations| {
                for operation in operations {
                    println!(
                        "{:>6}  {:<12} {:<8} {} -> {}",
                        operation.id,
                        operation.kind,
                        operation.state,
                        operation.source_path.display(),
                        operation.target_path.as_deref().unwrap_or("-"),
                    );
                    if let Some(error) = &operation.error {
                        println!("        {error}");
                    }
                }
            });
        }
        Command::BackupNow => {
            let backup = open(paths)?.backup_now()?;
            output.show(&backup, |backup| println!("{}", backup.path.display()));
        }
        Command::BackupList => {
            let library = look(paths)?;
            output.show(&library.backups()?, |backups| {
                for backup in backups {
                    println!("{:>12}  {}", backup.size, backup.path.display());
                }
            });
        }
        Command::BackupRestore { file } => {
            let set_aside = Library::restore_backup(&paths, &file)?;
            println!("restored. What it replaced is in {}", set_aside.display());
        }
        Command::RebuildSearch => open(paths)?.rebuild_search()?,
    }
    Ok(())
}

/// Normalizes one book in the isolated worker, as the app does, and writes the result.
fn normalize(book: &Path, out: &Path) -> Result<()> {
    let extension = book
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let format = musaeum_ingest::protocol::Format::from_extension(&extension)
        .filter(|format| format.flow().is_some())
        .ok_or_else(|| Error::Refused(format!("{} is not a flowing book", book.display())))?;
    let file = std::fs::File::open(book)
        .map_err(|error| Error::Refused(format!("{}: {error}", book.display())))?;
    let mut client =
        musaeum_library::IngestClient::new(musaeum_library::IngestConfig::for_normalizing());
    let started = Instant::now();
    let normalized = match client.normalize(&file, format) {
        musaeum_library::NormalizeOutcome::Normalized(normalized) => normalized,
        musaeum_library::NormalizeOutcome::Nothing(reason)
        | musaeum_library::NormalizeOutcome::Unavailable(reason) => {
            return Err(Error::Refused(reason));
        }
    };
    let elapsed = started.elapsed();
    let write = |path: &Path, data: &[u8]| {
        std::fs::write(path, data)
            .map_err(|error| Error::Refused(format!("{}: {error}", path.display())))
    };
    if out
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("epub"))
    {
        let book = musaeum_flow::package::NormalizedBook {
            manifest: normalized.manifest.clone(),
            files: normalized.files.clone(),
        };
        write(out, &book.to_epub())?;
    } else {
        for (path, data) in &normalized.files {
            let target = out.join(path);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| Error::Refused(format!("{}: {error}", parent.display())))?;
            }
            write(&target, data)?;
        }
    }
    let stats = &normalized.manifest.stats;
    println!(
        "{} documents in {} parts ({} cut), largest part {} bytes, {} files; {:.0} ms in the worker",
        stats.documents,
        stats.parts,
        stats.split_documents,
        stats.largest_part,
        normalized.files.len(),
        elapsed.as_secs_f64() * 1000.0
    );
    Ok(())
}

fn open(paths: LibraryPaths) -> Result<Library> {
    let library = Library::open(paths, LibraryOptions::default())?;
    let recovery = library.recovery_report();
    if recovery.completed + recovery.aborted + recovery.failed > 0 {
        eprintln!(
            "musaeum: finished what a crash interrupted: {} completed, {} never happened, {} unclear",
            recovery.completed, recovery.aborted, recovery.failed
        );
    }
    Ok(library)
}

fn look(paths: LibraryPaths) -> Result<Library> {
    Library::open_read_only(paths)
}

fn parse_state(text: &str) -> Result<InstanceState> {
    match text {
        "present" => Ok(InstanceState::Present),
        "absent" => Ok(InstanceState::Absent),
        "cloud_only" => Ok(InstanceState::CloudOnly),
        "unreadable" => Ok(InstanceState::Unreadable),
        other => Err(Error::Refused(format!("{other} is not a state"))),
    }
}

fn instance_at(library: &Library, path: &Path) -> Result<musaeum_library::InstanceInfo> {
    library
        .instance_at(path)?
        .ok_or_else(|| Error::NotFound(format!("the library has nothing at {}", path.display())))
}

fn document_of(library: &Library, target: Target) -> Result<DocumentInfo> {
    let id: DocumentId = match target {
        Target::Path(path) => instance_at(library, &path)?.document_id,
        Target::Document(id) => id,
        Target::Work(work) => *library
            .work(work)?
            .document_ids
            .first()
            .ok_or_else(|| Error::NotFound(format!("work {work} has no documents")))?,
    };
    library.document(id)
}

fn work_edit(edit: Edit) -> WorkEdit {
    WorkEdit {
        title: edit.title,
        rating: edit.rating,
        status: edit.status,
        series: edit.series,
        creators: edit.creators,
        ..WorkEdit::default()
    }
}

fn reconcile(paths: LibraryPaths, output: &Output, root: Option<i64>, hash: bool) -> Result<()> {
    let library = open(paths)?;
    let reports = match root {
        Some(root) => vec![library.reconcile(root, &ReconcileScope::Everything)?],
        None => library.reconcile_all()?,
    };
    let content = if hash {
        Some(library.process_content()?)
    } else {
        None
    };
    library.backup_if_due()?;

    #[derive(Serialize)]
    struct Outcome {
        scans: Vec<ReconcileReport>,
        content: Option<ContentReport>,
    }
    output.show(
        &Outcome {
            scans: reports,
            content,
        },
        |outcome| {
            for scan in &outcome.scans {
                show_scan(scan);
            }
            if let Some(content) = &outcome.content {
                show_content(content);
            }
        },
    );
    Ok(())
}

fn show_scan(scan: &ReconcileReport) {
    if !scan.root_available {
        println!(
            "root {}: cannot be read, so nothing about it changed",
            scan.root_id
        );
        return;
    }
    println!(
        "root {}: {} files in {} folders, in {} ms, reading {} bytes from {} files",
        scan.root_id,
        scan.files_seen,
        scan.directories_listed,
        scan.duration_ms,
        scan.bytes_read,
        scan.files_opened,
    );
    let lines = [
        ("new", scan.new_files),
        ("moved or renamed", scan.moved),
        ("came back", scan.restored),
        ("other names for a known file", scan.hard_links),
        ("changed in place", scan.changed_in_place),
        ("replaced at their path", scan.replaced_at_path),
        ("gone", scan.became_absent),
        ("now only in the cloud", scan.became_cloud_only),
        ("now local", scan.became_local),
        ("unreadable", scan.unreadable),
        ("folders moved", scan.folders_moved),
        ("folders new", scan.folders_new),
        ("folders gone", scan.folders_absent),
    ];
    for (label, count) in lines {
        if count > 0 {
            println!("  {label}: {count}");
        }
    }
}

fn show_content(content: &ContentReport) {
    println!(
        "hashed {} files, {} bytes, in {} ms",
        content.hashed, content.bytes_hashed, content.duration_ms
    );
    let lines = [
        (
            "came back as a file that had gone missing",
            content.reattached,
        ),
        ("copies of something known", content.duplicates),
        ("same bytes as before", content.same_as_previous),
        ("new bytes", content.new_revisions),
        ("suggestions", content.suggestions),
        ("metadata read", content.extracted),
        ("metadata could not be read", content.extraction_failed),
        ("workers replaced", content.worker_restarts),
        ("only in the cloud", content.cloud_only),
        ("unreadable", content.unreadable),
        ("changed while being read", content.changed_meanwhile),
    ];
    for (label, count) in lines {
        if count > 0 {
            println!("  {label}: {count}");
        }
    }
}

fn watch(paths: LibraryPaths, interval: Option<u64>) -> Result<()> {
    let library = Arc::new(open(paths)?);
    let mut config = IndexerConfig::default();
    if let Some(seconds) = interval {
        config.reconcile_interval = Duration::from_secs(seconds);
    }
    let (sender, events) = mpsc::channel();
    let _indexer = Indexer::start(Arc::clone(&library), config, Some(sender))?;
    println!("watching. Stop this with Ctrl+C: nothing is lost by stopping at any moment");
    for event in events {
        match event {
            IndexerEvent::Reconciled(scan) => show_scan(&scan),
            IndexerEvent::ContentProcessed(content) => {
                if content.hashed > 0 || content.extracted > 0 {
                    show_content(&content);
                }
            }
            IndexerEvent::WatchOverflowed(root) => {
                println!("root {root}: too many changes at once, so looking at all of it");
            }
            IndexerEvent::WatchLost(root, reason) => {
                println!("root {root}: not being watched for now: {reason}");
            }
            IndexerEvent::Failed(reason) => eprintln!("musaeum: {reason}"),
        }
    }
    Ok(())
}

fn inspect(paths: LibraryPaths, output: &Output, target: Target) -> Result<()> {
    let library = look(paths)?;
    let document = document_of(&library, target)?;
    let work = library.work(document.work_id)?;

    #[derive(Serialize)]
    struct Inspection<'a> {
        work: &'a musaeum_library::WorkInfo,
        document: &'a DocumentInfo,
    }
    output.show(
        &Inspection {
            work: &work,
            document: &document,
        },
        |_| {
            println!("work {}: {}", work.id, work.title);
            if !work.creators.is_empty() {
                println!("  by {}", work.creators.join("; "));
            }
            if let Some(language) = &work.language {
                println!("  language: {language}");
            }
            if let Some(year) = work.year {
                println!("  year: {year}");
            }
            if let Some(series) = &work.series {
                println!("  series: {series}");
            }
            if let Some(rating) = work.rating {
                println!("  rating: {rating}");
            }
            println!("  status: {}", work.status);
            println!(
                "document {}: {} ({})",
                document.id, document.format, document.layout
            );
            if let Some(id) = &document.pdf_id0 {
                println!("  /ID[0]: {id}");
            }
            if let Some(id) = &document.dc_identifier {
                println!("  dc:identifier: {id}");
            }
            if let Some(isbn) = &document.isbn {
                println!("  ISBN: {isbn}");
            }
            show_revisions(&document);
            for note in &document.annotations {
                println!(
                    "note {} on revision {}: {}",
                    note.id, note.revision_id, note.note
                );
            }
        },
    );
    Ok(())
}

fn show_revisions(document: &DocumentInfo) {
    for revision in &document.revisions {
        let hash = revision.blake3.as_deref().unwrap_or("(not confirmed yet)");
        println!(
            "revision {}: {} bytes, {:?}, BLAKE3 {hash}",
            revision.id, revision.size, revision.origin
        );
        if let Some(previous) = revision.previous_revision_id {
            println!("  after revision {previous}");
        }
        for instance in &revision.instances {
            println!(
                "  {:<10} {}",
                instance.state.as_str(),
                instance.path.display()
            );
        }
    }
}

fn search(
    paths: LibraryPaths,
    output: &Output,
    query: &str,
    limit: usize,
    keystrokes: bool,
) -> Result<()> {
    let library = look(paths)?;
    if !keystrokes {
        let started = Instant::now();
        let hits = library.search(query, limit)?;
        let elapsed = started.elapsed();
        output.show(&hits, |hits| {
            for hit in hits {
                let mark = if hit.available {
                    ""
                } else {
                    " (no file available)"
                };
                match &hit.creators {
                    Some(creators) => {
                        println!("{:>8}  {} — {creators}{mark}", hit.work_id, hit.title)
                    }
                    None => println!("{:>8}  {}{mark}", hit.work_id, hit.title),
                }
            }
            println!(
                "{} found in {:.2} ms",
                hits.len(),
                elapsed.as_secs_f64() * 1000.0
            );
        });
        return Ok(());
    }

    #[derive(Serialize)]
    struct Keystroke {
        typed: String,
        results: usize,
        milliseconds: f64,
    }
    let mut strokes = Vec::new();
    let mut typed = String::new();
    for character in query.chars() {
        typed.push(character);
        let started = Instant::now();
        let results = library.search(&typed, limit)?.len();
        strokes.push(Keystroke {
            typed: typed.clone(),
            results,
            milliseconds: started.elapsed().as_secs_f64() * 1000.0,
        });
    }
    output.show(&strokes, |strokes| {
        for stroke in strokes {
            println!(
                "{:>8.2} ms  {:>4} results  {}",
                stroke.milliseconds, stroke.results, stroke.typed
            );
        }
        let slowest = strokes
            .iter()
            .map(|stroke| stroke.milliseconds)
            .fold(0.0, f64::max);
        println!("slowest keystroke: {slowest:.2} ms, against a target of 30 ms");
    });
    Ok(())
}

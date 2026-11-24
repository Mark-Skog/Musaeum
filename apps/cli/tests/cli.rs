//! The real program, with real files, in real processes, including ones that die.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use gen_fixtures::books::{EpubMeta, epub_bytes, pdf_bytes};
use serde_json::Value;

struct Workspace {
    dir: PathBuf,
    library: PathBuf,
    root: PathBuf,
}

impl Workspace {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let number = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("musaeum-cli-{}-{number}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let root = dir.join("books");
        fs::create_dir(&root).unwrap();
        Self {
            library: dir.join("library"),
            root,
            dir,
        }
    }

    fn command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_musaeum"));
        command.arg("--library").arg(&self.library).args(arguments);
        command
    }

    fn run(&self, arguments: &[&str]) -> Output {
        let output = self.command(arguments).output().unwrap();
        assert!(
            output.status.success(),
            "musaeum {arguments:?}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn fails(&self, arguments: &[&str]) -> String {
        let output = self.command(arguments).output().unwrap();
        assert!(
            !output.status.success(),
            "musaeum {arguments:?} should have failed"
        );
        String::from_utf8_lossy(&output.stderr).into_owned()
    }

    fn json(&self, arguments: &[&str]) -> Value {
        let mut with_flag = vec!["--json"];
        with_flag.extend_from_slice(arguments);
        let output = self.run(&with_flag);
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn text(&self, arguments: &[&str]) -> String {
        String::from_utf8_lossy(&self.run(arguments).stdout).into_owned()
    }

    fn add_root(&self) {
        self.run(&["root", "add", self.root.to_str().unwrap()]);
    }

    fn write(&self, relative: &str, contents: &[u8]) -> PathBuf {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, contents).unwrap();
        path
    }

    fn status(&self) -> Value {
        self.json(&["status"])
    }

    fn db(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.library.join("library.db")).unwrap()
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn pdf(seed: u64) -> Vec<u8> {
    pdf_bytes(None, 1000, seed)
}

fn text_of(path: &Path) -> &str {
    path.to_str().unwrap()
}

#[test]
fn the_whole_cycle_from_the_command_line() {
    let workspace = Workspace::new();
    workspace.add_root();
    let path = workspace.write("shelf/dom-casmurro.pdf", &pdf(1));

    let outcome = workspace.json(&["reconcile"]);
    assert_eq!(outcome["scans"][0]["new_files"], 1);
    assert_eq!(outcome["content"]["hashed"], 1);

    let status = workspace.status();
    assert_eq!(status["works"], 1);
    assert_eq!(status["present"], 1);
    assert_eq!(status["unconfirmed_revisions"], 0);

    let hits = workspace.json(&["search", "casmurro"]);
    assert_eq!(hits[0]["title"], "dom-casmurro");
    assert_eq!(
        workspace.json(&["search", "smurr"])[0]["title"],
        "dom-casmurro"
    );

    let inspection = workspace.json(&["inspect", text_of(&path)]);
    let document = inspection["document"]["id"].as_i64().unwrap();
    assert_eq!(
        inspection["document"]["revisions"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(inspection["document"]["revisions"][0]["blake3"].is_string());

    workspace.run(&["note", "add", text_of(&path), "uma nota"]);
    workspace.run(&[
        "edit",
        text_of(&path),
        "--rating",
        "5",
        "--title",
        "Dom Casmurro",
    ]);
    let inspection = workspace.json(&["inspect", &format!("doc:{document}")]);
    assert_eq!(inspection["work"]["rating"], 5);
    assert_eq!(inspection["work"]["title"], "Dom Casmurro");
    assert_eq!(inspection["document"]["annotations"][0]["note"], "uma nota");

    // Renamed by someone else, and recognized without being read.
    let renamed = workspace.root.join("shelf").join("renamed.pdf");
    fs::rename(&path, &renamed).unwrap();
    let outcome = workspace.json(&["reconcile"]);
    assert_eq!(outcome["scans"][0]["moved"], 1);
    assert_eq!(outcome["scans"][0]["bytes_read"], 0);
    let inspection = workspace.json(&["inspect", text_of(&renamed)]);
    assert_eq!(inspection["document"]["id"], document);

    // Moved through the journal, and taken back.
    let moved = workspace.root.join("moved.pdf");
    workspace.run(&["move", text_of(&renamed), text_of(&moved)]);
    assert!(moved.exists() && !renamed.exists());
    let journal = workspace.json(&["journal"]);
    assert_eq!(journal[0]["kind"], "move_file");
    assert_eq!(journal[0]["state"], "done");
    workspace.run(&["undo"]);
    assert!(renamed.exists() && !moved.exists());

    // A folder, through the journal.
    let shelf = workspace.root.join("shelf");
    let shelf_renamed = workspace.root.join("prateleira");
    workspace.run(&["move", text_of(&shelf), text_of(&shelf_renamed)]);
    assert!(shelf_renamed.join("renamed.pdf").exists());
    let inspection = workspace.json(&["inspect", text_of(&shelf_renamed.join("renamed.pdf"))]);
    assert_eq!(inspection["document"]["id"], document);

    // To the Recycle Bin, and back, with the note still there.
    let in_shelf = shelf_renamed.join("renamed.pdf");
    workspace.run(&["recycle", text_of(&in_shelf)]);
    assert!(!in_shelf.exists());
    let listed = workspace.json(&["list", "--state", "absent"]);
    assert_eq!(listed.as_array().unwrap().len(), 1);
    workspace.run(&["undo"]);
    assert!(in_shelf.exists());
    let inspection = workspace.json(&["inspect", &format!("doc:{document}")]);
    assert_eq!(inspection["document"]["annotations"][0]["note"], "uma nota");
    assert_eq!(workspace.status()["absent"], 0);

    // Backups.
    let backup = workspace.json(&["backup", "now"]);
    let file = backup["path"].as_str().unwrap().to_owned();
    assert!(Path::new(&file).exists());
    assert_eq!(
        workspace
            .json(&["backup", "list"])
            .as_array()
            .unwrap()
            .len(),
        1
    );
    workspace.run(&["note", "add", text_of(&in_shelf), "depois do backup"]);
    let restored = workspace.text(&["backup", "restore", &file]);
    assert!(restored.contains("replaced"));
    let inspection = workspace.json(&["inspect", &format!("doc:{document}")]);
    assert_eq!(
        inspection["document"]["annotations"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

/// A child that is killed when it goes out of scope, so that a failed assertion leaves no process.
struct KillOnDrop(std::process::Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn looking_works_while_something_else_is_writing() {
    let workspace = Workspace::new();
    workspace.add_root();
    workspace.write("book.pdf", &pdf(1));
    workspace.run(&["reconcile"]);

    let mut watcher = KillOnDrop(
        workspace
            .command(&["watch"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    // Up and running once it says so, and whatever it says after that has to go somewhere.
    let mut output = BufReader::new(watcher.0.stdout.take().unwrap());
    let mut first_line = String::new();
    output.read_line(&mut first_line).unwrap();
    assert!(first_line.contains("watching"), "{first_line}");
    std::thread::spawn(move || std::io::copy(&mut output, &mut std::io::sink()));

    // It holds the library's lock, so writing is refused to anyone else.
    let elsewhere = workspace.dir.join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    let refused = workspace.fails(&["root", "add", text_of(&elsewhere)]);
    assert!(refused.contains("already writing"), "{refused}");

    assert_eq!(workspace.status()["works"], 1);
    assert_eq!(workspace.json(&["search", "book"])[0]["title"], "book");

    // It keeps up with the disk by itself.
    workspace.write("new.pdf", &pdf(2));
    let deadline = Instant::now() + Duration::from_secs(30);
    while workspace.status()["works"] != 2 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(workspace.status()["works"], 2);

    watcher.0.kill().unwrap();
    watcher.0.wait().unwrap();
    // And the lock went with it.
    workspace.run(&["reconcile"]);
}

#[test]
fn a_process_killed_in_the_middle_of_anything_loses_nothing() {
    let workspace = Workspace::new();
    workspace.add_root();
    const FILES: u64 = 1500;
    for number in 0..FILES {
        workspace.write(
            &format!("shelf{}/book{number}.pdf", number % 20),
            &pdf(number),
        );
    }

    // Killed at moments spread over the whole of the work, each time picking up where the last left.
    let mut killed_while_running = 0;
    for delay_ms in [30, 60, 100, 150, 220, 300, 400, 550, 700, 900] {
        let mut child = workspace
            .command(&["reconcile"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(delay_ms));
        if child.try_wait().unwrap().is_none() {
            killed_while_running += 1;
        }
        let _ = child.kill();
        child.wait().unwrap();

        // Whatever moment that was, the database is whole and consistent.
        let db = workspace.db();
        let integrity: String = db
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .unwrap();
        assert_eq!(integrity, "ok", "killed after {delay_ms} ms");
        let broken: i64 = db
            .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(broken, 0, "killed after {delay_ms} ms");
        let half_written: i64 = db
            .query_row(
                "SELECT count(*) FROM work w WHERE NOT EXISTS (
                     SELECT 1 FROM document d JOIN revision r ON r.document_id = d.id
                     JOIN file_instance i ON i.revision_id = r.id WHERE d.work_id = w.id)",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(half_written, 0, "killed after {delay_ms} ms");
    }
    assert!(
        killed_while_running > 0,
        "the test only means something if it hit the work"
    );

    let outcome = workspace.json(&["reconcile"]);
    let status = workspace.status();
    assert_eq!(status["present"], FILES, "{outcome}");
    assert_eq!(status["documents"], FILES, "nothing counted twice");
    assert_eq!(status["absent"], 0);
    assert_eq!(status["unconfirmed_revisions"], 0);
    assert_eq!(status["unfinished_operations"], 0);

    let unchanged = workspace.json(&["reconcile"]);
    assert_eq!(unchanged["scans"][0]["unchanged"], FILES);
    assert_eq!(unchanged["scans"][0]["bytes_read"], 0);
}

#[test]
fn a_journal_entry_left_by_a_crash_is_finished_at_the_next_start_and_said_so() {
    let workspace = Workspace::new();
    workspace.add_root();
    let path = workspace.write("book.pdf", &pdf(1));
    workspace.run(&["reconcile"]);
    let target = workspace.root.join("renamed.pdf");

    {
        let db = workspace.db();
        db.execute(
            "INSERT INTO fs_operation (kind, state, instance_id, source_path, target_path, undo_data, created_at)
             SELECT 'move_file', 'intent', id, ?1, ?2, '{}', 0 FROM file_instance",
            [text_of(&path), text_of(&target)],
        )
        .unwrap();
    }

    let output = workspace.run(&["reconcile"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("what a crash interrupted"), "{stderr}");
    assert_eq!(workspace.json(&["journal"])[0]["state"], "aborted");
}

#[test]
fn mistakes_get_a_message_and_a_failure_code() {
    let workspace = Workspace::new();
    assert!(workspace.fails(&["status"]).contains("no library"));
    workspace.add_root();
    assert!(
        workspace
            .fails(&["inspect", "doc:999"])
            .contains("no document")
    );
    assert!(
        workspace
            .fails(&["inspect", "C:\\nothing.pdf"])
            .contains("nothing at")
    );
    assert!(
        workspace
            .fails(&["root", "remove", "999"])
            .contains("no root")
    );
    assert!(workspace.fails(&["undo"]).contains("nothing to undo"));
    assert!(
        workspace
            .fails(&["list", "--state", "wat"])
            .contains("not a state")
    );

    let output = workspace.command(&["nonsense"]).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("Usage:"));
}

#[test]
fn reading_the_keystrokes_times_each_one() {
    let workspace = Workspace::new();
    workspace.add_root();
    workspace.write("dom casmurro.pdf", &pdf(1));
    workspace.run(&["reconcile"]);

    let strokes = workspace.json(&["search", "dom", "--keystrokes"]);
    let strokes = strokes.as_array().unwrap();
    assert_eq!(strokes.len(), 3);
    assert_eq!(strokes[2]["typed"], "dom");
    assert_eq!(strokes[2]["results"], 1);
    assert!(strokes[0]["milliseconds"].as_f64().unwrap() >= 0.0);

    let text = workspace.text(&["search", "dom", "--keystrokes"]);
    assert!(text.contains("slowest keystroke"));
}

#[test]
fn metadata_comes_through_the_sandboxed_worker_when_it_is_next_to_the_program() {
    let workspace = Workspace::new();
    workspace.add_root();
    let path = workspace.write(
        "nome.epub",
        &epub_bytes(
            &EpubMeta {
                title: "Título de Dentro",
                creators: &["Alguém"],
                identifier: Some("urn:x:1"),
                language: Some("pt"),
                date: Some("2001"),
            },
            500,
            1,
        ),
    );
    let worker = Path::new(env!("CARGO_BIN_EXE_musaeum")).with_file_name("musaeum-ingest.exe");
    let outcome = workspace.json(&["reconcile"]);
    assert_eq!(
        outcome["content"]["hashed"], 1,
        "hashing does not depend on the worker"
    );

    let inspection = workspace.json(&["inspect", text_of(&path)]);
    if worker.exists() {
        assert_eq!(inspection["work"]["title"], "Título de Dentro");
        assert_eq!(inspection["document"]["dc_identifier"], "urn:x:1");
    } else {
        // `cargo test -p musaeum-cli` does not build the worker, which lives in another package.
        assert_eq!(inspection["work"]["title"], "nome");
    }
}

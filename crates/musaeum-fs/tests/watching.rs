//! Against the real system, including a real overflow of its buffer.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use musaeum_fs::{DEFAULT_WATCH_BUFFER, DirectoryWatch, WatchEvent, watch_directory};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let number = NEXT.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("musaeum-watch-{}-{number}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn watch(dir: &Path, buffer: usize) -> (DirectoryWatch, Receiver<WatchEvent>) {
    let (sender, receiver) = mpsc::channel();
    let watch = watch_directory(dir, buffer, move |event| {
        let _ = sender.send(event);
    })
    .unwrap();
    (watch, receiver)
}

/// Waits until an event satisfies `matches`, returning whether one did in time.
fn saw(events: &Receiver<WatchEvent>, matches: impl Fn(&WatchEvent) -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match events.recv_timeout(left) {
            Ok(event) if matches(&event) => return true,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
    false
}

#[test]
fn changes_anywhere_below_are_reported_with_their_path() {
    let dir = TempDir::new();
    let inner = dir.0.join("inner");
    fs::create_dir(&inner).unwrap();
    let (_watch, events) = watch(&dir.0, DEFAULT_WATCH_BUFFER);

    let file = inner.join("book.pdf");
    fs::write(&file, b"x").unwrap();
    assert!(saw(&events, |event| {
        matches!(event, WatchEvent::Changed(path) if *path == file)
    }));

    let renamed = inner.join("renamed.pdf");
    fs::rename(&file, &renamed).unwrap();
    assert!(saw(&events, |event| {
        matches!(event, WatchEvent::Changed(path) if *path == renamed)
    }));
}

#[test]
fn a_change_made_the_moment_the_watch_returns_is_seen() {
    // The system records changes only from the first look on. A watch that returned before that
    // look would miss this, now and then, whenever its thread was slow to start.
    for _ in 0..30 {
        let dir = TempDir::new();
        let (_watch, events) = watch(&dir.0, DEFAULT_WATCH_BUFFER);
        let file = dir.0.join("at-once.pdf");
        fs::write(&file, b"x").unwrap();
        assert!(saw(&events, |event| {
            matches!(event, WatchEvent::Changed(path) if *path == file)
        }));
    }
}

#[test]
fn a_real_overflow_is_reported_and_the_watch_goes_on() {
    let dir = TempDir::new();
    // Too small for even one record, so every change overflows it, and no timing is involved.
    let (_watch, events) = watch(&dir.0, 4);

    fs::write(dir.0.join("a.pdf"), b"x").unwrap();
    assert!(saw(&events, |event| matches!(event, WatchEvent::Overflow)));

    // Unlike the `notify` crate's, this watch is still alive after that. What the system has to say
    // about the first file can take a moment to run out, so this is about a change made afterwards.
    thread::sleep(Duration::from_millis(200));
    while events.try_recv().is_ok() {}
    fs::write(dir.0.join("b.pdf"), b"x").unwrap();
    assert!(saw(&events, |event| matches!(event, WatchEvent::Overflow)));
}

#[test]
fn a_burst_that_piles_up_behind_a_busy_watch_overflows_a_normal_buffer_too() {
    let dir = TempDir::new();
    let (sender, events) = mpsc::channel();
    let (release, released) = mpsc::channel::<()>();
    let mut first = true;
    // Held up on its first event, as a watch is whenever what receives its events is busy.
    let _watch = watch_directory(&dir.0, 1024, move |event| {
        if first {
            first = false;
            let _ = released.recv();
        }
        let _ = sender.send(event);
    })
    .unwrap();

    fs::write(dir.0.join("first.pdf"), b"x").unwrap();
    thread::sleep(Duration::from_millis(100));
    for number in 0..2000 {
        fs::write(dir.0.join(format!("{number}.pdf")), b"x").unwrap();
    }
    release.send(()).unwrap();

    assert!(saw(&events, |event| matches!(event, WatchEvent::Overflow)));
}

#[test]
fn deleting_the_directory_ends_the_watch_and_says_so() {
    let dir = TempDir::new();
    let watched = dir.0.join("watched");
    fs::create_dir(&watched).unwrap();
    let (_watch, events) = watch(&watched, DEFAULT_WATCH_BUFFER);

    fs::remove_dir(&watched).unwrap();
    assert!(saw(&events, |event| matches!(event, WatchEvent::Lost(_))));
}

#[test]
fn watching_what_does_not_exist_is_an_error() {
    let dir = TempDir::new();
    assert!(watch_directory(&dir.0.join("none"), 1024, |_| {}).is_err());
}

#[test]
fn dropping_a_watch_stops_it_promptly_and_lets_the_directory_go() {
    let dir = TempDir::new();
    let watched = dir.0.join("watched");
    fs::create_dir(&watched).unwrap();
    let (watch, _events) = watch(&watched, DEFAULT_WATCH_BUFFER);

    let start = Instant::now();
    drop(watch);
    assert!(start.elapsed() < Duration::from_secs(2));
    fs::remove_dir(&watched).unwrap();
}

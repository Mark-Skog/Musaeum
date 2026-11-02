//! Measures the library at scale, over real files on a real disk.
//!
//! Generates a catalog of books that really exist as files, EPUBs and PDFs with bytes of their own,
//! runs all of the pipeline over them, and then types searches into the result one keystroke at a
//! time. The vocabulary is skewed the way real titles are, so that common words have the long lists
//! that make search slow, and some of the queries are chosen to be as bad as possible.
//!
//! ```text
//! library-bench [--works N] [--dir DIR] [--out FILE] [--keep] [--reuse | --reuse-books]
//!               [--normal-priority]
//! ```
//!
//! `--reuse` skips generating and indexing, and goes straight to the search, over what a run with
//! `--keep` left behind. `--reuse-books` only skips generating, and indexes those books from nothing.

mod catalog;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use catalog::{Catalog, Random};
use musaeum_library::{
    DEFAULT_SEARCH_LIMIT, Library, LibraryOptions, LibraryPaths, ReconcileScope, WorkEdit,
};
use serde::Serialize;

/// The target in the plan.
const TARGET_MS: f64 = 30.0;

struct Options {
    works: usize,
    dir: PathBuf,
    out: Option<PathBuf>,
    keep: bool,
    reuse: bool,
    reuse_books: bool,
    normal_priority: bool,
}

fn parse_options() -> Result<Options, String> {
    let mut options = Options {
        works: 100_000,
        dir: std::env::temp_dir().join("musaeum-library-bench"),
        out: None,
        keep: false,
        reuse: false,
        reuse_books: false,
        normal_priority: false,
    };
    let mut arguments = std::env::args_os().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--works") => {
                options.works = arguments
                    .next()
                    .and_then(|value| value.to_str()?.parse().ok())
                    .ok_or("--works takes a number")?;
            }
            Some("--dir") => {
                options.dir = arguments
                    .next()
                    .map(PathBuf::from)
                    .ok_or("--dir takes a folder")?;
            }
            Some("--out") => {
                options.out = Some(
                    arguments
                        .next()
                        .map(PathBuf::from)
                        .ok_or("--out takes a file")?,
                );
            }
            Some("--keep") => options.keep = true,
            Some("--reuse") => options.reuse = true,
            Some("--reuse-books") => options.reuse_books = true,
            Some("--normal-priority") => options.normal_priority = true,
            other => return Err(format!("{other:?} is not an option")),
        }
    }
    Ok(options)
}

/// What the percentiles of some keystrokes are.
#[derive(Clone, Debug, Serialize)]
struct Latency {
    keystrokes: usize,
    mean_ms: f64,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    max_ms: f64,
    over_target: usize,
    slowest_query: String,
}

fn latency_of(samples: &[(String, f64)]) -> Latency {
    let mut times: Vec<f64> = samples.iter().map(|(_, ms)| *ms).collect();
    times.sort_by(f64::total_cmp);
    let percentile = |p: f64| -> f64 {
        if times.is_empty() {
            return 0.0;
        }
        times[((times.len() - 1) as f64 * p).round() as usize]
    };
    let slowest = samples
        .iter()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(query, _)| query.clone())
        .unwrap_or_default();
    Latency {
        keystrokes: times.len(),
        mean_ms: times.iter().sum::<f64>() / times.len().max(1) as f64,
        p50_ms: percentile(0.50),
        p95_ms: percentile(0.95),
        p99_ms: percentile(0.99),
        max_ms: percentile(1.0),
        over_target: times.iter().filter(|ms| **ms > TARGET_MS).count(),
        slowest_query: slowest,
    }
}

fn print_latency(name: &str, latency: &Latency) {
    println!(
        "  {name:<26} {:>6}  mean {:>6.2}  p50 {:>6.2}  p95 {:>6.2}  p99 {:>6.2}  max {:>7.2}  over {TARGET_MS} ms: {:>4}   (slowest: {:?})",
        latency.keystrokes,
        latency.mean_ms,
        latency.p50_ms,
        latency.p95_ms,
        latency.p99_ms,
        latency.max_ms,
        latency.over_target,
        latency.slowest_query,
    );
}

/// Every prefix of a query, each as a keystroke, the way someone would type it.
fn type_query(library: &Library, query: &str, samples: &mut Vec<(String, f64)>) -> usize {
    let mut typed = String::new();
    let mut found = 0;
    for character in query.chars() {
        typed.push(character);
        let started = Instant::now();
        let hits = library
            .search(&typed, DEFAULT_SEARCH_LIMIT)
            .expect("a search does not fail");
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        found = hits.len();
        samples.push((typed.clone(), elapsed));
    }
    found
}

/// What gets typed, by kind. Each kind is a different way for search to be slow.
fn query_classes(catalog: &Catalog) -> Vec<(&'static str, Vec<String>)> {
    let mut random = Random::new(2026);
    let pick = |random: &mut Random| catalog.entry(random.below(catalog.len()));

    let titles = (0..300).map(|_| pick(&mut random).title).collect();
    let authors = (0..300)
        .map(|_| pick(&mut random).creators.join(" "))
        .collect();
    let two_words = (0..200)
        .map(|_| {
            let entry = pick(&mut random);
            let surname = entry.creators[0]
                .split(' ')
                .next_back()
                .unwrap_or("")
                .to_owned();
            let word = entry.title.split(' ').next().unwrap_or("").to_owned();
            format!("{word} {surname}")
        })
        .collect();
    let substrings = (0..200)
        .map(|_| {
            let title = pick(&mut random).title;
            let word = title
                .split(' ')
                .max_by_key(|word| word.chars().count())
                .unwrap_or("")
                .to_owned();
            let characters: Vec<char> = word.chars().collect();
            let start = characters.len().min(2);
            characters[start..].iter().collect()
        })
        .collect();
    let unaccented = (0..200)
        .map(|_| catalog::without_accents(&pick(&mut random).title))
        .collect();
    let one_letter = "abcdefghijklmnopqrstuvwxyzáéíóúçñ"
        .chars()
        .map(String::from)
        .collect();
    let common = catalog::COMMON_FRAGMENTS
        .iter()
        .map(|text| (*text).to_owned())
        .collect();
    let nothing = (0..100)
        .map(|_| {
            (0..12)
                .map(|_| char::from(b"qxzwkjvy"[random.below(8)]))
                .collect()
        })
        .collect();
    let pairs_of_common_words = catalog::common_word_pairs(120);
    vec![
        ("title typed from the start", titles),
        ("author typed", authors),
        ("title word + surname", two_words),
        ("middle of a word", substrings),
        ("without accents", unaccented),
        ("one letter", one_letter),
        ("very common fragments", common),
        ("two very common words", pairs_of_common_words),
        ("matches nothing", nothing),
    ]
}

#[derive(Debug, Serialize)]
struct ClassResult {
    class: String,
    latency: Latency,
}

fn measure_search(
    library: &Library,
    catalog: &Catalog,
    label: &str,
) -> (Vec<ClassResult>, Latency) {
    println!("search, {label} (ms per keystroke):");
    let mut results = Vec::new();
    let mut everything = Vec::new();
    for (class, queries) in query_classes(catalog) {
        let mut samples = Vec::new();
        for query in &queries {
            type_query(library, query, &mut samples);
        }
        let latency = latency_of(&samples);
        print_latency(class, &latency);
        results.push(ClassResult {
            class: class.to_owned(),
            latency,
        });
        everything.extend(samples);
    }
    let total = latency_of(&everything);
    print_latency("ALL", &total);
    (results, total)
}

/// The most this process has had in memory, in bytes.
fn peak_working_set() -> u64 {
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let mut counters = PROCESS_MEMORY_COUNTERS {
        cb: size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ..PROCESS_MEMORY_COUNTERS::default()
    };
    // SAFETY: the pseudo-handle is always valid, and the size is the structure's own.
    unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) };
    counters.PeakWorkingSetSize as u64
}

fn working_set() -> u64 {
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let mut counters = PROCESS_MEMORY_COUNTERS {
        cb: size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ..PROCESS_MEMORY_COUNTERS::default()
    };
    // SAFETY: as above.
    unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) };
    counters.WorkingSetSize as u64
}

fn folder_size(path: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| {
            let metadata = entry.metadata();
            if metadata.as_ref().is_ok_and(fs::Metadata::is_dir) {
                folder_size(&entry.path())
            } else {
                metadata.map_or(0, |metadata| metadata.len())
            }
        })
        .sum()
}

#[derive(Debug, Serialize)]
struct IndexReport {
    generated_bytes: u64,
    generate_seconds: f64,
    scan_seconds: f64,
    scan_files_per_second: f64,
    hash_seconds: f64,
    hash_files_per_second: f64,
    hash_megabytes_per_second: f64,
    total_index_seconds: f64,
    rescan_nothing_changed_seconds: f64,
    rescan_bytes_read: u64,
    peak_working_set_bytes: u64,
    working_set_after_indexing_bytes: u64,
    backup_seconds: f64,
    backup_bytes: u64,
    folder_rename_seconds: f64,
    folder_rename_files_in_it: u64,
}

#[derive(Debug, Serialize)]
struct Report {
    works: usize,
    low_priority: bool,
    index: Option<IndexReport>,
    reopen_ms: f64,
    first_search_after_open_ms: f64,
    library_db_bytes: u64,
    cache_db_bytes: u64,
    search_idle: Vec<ClassResult>,
    search_idle_all: Latency,
    search_while_writing: Vec<ClassResult>,
    search_while_writing_all: Latency,
    writes_during_search: u64,
    write_latency: Latency,
    target_ms: f64,
    target_met_p99: bool,
    target_met_max: bool,
}

fn main() {
    let options = match parse_options() {
        Ok(options) => options,
        Err(message) => {
            eprintln!("library-bench: {message}");
            std::process::exit(2);
        }
    };
    if let Err(error) = run(&options) {
        eprintln!("library-bench: {error}");
        std::process::exit(1);
    }
}

fn library_options(options: &Options) -> LibraryOptions {
    LibraryOptions {
        low_priority: !options.normal_priority,
        ..LibraryOptions::default()
    }
}

/// Everything up to the search: books as files, a scan, hashes, and what happens to them afterwards.
fn generate_and_index(
    options: &Options,
    catalog: &Catalog,
    books: &Path,
    library_dir: &Path,
) -> Result<IndexReport, Box<dyn std::error::Error>> {
    let (generated_bytes, generate_seconds) = if options.reuse_books {
        if library_dir.exists() {
            fs::remove_dir_all(library_dir)?;
        }
        (folder_size(books), 0.0)
    } else {
        if options.dir.exists() {
            fs::remove_dir_all(&options.dir)?;
        }
        fs::create_dir_all(books)?;
        println!(
            "generating {} books under {}",
            options.works,
            books.display()
        );
        let started = Instant::now();
        let bytes = catalog.write_all(books)?;
        (bytes, started.elapsed().as_secs_f64())
    };
    println!(
        "books: {:.1} s to generate, {:.0} MiB",
        generate_seconds,
        generated_bytes as f64 / 1_048_576.0
    );

    let library = Library::open(
        LibraryPaths::in_directory(library_dir),
        library_options(options),
    )?;
    let root = library.add_root(books, &[])?;

    println!("first scan");
    let started = Instant::now();
    let scan = library.reconcile(root.id, &ReconcileScope::Everything)?;
    let scan_seconds = started.elapsed().as_secs_f64();
    println!(
        "  {scan_seconds:.1} s, {:.0} files/s: {} new, {} MiB read for fingerprints",
        options.works as f64 / scan_seconds,
        scan.new_files,
        scan.bytes_read / 1_048_576
    );
    assert_eq!(scan.new_files, options.works as u64, "every book was found");

    println!("full hashes and metadata");
    let started = Instant::now();
    let content = library.process_content()?;
    let hash_seconds = started.elapsed().as_secs_f64();
    println!(
        "  {hash_seconds:.1} s, {:.0} files/s, {:.0} MB/s: {} hashed, {} with metadata, {} refused, {} workers replaced",
        options.works as f64 / hash_seconds,
        content.bytes_hashed as f64 / 1_000_000.0 / hash_seconds,
        content.hashed,
        content.extracted,
        content.extraction_failed,
        content.worker_restarts,
    );

    let status = library.status()?;
    println!(
        "library: {} works, {} present, {} waiting for a hash, {} absent",
        status.works, status.present, status.unconfirmed_revisions, status.absent
    );
    assert_eq!(status.works, options.works as u64);
    assert_eq!(status.present, options.works as u64);
    assert_eq!(status.unconfirmed_revisions, 0);

    println!("a second scan, with nothing changed");
    let started = Instant::now();
    let rescan = library.reconcile(root.id, &ReconcileScope::Everything)?;
    let rescan_seconds = started.elapsed().as_secs_f64();
    println!(
        "  {rescan_seconds:.2} s, {} unchanged, {} bytes read",
        rescan.unchanged, rescan.bytes_read
    );
    assert_eq!(rescan.unchanged, options.works as u64);
    assert_eq!(rescan.bytes_read, 0);

    println!("renaming a folder with a good part of the library in it");
    let shelf = catalog.largest_shelf(books);
    let renamed = shelf.with_file_name("renamed-shelf");
    let files_in_shelf = catalog::count_files(&shelf);
    fs::rename(&shelf, &renamed)?;
    let started = Instant::now();
    let moved = library.reconcile(root.id, &ReconcileScope::Everything)?;
    let folder_rename_seconds = started.elapsed().as_secs_f64();
    println!(
        "  {folder_rename_seconds:.2} s for {files_in_shelf} files in it: {} folders moved, {} files touched, {} bytes read",
        moved.folders_moved, moved.moved, moved.bytes_read
    );
    assert_eq!(moved.folders_moved, 1);
    assert_eq!(moved.moved, 0);
    assert_eq!(moved.bytes_read, 0);
    fs::rename(&renamed, &shelf)?;
    library.reconcile(root.id, &ReconcileScope::Everything)?;

    println!("backup");
    let started = Instant::now();
    let backup = library.backup_now()?;
    let backup_seconds = started.elapsed().as_secs_f64();
    println!(
        "  {backup_seconds:.2} s, {:.0} MiB",
        backup.size as f64 / 1_048_576.0
    );

    let peak_working_set_bytes = peak_working_set();
    let working_set_after_indexing_bytes = working_set();
    println!(
        "memory: peak {:.0} MiB, now {:.0} MiB",
        peak_working_set_bytes as f64 / 1_048_576.0,
        working_set_after_indexing_bytes as f64 / 1_048_576.0,
    );
    Ok(IndexReport {
        generated_bytes,
        generate_seconds,
        scan_seconds,
        scan_files_per_second: options.works as f64 / scan_seconds,
        hash_seconds,
        hash_files_per_second: options.works as f64 / hash_seconds,
        hash_megabytes_per_second: content.bytes_hashed as f64 / 1_000_000.0 / hash_seconds,
        total_index_seconds: scan_seconds + hash_seconds,
        rescan_nothing_changed_seconds: rescan_seconds,
        rescan_bytes_read: rescan.bytes_read,
        peak_working_set_bytes,
        working_set_after_indexing_bytes,
        backup_seconds,
        backup_bytes: backup.size,
        folder_rename_seconds,
        folder_rename_files_in_it: files_in_shelf,
    })
}

fn run(options: &Options) -> Result<(), Box<dyn std::error::Error>> {
    let books = options.dir.join("books");
    let library_dir = options.dir.join("library");
    let catalog = Catalog::new(options.works);

    let index = if options.reuse {
        None
    } else {
        Some(generate_and_index(options, &catalog, &books, &library_dir)?)
    };

    let library_db_bytes = fs::metadata(library_dir.join("library.db"))?.len();
    let cache_db_bytes = folder_size(&library_dir.join("cache"));
    println!(
        "library.db {:.0} MiB, cache {:.0} MiB",
        library_db_bytes as f64 / 1_048_576.0,
        cache_db_bytes as f64 / 1_048_576.0,
    );

    // A fresh start, as when the program opens.
    let started = Instant::now();
    let library = Arc::new(Library::open(
        LibraryPaths::in_directory(&library_dir),
        library_options(options),
    )?);
    let reopen_ms = started.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    library.search("a", DEFAULT_SEARCH_LIMIT)?;
    let first_search_after_open_ms = started.elapsed().as_secs_f64() * 1000.0;
    println!(
        "opening the library: {reopen_ms:.1} ms; first search after that: {first_search_after_open_ms:.2} ms"
    );

    let (search_idle, search_idle_all) = measure_search(&library, &catalog, "nothing else running");

    // The same again with the only writer busy all the time, and each write touching the search
    // indexes, which is the worst thing it can be doing. What it goes through is measured too, because
    // a read held up by a write would show up in both.
    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let library = Arc::clone(&library);
        let stop = Arc::clone(&stop);
        let works = options.works;
        thread::spawn(move || {
            let mut random = Random::new(7);
            let mut samples = Vec::new();
            while !stop.load(Ordering::Relaxed) {
                let work = random.below(works) as i64 + 1;
                let edit = WorkEdit {
                    title: Some(format!("Editado {}", random.below(1_000_000))),
                    ..WorkEdit::default()
                };
                let started = Instant::now();
                if library.edit_work(work, edit).is_ok() {
                    let elapsed = started.elapsed().as_secs_f64() * 1000.0;
                    samples.push((format!("work {work}"), elapsed));
                }
                thread::sleep(Duration::from_millis(1));
            }
            samples
        })
    };
    let (search_while_writing, search_while_writing_all) =
        measure_search(&library, &catalog, "with the writer busy");
    stop.store(true, Ordering::Relaxed);
    let writes = writer.join().map_err(|_| "the writer panicked")?;
    let writes_during_search = writes.len() as u64;
    let write_latency = latency_of(&writes);
    println!("writes committed meanwhile (ms per commit):");
    print_latency("writes", &write_latency);

    let report = Report {
        works: options.works,
        low_priority: !options.normal_priority,
        index,
        reopen_ms,
        first_search_after_open_ms,
        library_db_bytes,
        cache_db_bytes,
        target_ms: TARGET_MS,
        target_met_p99: search_idle_all.p99_ms <= TARGET_MS
            && search_while_writing_all.p99_ms <= TARGET_MS,
        target_met_max: search_idle_all.max_ms <= TARGET_MS
            && search_while_writing_all.max_ms <= TARGET_MS,
        search_idle,
        search_idle_all,
        search_while_writing,
        search_while_writing_all,
        writes_during_search,
        write_latency,
    };

    let out = options.out.clone().unwrap_or_else(|| {
        PathBuf::from("bench-results").join(format!("library-{}.json", options.works))
    });
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&out, serde_json::to_string_pretty(&report)?)?;
    println!("written to {}", out.display());
    println!(
        "target of {TARGET_MS} ms per keystroke: p99 {}, worst case {}",
        if report.target_met_p99 {
            "met"
        } else {
            "MISSED"
        },
        if report.target_met_max {
            "met"
        } else {
            "MISSED"
        },
    );

    drop(library);
    if !options.keep && !options.reuse && !options.reuse_books {
        fs::remove_dir_all(&options.dir)?;
    }
    Ok(())
}

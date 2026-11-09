//! Normalizes one book in this process and prints what came out. For development only: the app
//! normalizes in the isolated ingest worker, never in its own process.
//!
//! ```text
//! cargo run --release -p musaeum-flow --example normalize -- BOOK [OUT_DIR | OUT.epub]
//! ```

use std::path::Path;
use std::time::Instant;

use musaeum_flow::normalize::{Options, normalize};
use musaeum_flow::package::{NavPoint, SourceFormat};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(input) = args.first() else {
        eprintln!("usage: normalize BOOK [OUT_DIR | OUT.epub]");
        std::process::exit(2);
    };
    let extension = Path::new(input)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let format = match extension.as_str() {
        "epub" => SourceFormat::Epub,
        "mobi" | "prc" | "azw" => SourceFormat::Mobi,
        "azw3" => SourceFormat::Azw3,
        "fb2" => SourceFormat::Fb2,
        "txt" => SourceFormat::Txt,
        "md" | "markdown" => SourceFormat::Markdown,
        other => {
            eprintln!("unknown extension {other}");
            std::process::exit(2);
        }
    };
    let bytes = std::fs::read(input).expect("reading the book");
    let started = Instant::now();
    match normalize(format, &bytes, &Options::default()) {
        Ok(book) => {
            let elapsed = started.elapsed();
            let m = &book.manifest;
            println!(
                "{}: {:?} {}; {} documents, {} parts ({} split, {} oversize, largest {} B), {} resources, {} files, toc {} entries, {:.1} ms",
                input,
                m.source.format,
                m.source.version.as_deref().unwrap_or("-"),
                m.stats.documents,
                m.stats.parts,
                m.stats.split_documents,
                m.stats.oversize_parts,
                m.stats.largest_part,
                m.stats.resources,
                book.files.len(),
                NavPoint::count(&m.toc),
                elapsed.as_secs_f64() * 1000.0
            );
            println!(
                "title: {:?}; creators: {:?}",
                m.metadata.title(),
                m.metadata
                    .creators
                    .iter()
                    .map(|c| &c.name)
                    .collect::<Vec<_>>()
            );
            println!("sanitized: {:?}", m.stats.sanitized);
            for warning in m.warnings.iter().take(10) {
                println!("warning: {warning}");
            }
            if let Some(out) = args.get(1) {
                if out.ends_with(".epub") {
                    std::fs::write(out, book.to_epub()).expect("writing the EPUB");
                } else {
                    for (path, data) in &book.files {
                        let target = Path::new(out).join(path);
                        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
                        std::fs::write(target, data).unwrap();
                    }
                }
            }
        }
        Err(error) => {
            println!("{input}: {error}");
            std::process::exit(1);
        }
    }
}

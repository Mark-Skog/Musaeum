//! Command line for the fixture generator.
//!
//! ```text
//! gen-fixtures [--out DIR] [--scan-pages N] [--vector-pages N] [--canary-port N] [--only flow]
//! ```

use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::Instant;
use std::{env, fs};

fn main() -> Result<(), Box<dyn Error>> {
    let mut out = PathBuf::from("fixtures");
    let mut scan_pages = 2000usize;
    let mut vector_pages = 500usize;
    let mut canary_port = 48780u16;
    let mut only: Option<String> = None;

    let mut args = env::args().skip(1);
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--out" => out = PathBuf::from(value()?),
            "--scan-pages" => scan_pages = value()?.parse()?,
            "--vector-pages" => vector_pages = value()?.parse()?,
            "--canary-port" => canary_port = value()?.parse()?,
            "--only" => only = Some(value()?),
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }

    fs::create_dir_all(&out)?;
    let report = |path: &Path, started: Instant| -> Result<(), Box<dyn Error>> {
        let megabytes = fs::metadata(path)?.len() as f64 / 1_048_576.0;
        println!(
            "{}: {megabytes:.1} MiB in {:.1} s",
            path.display(),
            started.elapsed().as_secs_f32()
        );
        Ok(())
    };

    if only.as_deref() == Some("flow") {
        let started = Instant::now();
        gen_fixtures::flow::write_flow_fixtures(&out)?;
        for name in [
            "flow.epub",
            "flow-epub2.epub",
            "flow.mobi",
            "flow.azw3",
            "flow.fb2",
            "flow-1251.fb2",
            "flow.txt",
            "flow.md",
        ] {
            report(&out.join(name), started)?;
        }
        return Ok(());
    }

    let started = Instant::now();
    let path = out.join("mixed.pdf");
    gen_fixtures::write_mixed_fixture(&path)?;
    report(&path, started)?;

    let started = Instant::now();
    let path = out.join("spreads.pdf");
    gen_fixtures::write_spread_fixture(&path)?;
    report(&path, started)?;

    let started = Instant::now();
    let path = out.join(format!("scan-{scan_pages}.pdf"));
    gen_fixtures::write_scan_fixture(&path, scan_pages)?;
    report(&path, started)?;

    let started = Instant::now();
    let path = out.join(format!("vector-{vector_pages}.pdf"));
    gen_fixtures::write_vector_fixture(&path, vector_pages)?;
    report(&path, started)?;

    let started = Instant::now();
    gen_fixtures::comics::write_comic_fixtures(&out)?;
    for name in ["comic.cbz", "comic-rar4.cbr", "comic-rar5.cbr", "manga.cbz"] {
        report(&out.join(name), started)?;
    }

    let started = Instant::now();
    let path = out.join("book.djvu");
    gen_fixtures::djvu::write_djvu_fixture(&path)?;
    report(&path, started)?;

    let started = Instant::now();
    let path = out.join("colors.pdf");
    gen_fixtures::color::write_color_fixture(&path)?;
    report(&path, started)?;

    let started = Instant::now();
    let path = out.join("content.pdf");
    gen_fixtures::write_content_fixture(&path)?;
    report(&path, started)?;

    let started = Instant::now();
    let path = out.join("large.epub");
    gen_fixtures::epub::write_large_epub(&path)?;
    report(&path, started)?;

    let started = Instant::now();
    let path = out.join("hostile.epub");
    gen_fixtures::epub::write_hostile_epub(&path, canary_port)?;
    report(&path, started)?;

    let started = Instant::now();
    gen_fixtures::flow::write_flow_fixtures(&out)?;
    for name in [
        "flow.epub",
        "flow-epub2.epub",
        "flow.mobi",
        "flow.azw3",
        "flow.fb2",
        "flow-1251.fb2",
        "flow.txt",
        "flow.md",
    ] {
        report(&out.join(name), started)?;
    }
    Ok(())
}

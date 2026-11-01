//! Command line for the fixture generator.
//!
//! ```text
//! gen-fixtures [--out DIR] [--scan-pages N] [--vector-pages N] [--canary-port N]
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

    let mut args = env::args().skip(1);
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--out" => out = PathBuf::from(value()?),
            "--scan-pages" => scan_pages = value()?.parse()?,
            "--vector-pages" => vector_pages = value()?.parse()?,
            "--canary-port" => canary_port = value()?.parse()?,
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

    let started = Instant::now();
    let path = out.join("mixed.pdf");
    gen_fixtures::write_mixed_fixture(&path)?;
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
    let path = out.join("large.epub");
    gen_fixtures::epub::write_large_epub(&path)?;
    report(&path, started)?;

    let started = Instant::now();
    let path = out.join("hostile.epub");
    gen_fixtures::epub::write_hostile_epub(&path, canary_port)?;
    report(&path, started)?;
    Ok(())
}

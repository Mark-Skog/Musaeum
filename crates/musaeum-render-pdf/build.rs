//! Copies the pinned PDFium DLL next to the worker binary, where the worker looks for
//! it. Loading it by absolute path keeps the DLL search order out of the picture.

use std::path::{Path, PathBuf};
use std::{env, fs};

const PDFIUM_DLL: &str = "../../vendor/pdfium/chromium-8125/bin/pdfium.dll";

fn main() {
    println!("cargo:rerun-if-changed={PDFIUM_DLL}");
    println!("cargo:rerun-if-changed=build.rs");

    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let source = manifest_dir.join(PDFIUM_DLL);
    if !source.exists() {
        println!(
            "cargo:warning=PDFium is missing. Run tools/fetch-pdfium.ps1 before starting the worker."
        );
        return;
    }

    // OUT_DIR is target/<profile>/build/<package>-<hash>/out.
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("set by cargo"));
    let profile_dir = out_dir
        .ancestors()
        .nth(3)
        .expect("OUT_DIR has the documented shape");
    copy_if_changed(&source, &profile_dir.join("pdfium.dll"));
}

fn copy_if_changed(source: &Path, target: &Path) {
    let unchanged = match (fs::metadata(source), fs::metadata(target)) {
        (Ok(a), Ok(b)) => a.len() == b.len() && a.modified().ok() == b.modified().ok(),
        _ => false,
    };
    if !unchanged {
        fs::copy(source, target).expect("copying pdfium.dll next to the worker");
    }
}

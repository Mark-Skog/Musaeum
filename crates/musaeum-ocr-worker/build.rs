//! Copies the recognition models next to the worker, into `tessdata/`, where the host looks for
//! them: the host reads them and sends them to the worker, which opens no file.

use std::path::{Path, PathBuf};
use std::{env, fs};

const MODELS: &str = "../../vendor/tesseract/tessdata";

fn main() {
    println!("cargo:rerun-if-changed={MODELS}");
    println!("cargo:rerun-if-changed=build.rs");
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let source = manifest_dir.join(MODELS);
    let Ok(entries) = fs::read_dir(&source) else {
        println!("cargo:warning=The OCR models are missing. Run tools/fetch-ocr.ps1.");
        return;
    };
    // OUT_DIR is target/<profile>/build/<package>-<hash>/out.
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("set by cargo"));
    let target = out_dir
        .ancestors()
        .nth(3)
        .expect("OUT_DIR has the documented shape")
        .join("tessdata");
    fs::create_dir_all(&target).expect("the profile folder is writable");
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let wanted = path
            .extension()
            .is_some_and(|extension| extension == "traineddata")
            || name == "LICENSE";
        if wanted {
            copy_if_changed(&path, &target.join(name));
        }
    }
}

fn copy_if_changed(source: &Path, target: &Path) {
    let unchanged = match (fs::metadata(source), fs::metadata(target)) {
        (Ok(a), Ok(b)) => a.len() == b.len() && a.modified().ok() == b.modified().ok(),
        _ => false,
    };
    if !unchanged {
        fs::copy(source, target).expect("copying a model next to the worker");
    }
}

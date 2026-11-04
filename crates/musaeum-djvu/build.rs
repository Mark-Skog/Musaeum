//! Builds DjVuLibre's library from `vendor/djvulibre` together with the C++ bridge in `shim/`,
//! with the definitions DjVuLibre's own Visual Studio project uses, minus the DLL exports: it is
//! linked statically, so the sandboxed worker loads no library of its own.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    assert_eq!(
        target_os, "windows",
        "only the Windows build of DjVuLibre is set up so far"
    );

    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("set by Cargo"));
    let library = manifest
        .join("..")
        .join("..")
        .join("vendor")
        .join("djvulibre")
        .join("libdjvu");
    let shim = manifest.join("shim");

    let mut build = cc::Build::new();
    build
        .cpp(true)
        .include(&library)
        .include(&shim)
        .define("HAVE_NAMESPACES", None)
        .define("WIN32", None)
        .define("NDEBUG", None)
        // Empty, so that nothing is declared as imported from or exported by a DLL.
        .define("DJVUAPI", "")
        .define("DDJVUAPI", "")
        .define("MINILISPAPI", "")
        .warnings(false);
    let mut sources: Vec<PathBuf> = fs::read_dir(&library)
        .expect("vendor/djvulibre/libdjvu is there")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|extension| extension == "cpp"))
        .collect();
    // The same order on every machine, so builds are reproducible.
    sources.sort();
    build.files(&sources);
    build.file(shim.join("reader.cpp"));
    if env::var_os("CARGO_FEATURE_ENCODE").is_some() {
        build.file(shim.join("encoder.cpp"));
    }
    build.compile("musaeum_djvulibre");

    // DjVuLibre looks in the registry for its message catalogs. Inside the sandbox it finds nothing,
    // which only means messages stay in English.
    println!("cargo:rustc-link-lib=advapi32");

    println!("cargo:rerun-if-changed={}", library.display());
    println!("cargo:rerun-if-changed={}", shim.display());
}

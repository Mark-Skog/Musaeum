//! Builds Leptonica and Tesseract from `vendor/tesseract`, which `tools/fetch-ocr.ps1` fills, with
//! the C++ bridge in `shim/`, all linked statically so the sandboxed OCR worker loads no library of
//! its own, as `musaeum-djvu` does with DjVuLibre.
//!
//! Leptonica is built without any image library: Tesseract only ever gets pixels from memory here,
//! so the stubs Leptonica has for reading files are all it needs. Tesseract is built the way its
//! CMake build does with `DISABLED_LEGACY_ENGINE`, `GRAPHICS_DISABLED` and `FAST_FLOAT`: only the
//! LSTM recognizer, no debugging window, single-precision networks. The source list is read from
//! Tesseract's own `cmake/SourceLists.cmake`, so it is exactly what upstream compiles. The SIMD
//! kernels are compiled with their own instruction sets, and Tesseract picks among them at run time
//! by what the processor has. `TESSERACT_IMAGEDATA_AS_PIX` keeps pictures in memory as pictures, so
//! nothing ever goes through a file.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::PathBuf;

const TESSERACT: &str = "tesseract-5.5.4";
const LEPTONICA: &str = "leptonica-1.87.0";
const VERSION: (u32, u32, u32) = (5, 5, 4);

/// Every `set(NAME files...)` of a CMake list file.
fn cmake_lists(text: &str) -> BTreeMap<String, Vec<String>> {
    let mut lists = BTreeMap::new();
    let mut current: Option<(String, Vec<String>)> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("set(") {
            let name = rest
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_owned();
            current = Some((name, Vec::new()));
            continue;
        }
        if line == ")" {
            if let Some((name, files)) = current.take() {
                lists.insert(name, files);
            }
            continue;
        }
        if let Some((_, files)) = current.as_mut() {
            files.push(line.to_owned());
        }
    }
    lists
}

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    assert!(
        target_os == "windows" && target_env == "msvc",
        "only the Windows build with MSVC of Tesseract is set up so far"
    );

    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("set by Cargo"));
    let vendor = manifest
        .join("..")
        .join("..")
        .join("vendor")
        .join("tesseract");
    let tesseract = vendor.join(TESSERACT);
    let leptonica = vendor.join(LEPTONICA);
    assert!(
        tesseract.join("src").is_dir() && leptonica.join("src").is_dir(),
        "{} has no sources: run tools/fetch-ocr.ps1 first",
        vendor.display()
    );
    let out = PathBuf::from(env::var("OUT_DIR").expect("set by Cargo"));
    let shim = manifest.join("shim");

    // Leptonica, with every image library off.
    let lepton_src = leptonica.join("src");
    let mut lept_sources: Vec<PathBuf> = fs::read_dir(&lepton_src)
        .expect("leptonica's src is there")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|extension| extension == "c"))
        .collect();
    // The same order on every machine, so builds are reproducible.
    lept_sources.sort();
    // What Leptonica's configure would make of endianness.h.in on x86-64.
    let lept_generated = out.join("leptonica");
    fs::create_dir_all(&lept_generated).expect("OUT_DIR is writable");
    fs::write(
        lept_generated.join("endianness.h"),
        "#if !defined (L_BIG_ENDIAN) && !defined (L_LITTLE_ENDIAN)
#define L_LITTLE_ENDIAN
#endif
",
    )
    .expect("OUT_DIR is writable");
    let mut lept = cc::Build::new();
    lept.include(&lept_generated)
        .include(&lepton_src)
        .files(&lept_sources)
        .define("NDEBUG", None)
        .define("_CRT_SECURE_NO_WARNINGS", None)
        .warnings(false)
        .flag_if_supported("/utf-8");
    for library in [
        "HAVE_LIBJPEG",
        "HAVE_LIBTIFF",
        "HAVE_LIBPNG",
        "HAVE_LIBZ",
        "HAVE_LIBGIF",
        "HAVE_LIBUNGIF",
        "HAVE_LIBWEBP",
        "HAVE_LIBWEBP_ANIM",
        "HAVE_LIBJP2K",
    ] {
        lept.define(library, "0");
    }
    lept.compile("musaeum_leptonica");

    // What CMake would generate: the configuration and the version header.
    let generated = out.join("generated");
    fs::create_dir_all(generated.join("tesseract")).expect("OUT_DIR is writable");
    fs::write(
        generated.join("config_auto.h"),
        format!(
            "#define PACKAGE_VERSION \"{}.{}.{}\"\n#define GRAPHICS_DISABLED 1\n#define FAST_FLOAT 1\n#define DISABLED_LEGACY_ENGINE 1\n",
            VERSION.0, VERSION.1, VERSION.2
        ),
    )
    .expect("OUT_DIR is writable");
    let template = fs::read_to_string(tesseract.join("include/tesseract/version.h.in"))
        .expect("version.h.in is there");
    fs::write(
        generated.join("tesseract").join("version.h"),
        template
            .replace("@GENERIC_MAJOR_VERSION@", &VERSION.0.to_string())
            .replace("@GENERIC_MINOR_VERSION@", &VERSION.1.to_string())
            .replace("@GENERIC_MICRO_VERSION@", &VERSION.2.to_string())
            .replace(
                "@PACKAGE_VERSION@",
                &format!("{}.{}.{}", VERSION.0, VERSION.1, VERSION.2),
            ),
    )
    .expect("OUT_DIR is writable");

    let lists = cmake_lists(
        &fs::read_to_string(tesseract.join("cmake/SourceLists.cmake"))
            .expect("SourceLists.cmake is there"),
    );
    let list = |name: &str| -> Vec<String> {
        lists
            .get(name)
            .unwrap_or_else(|| panic!("SourceLists.cmake has no {name}"))
            .clone()
    };
    let legacy = list("TESSERACT_SRC_LEGACY");
    let mut core: Vec<String> = [
        "TESSERACT_SRC_API",
        "TESSERACT_SRC_ARCH",
        "TESSERACT_SRC_CCMAIN",
        "TESSERACT_SRC_CCSTRUCT",
        "TESSERACT_SRC_CCUTIL",
        "TESSERACT_SRC_CLASSIFY",
        "TESSERACT_SRC_CUTIL",
        "TESSERACT_SRC_DICT",
        "TESSERACT_SRC_LSTM",
        "TESSERACT_SRC_TEXTORD",
        "TESSERACT_SRC_VIEWER",
        "TESSERACT_SRC_WORDREC",
    ]
    .iter()
    .flat_map(|name| list(name))
    .filter(|file| !legacy.contains(file))
    .collect();
    core.sort();
    core.dedup();

    let modules = [
        "arch", "ccmain", "ccstruct", "ccutil", "classify", "cutil", "dict", "lstm", "textord",
        "viewer", "wordrec",
    ];
    let base = |optimized: &[&str]| -> cc::Build {
        let mut build = cc::Build::new();
        build
            .cpp(true)
            .std("c++17")
            .include(&generated)
            .include(tesseract.join("include"))
            .include(tesseract.join("src"))
            .include(&lept_generated)
            .include(&lepton_src)
            .define("HAVE_CONFIG_H", None)
            .define("NDEBUG", None)
            .define("_CRT_SECURE_NO_WARNINGS", None)
            .define("_CRT_NONSTDC_NO_DEPRECATE", None)
            .define("_USE_MATH_DEFINES", None)
            .define("NOMINMAX", None)
            // The workaround upstream uses for a C++ runtime older than the compiler, which an
            // installed Visual C++ redistributable can be.
            .define("_DISABLE_CONSTEXPR_MUTEX_CONSTRUCTOR", None)
            .define("_SILENCE_STDEXT_HASH_DEPRECATION_WARNINGS", "1")
            // Each line's picture kept as a Pix, as upstream allows, instead of encoded as PNG:
            // without libpng, Leptonica would fall back to PNM through a temporary file, which the
            // sandbox forbids and nobody should leave behind.
            .define("TESSERACT_IMAGEDATA_AS_PIX", None)
            .define("HAVE_AVX", None)
            .define("HAVE_AVX2", None)
            .define("HAVE_AVX512F", None)
            .define("HAVE_FMA", None)
            .define("HAVE_SSE4_1", None)
            .warnings(false)
            .flag("/utf-8")
            .flag("/EHsc")
            .flag("/bigobj");
        for module in modules {
            build.include(tesseract.join("src").join(module));
        }
        for flag in optimized {
            build.flag(flag);
        }
        build
    };
    let files = |names: &[String]| -> Vec<PathBuf> {
        names
            .iter()
            .map(|name| tesseract.join(name))
            .inspect(|path| assert!(path.is_file(), "{} is missing", path.display()))
            .collect()
    };
    let mut main_build = base(&[]);
    main_build.files(files(&core));
    main_build.file(shim.join("ocr.cpp"));
    main_build.include(&shim);
    main_build.compile("musaeum_tesseract");
    // Each kernel with its instruction set, as upstream's CMake has them for MSVC. MSVC has no
    // switch for FMA or SSE4.1 alone and allows their intrinsics anywhere, so those two only get
    // the macro the source checks for.
    for (name, list_name, flags) in [
        (
            "musaeum_tesseract_avx",
            "TESSERACT_SRC_ARCH_AVX",
            &["/arch:AVX"][..],
        ),
        (
            "musaeum_tesseract_avx2",
            "TESSERACT_SRC_ARCH_AVX2",
            &["/arch:AVX2"][..],
        ),
        (
            "musaeum_tesseract_avx512",
            "TESSERACT_SRC_ARCH_AVX512F",
            &["/arch:AVX512"][..],
        ),
        (
            "musaeum_tesseract_fma",
            "TESSERACT_SRC_ARCH_FMA",
            &["-D__FMA__"][..],
        ),
        (
            "musaeum_tesseract_sse41",
            "TESSERACT_SRC_ARCH_SSE41",
            &["-D__SSE4_1__"][..],
        ),
    ] {
        // dotproductavx.cpp is in both the AVX and the AVX2 list; it is built once, with AVX.
        let mut names = list(list_name);
        if list_name == "TESSERACT_SRC_ARCH_AVX2" {
            names.retain(|file| !file.ends_with("dotproductavx.cpp"));
        }
        let mut build = base(flags);
        build.files(files(&names));
        build.compile(name);
    }

    println!("cargo:rerun-if-changed={}", shim.display());
    println!("cargo:rerun-if-changed={}", tesseract.join("src").display());
    println!("cargo:rerun-if-changed={}", lepton_src.display());
    println!("cargo:rerun-if-changed=build.rs");
}

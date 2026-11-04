//! Builds the vendored subset of libarchive: the reading core, ZIP, RAR and RAR5, and nothing that
//! writes archives, touches the disk or starts programs. `vendor/libarchive/README.md` says where
//! the sources and `config.h` come from.

use std::env;
use std::path::PathBuf;

/// Every file compiled. Leaving a file out is how a feature stays out: there is no code here to
/// write files (`archive_write_disk_*.c`) or to run a program as a filter
/// (`archive_read_support_filter_program.c`, `filter_fork_windows.c`).
const SOURCES: &[&str] = &[
    "archive_acl.c",
    "archive_blake2s_ref.c",
    "archive_blake2sp_ref.c",
    "archive_check_magic.c",
    "archive_crc32.c",
    "archive_cryptor.c",
    "archive_entry.c",
    "archive_entry_sparse.c",
    "archive_entry_stat.c",
    "archive_entry_xattr.c",
    "archive_hmac.c",
    "archive_options.c",
    "archive_ppmd7.c",
    "archive_ppmd8.c",
    "archive_random.c",
    "archive_rb.c",
    "archive_read.c",
    "archive_read_add_passphrase.c",
    "archive_read_open_memory.c",
    "archive_read_set_options.c",
    "archive_read_support_format_rar.c",
    "archive_read_support_format_rar5.c",
    "archive_read_support_format_zip.c",
    "archive_string.c",
    "archive_string_sprintf.c",
    "archive_time.c",
    "archive_util.c",
    "archive_virtual.c",
    "archive_windows.c",
];

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    assert_eq!(
        target_os, "windows",
        "only a Windows config.h is vendored for libarchive so far"
    );

    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("set by Cargo"));
    let root = manifest
        .join("..")
        .join("..")
        .join("vendor")
        .join("libarchive");
    let sources = root.join("libarchive");
    let zlib = env::var_os("DEP_Z_INCLUDE").expect("libz-sys exports the path of zlib.h");

    let mut build = cc::Build::new();
    build
        .include(root.join("config").join("windows"))
        .include(&sources)
        .include(zlib)
        .define("HAVE_CONFIG_H", None)
        .define("LIBARCHIVE_STATIC", None)
        // CMake ran without zlib, which libz-sys provides instead; these are the two lines it would
        // have written.
        .define("HAVE_ZLIB_H", "1")
        .define("HAVE_LIBZ", "1")
        .warnings(false);
    for file in SOURCES {
        build.file(sources.join(file));
    }
    build.compile("musaeum_libarchive");

    // Random numbers and the decryption libarchive offers through Windows CNG.
    println!("cargo:rustc-link-lib=bcrypt");
    println!("cargo:rerun-if-changed={}", root.display());
}

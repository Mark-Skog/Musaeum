//! The DjVu worker as the app's reader starts it, read from outside: the policy the app gives
//! DjVu, and nothing the worker holds that reaches the user's files, the network or another
//! process. The denials themselves are tried one by one in `musaeum-render-pdf/tests/sandbox.rs`,
//! with the same token; this worker adds win32k off and only DLLs Microsoft signed.

use std::fs;
use std::path::{Path, PathBuf};

use musaeum_core::FixedFormat;
use musaeum_sandbox::Policy;
use musaeum_worker::harness::Worker;

fn exe() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_musaeum-render-djvu"))
}

#[test]
fn the_worker_runs_as_locked_down_as_the_app_says_and_holds_only_the_book() {
    assert_eq!(FixedFormat::Djvu.policy(), Policy::LockdownParser);
    assert_eq!(FixedFormat::Djvu.worker_exe(), "musaeum-render-djvu.exe");
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("djvu-sandbox");
    fs::create_dir_all(&dir).unwrap();
    let book = dir.join("book.djvu");
    gen_fixtures::djvu::write_djvu_fixture(&book).unwrap();

    let worker = Worker::started_as_in_the_app(exe(), &book, FixedFormat::Djvu);
    let violations = worker.sandbox_violations(FixedFormat::Djvu.policy(), &book);
    assert!(violations.is_empty(), "{violations:#?}");
}

//! The OCR worker as the app starts it, read from outside: the policy and the memory cap the app's
//! client gives it, and nothing it holds that reaches a file, the network or another process,
//! before and after Tesseract has its models. The models come as bytes through the pipe; the
//! worker never opens a file. The denials themselves are tried one by one in
//! `musaeum-render-pdf/tests/sandbox.rs`, with the same token.

use std::io::Write;
use std::path::{Path, PathBuf};

use musaeum_ocr::OcrConfig;
use musaeum_ocr::protocol::{
    ModelHeader, PROTOCOL_VERSION, Reply, Request, read_frame, write_frame,
};
use musaeum_sandbox::{Documents, Policy, SandboxedChild, SpawnConfig};

fn exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_musaeum-ocr"))
}

fn models() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/tesseract/tessdata")
}

fn violations(child: &SandboxedChild, policy: Policy) -> Vec<String> {
    let report = child.report().expect("the worker's sandbox can be read");
    let audit = child.audit().expect("the worker's handles can be read");
    println!("{report:#?}\n{audit:#?}");
    let mut found = report.violations(policy);
    found.extend(audit.violations(policy, Documents::Only(&[])));
    found
}

#[test]
fn the_worker_runs_as_locked_down_as_the_app_says_and_holds_no_file() {
    let config = OcrConfig::default();
    assert_eq!(config.policy, Policy::LockdownParser);
    let exe = exe();
    let mut child = musaeum_sandbox::spawn(SpawnConfig {
        exe: &exe,
        args: &[],
        policy: config.policy,
        memory_limit: Some(config.memory_limit),
    })
    .expect("the worker starts");
    let mut stdin = child.stdin.take().expect("a stdin pipe");
    let mut stdout = child.stdout.take().expect("a stdout pipe");

    let ready: Reply = read_frame(&mut stdout)
        .unwrap()
        .expect("the worker answers");
    assert_eq!(
        ready,
        Reply::Ready {
            protocol: PROTOCOL_VERSION
        }
    );
    let found = violations(&child, config.policy);
    assert!(found.is_empty(), "once ready: {found:#?}");

    let model = std::fs::read(models().join("eng.traineddata")).expect("tools/fetch-ocr.ps1 ran");
    write_frame(
        &mut stdin,
        &Request::Load {
            models: vec![ModelHeader {
                language: "eng".to_owned(),
                bytes: model.len() as u64,
            }],
            languages: "eng".to_owned(),
        },
    )
    .unwrap();
    stdin.write_all(&model).unwrap();
    stdin.flush().unwrap();
    let loaded: Reply = read_frame(&mut stdout)
        .unwrap()
        .expect("the worker answers");
    assert_eq!(loaded, Reply::Loaded);
    let found = violations(&child, config.policy);
    assert!(found.is_empty(), "with Tesseract loaded: {found:#?}");
}

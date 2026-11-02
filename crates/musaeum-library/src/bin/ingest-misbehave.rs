//! A worker gone wrong, for testing that the host survives one.
//!
//! The host has to assume that a book took over its worker, and this plays that worker. It answers
//! the first request the way its argument says, and it does not run in any sandbox: nothing here
//! needs one, because it reads nothing.

use std::io::{self, Write};
use std::process::ExitCode;
use std::time::Duration;

use musaeum_ingest::parse::Extracted;
use musaeum_ingest::protocol::{PROTOCOL_VERSION, Reply, Request, read_frame, write_frame};

fn main() -> ExitCode {
    // From its own file name, because the library starts its worker with no arguments. Tests copy this
    // executable to a name that says what to do.
    let behaviour = std::env::current_exe()
        .ok()
        .and_then(|exe| {
            exe.file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
        .unwrap_or_default();
    musaeum_sandbox::target::die_quietly();

    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();

    match behaviour.as_str() {
        "never-ready" => loop {
            std::thread::sleep(Duration::from_secs(60));
        },
        "wrong-protocol" => {
            let _ = write_frame(
                &mut output,
                &Reply::Ready {
                    protocol: PROTOCOL_VERSION + 1,
                },
            );
            return ExitCode::SUCCESS;
        }
        _ => {}
    }
    let _ = write_frame(
        &mut output,
        &Reply::Ready {
            protocol: PROTOCOL_VERSION,
        },
    );

    let Ok(Some(_request)) = read_frame::<Request>(&mut input) else {
        return ExitCode::FAILURE;
    };
    match behaviour.as_str() {
        "crash" => std::process::abort(),
        "exit" => return ExitCode::SUCCESS,
        "hang" => loop {
            std::thread::sleep(Duration::from_secs(60));
        },
        "garbage" => {
            let _ = output.write_all(&5u32.to_le_bytes());
            let _ = output.write_all(b"@@@@@");
            let _ = output.flush();
        }
        "huge-frame" => {
            let _ = output.write_all(&u32::MAX.to_le_bytes());
            let _ = output.flush();
        }
        "out-of-turn" => {
            let _ = write_frame(
                &mut output,
                &Reply::Ready {
                    protocol: PROTOCOL_VERSION,
                },
            );
        }
        "lie" => {
            let lie = Extracted {
                pdf_id0: Some("<script>alert(1)</script>".to_owned()),
                dc_identifier: Some("x".repeat(100_000)),
                isbn: Some("not an isbn".to_owned()),
                title: Some(format!("  A\u{0}B\u{7}C {}", "y".repeat(100_000))),
                creators: vec!["Someone".to_owned(); 100],
                language: Some("z".repeat(100_000)),
                year: Some(99_999),
            };
            let _ = write_frame(&mut output, &Reply::Extracted(lie));
        }
        _ => {}
    }
    // Stays until it is killed, which is what the host does with a worker it cannot trust.
    let _ = read_frame::<Request>(&mut input);
    ExitCode::SUCCESS
}

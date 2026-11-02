//! The library from a command line.
//!
//! Whatever only looks opens the library for reading, so it works while something else, such as
//! `musaeum watch`, is writing to it.

#![forbid(unsafe_code)]

mod args;
mod commands;

use std::process::ExitCode;

use args::{ArgsError, USAGE};

fn main() -> ExitCode {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    let invocation = match args::parse(arguments) {
        Ok(invocation) => invocation,
        Err(ArgsError::Help) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(ArgsError::Invalid(message)) => {
            eprintln!("musaeum: {message}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match commands::run(invocation) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("musaeum: {error}");
            ExitCode::FAILURE
        }
    }
}

//! Starts and locks down the processes that interpret untrusted bytes.
//!
//! A PDF parser is exactly the kind of code that gets exploited, so the worker that runs one
//! is treated as if it had already been compromised. The goal: an attacker in control of it
//! can draw wrong pixels into its own shared memory, and little else. It should not be able
//! to read the user's files, reach the network, start a program or touch another process.
//!
//! Several independent layers get there, because each one has gaps the others cover:
//!
//! | Layer | What it stops |
//! |-------|---------------|
//! | Restricted token | Opening anything by name: files, pipes, registry keys, sockets |
//! | Low integrity | Writing to anything the user owns, even if a token check passes |
//! | Process mitigations | Generating code, and the classic tricks for loading it |
//! | Job object | Child processes, memory exhaustion, clipboard and other USER objects |
//! | Separate window station | Reading the screen and sending input to the user's windows |
//!
//! # How a process starts
//!
//! A token this restricted cannot even load a DLL, so the sequence has two stages:
//!
//! 1. [`spawn`] creates the process suspended, with the final token as its primary token,
//!    and gives its main thread a more capable impersonation token before it runs.
//! 2. The process loads what it needs, and then calls [`target::enter_lockdown`] *before
//!    reading any untrusted input*. That drops the impersonation token for good, and only
//!    the final token is left.
//!
//! What remains possible afterwards is what the host handed over: handles it duplicated
//! into the process, and pipes it inherited at creation.

#[cfg(not(windows))]
compile_error!("musaeum-sandbox only implements Windows sandboxing in Phase 0.");

pub mod audit;
mod command_line;
mod desktop;
pub mod inspect;
mod job;
mod mitigation;
mod spawn;
pub mod target;
mod token;

pub use audit::{Documents, HeldHandle, ProcessAudit};
pub use inspect::{SandboxReport, TokenReport};
pub use job::JobReport;
pub use mitigation::MitigationReport;
pub use spawn::{Policy, SandboxedChild, SpawnConfig, spawn};

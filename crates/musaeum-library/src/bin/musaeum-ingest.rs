//! The isolated worker that reads metadata out of books.
//!
//! All of it is in `musaeum-ingest`. The executable is built from this package because Cargo only
//! gives a package's tests the executables of that same package, and the battery here needs the
//! real worker, in the real sandbox.

fn main() -> std::process::ExitCode {
    musaeum_ingest::worker::main()
}

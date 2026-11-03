//! Command-line options.
//!
//! ```text
//! musaeum-desktop [--pdf PATH | --epub PATH] [--delivery auto|a|b|c] [--workers N]
//!                 [--pull-batch N] [--gpu-layers N] [--tile-budget-mb N] [--library DIR]
//!                 [--bench OUT.json] [--main-stress-ms N]
//!                 [--scenario full|quick|smoke|zoom|memory|layout|security]
//!                 [--uncapped] [--zoom Z] [--width W] [--height H]
//!                 [--delivery-fault FAULT[,FAULT...]] [--no-sandbox]
//!                 [--epub-protection full|csp-only|sandbox-only|none]
//! ```

use std::path::PathBuf;
use std::thread;

use serde::{Deserialize, Serialize};

/// Which pixel path a session uses.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DeliveryPath {
    /// The compositor worker pulls tiles with `fetch` from a custom protocol.
    A,
    /// Pixels stay in a WebView2 shared buffer; the main thread copies them into a
    /// `SharedArrayBuffer` ring and wakes the compositor worker through `Atomics`.
    B,
    /// The same buffers as Path B, transferred to the compositor worker once, so it reads
    /// the pixels where PDFium wrote them. No copy on the CPU and no main thread at all.
    /// This relies on a transfer behaviour WebView2 does not document, so the frontend
    /// verifies it at startup with `probe.ts`.
    C,
}

/// What the command line asked for. Anything but `Auto` is for measuring one path, so it
/// fails loudly when that path is unavailable instead of quietly measuring another.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DeliveryRequest {
    /// Tests the runtime at startup and takes the best path that works.
    Auto,
    /// Only Path A.
    A,
    /// Only Path B.
    B,
    /// Only Path C.
    C,
}

/// A failure to simulate at startup, so the automatic selection can be tested end to end on
/// a runtime where everything works. Each one changes what the selection observes, never
/// the selection itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeliveryFault {
    /// The worker gets a private copy of a transferred buffer instead of live memory.
    CopiedTransfer,
    /// `postMessage` refuses to transfer the buffer.
    RejectedTransfer,
    /// The host cannot create shared buffers, as on a runtime without the interface.
    NoSharedBuffers,
    /// The page is not cross-origin isolated, so it has no `SharedArrayBuffer`.
    NotIsolated,
}

impl DeliveryFault {
    fn parse(text: &str) -> Result<Self, String> {
        match text {
            "copied-transfer" => Ok(Self::CopiedTransfer),
            "rejected-transfer" => Ok(Self::RejectedTransfer),
            "no-shared-buffers" => Ok(Self::NoSharedBuffers),
            "not-isolated" => Ok(Self::NotIsolated),
            other => Err(format!("unknown delivery fault: {other}")),
        }
    }
}

/// Which of a book's barriers are up. Anything but `Full` exists to show, in a benchmark, that
/// each barrier holds on its own and that the hostile book's scripts do run without them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BookProtection {
    /// The Content Security Policy, and sections sandboxed without scripts.
    Full,
    /// The policy alone, as on WebKit, where sections need `allow-scripts`.
    CspOnly,
    /// The section sandbox alone.
    SandboxOnly,
    /// Neither. Only the origin is left, so this shows what that one barrier stops.
    None,
}

/// How to run the benchmark instead of waiting for a user.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BenchOptions {
    /// Where to write the report.
    pub out: PathBuf,
    /// `full`, `quick`, `smoke` or `memory` for a PDF; `layout`, `security`, `smoke` or
    /// `memory` for an EPUB; `memory` with no document.
    pub scenario: String,
    /// Blocks the main thread this long, five times a second, to imitate heavy UI work.
    pub main_stress_ms: u32,
    /// Lets frames run faster than the display.
    pub uncapped: bool,
    /// Device pixels per PDF point.
    pub zoom: f32,
}

/// Everything decided at startup.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupOptions {
    /// The PDF to open right away, if any.
    pub pdf: Option<PathBuf>,
    /// The EPUB to open right away, if any.
    pub epub: Option<PathBuf>,
    /// Which of a book's barriers are up.
    pub book_protection: BookProtection,
    /// Delivery path to use, or `Auto` to choose one at startup.
    pub delivery: DeliveryRequest,
    /// Failures to simulate during the automatic selection.
    pub delivery_faults: Vec<DeliveryFault>,
    /// Render worker processes.
    pub workers: usize,
    /// Most tiles in one Path A response.
    pub pull_batch: usize,
    /// Size of the compositor's tile cache, instead of what suits the screen.
    pub gpu_layers: Option<u32>,
    /// The GPU tile cache's budget in mebibytes, instead of what suits the screen.
    pub tile_budget_mb: Option<u32>,
    /// The library to remember reading positions in. Without it, the usual one is used if it
    /// exists, and nothing is remembered if it does not: opening a file never creates a library.
    pub library: Option<PathBuf>,
    /// Runs the render workers outside the sandbox, to measure what the sandbox costs.
    pub no_sandbox: bool,
    /// Window width in logical pixels.
    pub width: u32,
    /// Window height in logical pixels.
    pub height: u32,
    /// Present when running as a benchmark.
    pub bench: Option<BenchOptions>,
}

/// Half the logical cores, so rendering does not compete with the compositor, the WebView's own
/// processes and the rest of the desktop, and no more than four. Six were no smoother in Phase 0's
/// measurements, and each worker holds memory while reading.
fn default_workers() -> usize {
    let cores = thread::available_parallelism().map_or(4, std::num::NonZero::get);
    (cores / 2).clamp(2, 4)
}

impl StartupOptions {
    /// Parses the process arguments, excluding the program name.
    pub fn parse(args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut options = Self {
            pdf: None,
            epub: None,
            book_protection: BookProtection::Full,
            delivery: DeliveryRequest::Auto,
            delivery_faults: Vec::new(),
            workers: default_workers(),
            pull_batch: 4,
            gpu_layers: None,
            tile_budget_mb: None,
            library: None,
            no_sandbox: false,
            width: 1600,
            height: 900,
            bench: None,
        };
        let mut out = None;
        let mut scenario = "full".to_owned();
        let mut main_stress_ms = 0;
        let mut uncapped = false;
        let mut zoom = 2.0f32;

        let mut args = args;
        while let Some(flag) = args.next() {
            let mut value = || args.next().ok_or_else(|| format!("{flag} needs a value"));
            match flag.as_str() {
                "--pdf" => options.pdf = Some(PathBuf::from(value()?)),
                "--epub" => options.epub = Some(PathBuf::from(value()?)),
                "--epub-protection" => {
                    options.book_protection = match value()?.as_str() {
                        "full" => BookProtection::Full,
                        "csp-only" => BookProtection::CspOnly,
                        "sandbox-only" => BookProtection::SandboxOnly,
                        "none" => BookProtection::None,
                        other => return Err(format!("unknown protection: {other}")),
                    }
                }
                "--delivery" => {
                    options.delivery = match value()?.as_str() {
                        "auto" => DeliveryRequest::Auto,
                        "a" => DeliveryRequest::A,
                        "b" => DeliveryRequest::B,
                        "c" => DeliveryRequest::C,
                        other => {
                            return Err(format!("--delivery must be auto, a, b or c, got {other}"));
                        }
                    }
                }
                "--delivery-fault" => {
                    options.delivery_faults = value()?
                        .split(',')
                        .map(DeliveryFault::parse)
                        .collect::<Result<_, _>>()?;
                }
                "--workers" => options.workers = parse_number(&flag, &value()?)?,
                "--pull-batch" => options.pull_batch = parse_number(&flag, &value()?)?,
                "--gpu-layers" => options.gpu_layers = Some(parse_number(&flag, &value()?)?),
                "--tile-budget-mb" => {
                    options.tile_budget_mb = Some(parse_number(&flag, &value()?)?);
                }
                "--library" => options.library = Some(PathBuf::from(value()?)),
                "--no-sandbox" => options.no_sandbox = true,
                "--width" => options.width = parse_number(&flag, &value()?)?,
                "--height" => options.height = parse_number(&flag, &value()?)?,
                "--bench" => out = Some(PathBuf::from(value()?)),
                "--scenario" => scenario = value()?,
                "--main-stress-ms" => main_stress_ms = parse_number(&flag, &value()?)?,
                "--uncapped" => uncapped = true,
                "--zoom" => zoom = parse_number(&flag, &value()?)?,
                other => return Err(format!("unknown argument: {other}")),
            }
        }

        if options.pdf.is_some() && options.epub.is_some() {
            return Err("open a PDF or an EPUB, not both".to_owned());
        }
        if !matches!(
            scenario.as_str(),
            "full" | "quick" | "smoke" | "zoom" | "memory" | "layout" | "security"
        ) {
            return Err(format!("unknown scenario: {scenario}"));
        }
        if out.is_none() && options.book_protection != BookProtection::Full {
            return Err(
                "--epub-protection lowers a book's defences, so it needs --bench".to_owned(),
            );
        }
        if out.is_none() && options.no_sandbox {
            return Err(
                "--no-sandbox exposes the system to a hostile PDF, so it needs --bench".to_owned(),
            );
        }
        if let Some(out) = out {
            if options.pdf.is_none() && options.epub.is_none() && scenario != "memory" {
                return Err("--bench needs a document, except with --scenario memory".to_owned());
            }
            options.bench = Some(BenchOptions {
                out,
                scenario,
                main_stress_ms,
                uncapped,
                zoom,
            });
        }
        Ok(options)
    }

    /// The window's inner size in logical pixels.
    pub fn window_size(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

fn parse_number<T: std::str::FromStr>(flag: &str, text: &str) -> Result<T, String> {
    text.parse()
        .map_err(|_| format!("{flag} needs a number, got {text}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<StartupOptions, String> {
        StartupOptions::parse(args.iter().map(ToString::to_string))
    }

    #[test]
    fn no_arguments_means_interactive_with_automatic_delivery() {
        let options = parse(&[]).unwrap();
        assert_eq!(options.delivery, DeliveryRequest::Auto);
        assert!(options.delivery_faults.is_empty());
        assert!(options.pdf.is_none());
        assert!(options.bench.is_none());
        assert!((2..=4).contains(&options.workers));
    }

    #[test]
    fn reads_every_benchmark_option() {
        let options = parse(&[
            "--pdf",
            "a.pdf",
            "--delivery",
            "b",
            "--workers",
            "3",
            "--bench",
            "out.json",
            "--scenario",
            "quick",
            "--main-stress-ms",
            "50",
            "--uncapped",
            "--zoom",
            "1.5",
        ])
        .unwrap();
        assert_eq!(options.delivery, DeliveryRequest::B);
        assert_eq!(options.workers, 3);
        let bench = options.bench.unwrap();
        assert_eq!(bench.out, PathBuf::from("out.json"));
        assert_eq!(bench.scenario, "quick");
        assert_eq!(bench.main_stress_ms, 50);
        assert!(bench.uncapped);
        assert_eq!(bench.zoom, 1.5);
    }

    #[test]
    fn reads_a_list_of_delivery_faults() {
        let options = parse(&["--delivery-fault", "copied-transfer,not-isolated"]).unwrap();
        assert_eq!(
            options.delivery_faults,
            [DeliveryFault::CopiedTransfer, DeliveryFault::NotIsolated]
        );
        assert!(parse(&["--delivery-fault", "lightning"]).is_err());
    }

    #[test]
    fn a_book_opens_with_every_barrier_up() {
        let options = parse(&["--epub", "a.epub"]).unwrap();
        assert_eq!(options.epub, Some(PathBuf::from("a.epub")));
        assert_eq!(options.book_protection, BookProtection::Full);
    }

    #[test]
    fn only_a_benchmark_can_lower_a_books_barriers() {
        assert!(parse(&["--epub", "a.epub", "--epub-protection", "none"]).is_err());
        let options = parse(&[
            "--epub",
            "a.epub",
            "--epub-protection",
            "csp-only",
            "--bench",
            "out.json",
            "--scenario",
            "security",
        ])
        .unwrap();
        assert_eq!(options.book_protection, BookProtection::CspOnly);
    }

    #[test]
    fn only_a_benchmark_can_run_outside_the_sandbox() {
        assert!(parse(&["--pdf", "a.pdf", "--no-sandbox"]).is_err());
        let options = parse(&["--pdf", "a.pdf", "--no-sandbox", "--bench", "out.json"]).unwrap();
        assert!(options.no_sandbox);
    }

    #[test]
    fn a_memory_benchmark_can_run_with_nothing_open() {
        assert!(parse(&["--bench", "out.json", "--scenario", "memory"]).is_ok());
        assert!(parse(&["--pdf", "a.pdf", "--epub", "a.epub"]).is_err());
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse(&["--delivery", "d"]).is_err());
        assert!(parse(&["--workers", "many"]).is_err());
        assert!(parse(&["--pdf"]).is_err());
        assert!(parse(&["--nonsense"]).is_err());
        assert!(parse(&["--scenario", "long"]).is_err());
        assert!(
            parse(&["--bench", "out.json"]).is_err(),
            "a benchmark needs a PDF"
        );
    }
}

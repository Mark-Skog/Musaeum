//! Musaeum's desktop shell.
//!
//! At this stage it hosts the experiments of Phase 0: a tile pipeline for fixed layout, a
//! reader for fluid layout at an origin of its own, and a harness that measures them. The
//! rendering and scheduling logic lives in `musaeum-core`; this crate only connects it to
//! Tauri and WebView2.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod book;
mod memory;
mod options;
mod protocol;
mod session;
mod webview2;

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

use options::StartupOptions;
use session::AppState;

/// Tauri's defaults, which `additional_browser_args` would otherwise replace.
const BASE_FEATURES: &str = "msWebOOUI,msPdfOOUI,msSmartScreenProtection";

fn browser_args(options: &StartupOptions) -> String {
    match &options.bench {
        None => format!("--disable-features={BASE_FEATURES}"),
        Some(bench) => {
            // A benchmark window may lose focus or be covered. Without these flags,
            // Chromium would throttle it and the numbers would measure that instead.
            let mut args = format!(
                "--disable-features={BASE_FEATURES},CalculateNativeWinOcclusion \
                 --disable-background-timer-throttling \
                 --disable-renderer-backgrounding \
                 --disable-backgrounding-occluded-windows"
            );
            if bench.uncapped {
                // Frame pacing no longer follows the display, which exposes the pipeline's
                // real capacity on a monitor slower than the target refresh rate.
                args.push_str(" --disable-gpu-vsync --disable-frame-rate-limit");
            }
            args
        }
    }
}

fn main() {
    let options = match StartupOptions::parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };
    let window_args = browser_args(&options);
    let (width, height) = options.window_size();
    let is_bench = options.bench.is_some();

    let result = tauri::Builder::default()
        .manage(AppState::new(options))
        .manage(book::BookState::default())
        .register_asynchronous_uri_scheme_protocol(
            protocol::SCHEME,
            |context, request, responder| {
                protocol::handle(context.app_handle(), request, responder);
            },
        )
        .register_asynchronous_uri_scheme_protocol(book::SCHEME, |context, request, responder| {
            book::handle(context.app_handle(), request, responder);
        })
        .invoke_handler(tauri::generate_handler![
            book::open_book,
            session::startup_options,
            session::uptime_ms,
            session::open_delivery_probe,
            session::open_session,
            session::close_session,
            session::pool_stats,
            session::probe_exchange,
            session::bench_mark_phase,
            session::bench_resize_window,
            session::bench_finish,
            session::bench_fail,
        ])
        .setup(move |app| {
            WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("Musaeum")
                .inner_size(f64::from(width), f64::from(height))
                .additional_browser_args(&window_args)
                .always_on_top(is_bench)
                .focused(true)
                .build()?;
            Ok(())
        })
        .build(tauri::generate_context!());

    match result {
        Ok(app) => app.run(|handle, event| {
            if let tauri::RunEvent::ExitRequested { .. } = event {
                // Stop the workers while the WebView2 buffers they map are still alive.
                handle.state::<AppState>().stop_current_session();
            }
        }),
        Err(error) => {
            eprintln!("could not start Musaeum: {error}");
            std::process::exit(1);
        }
    }
}

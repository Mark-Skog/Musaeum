//! Musaeum's desktop shell.
//!
//! At this stage it hosts the experiments of Phase 0: a tile pipeline for fixed layout, a
//! reader for fluid layout at an origin of its own, and a harness that measures them. The
//! rendering and scheduling logic lives in `musaeum-core`; this crate only connects it to
//! Tauri and WebView2.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod annotations;
mod background;
mod book;
mod calibre;
mod covers;
mod exchange;
mod flow;
mod fulltext;
mod lines;
mod links;
mod memory;
mod ocr;
mod options;
mod pdf_annotations;
mod picker;
mod preferences;
mod protocol;
mod reading;
mod review;
mod session;
mod shelf;
mod taxonomy;
#[cfg(windows)]
mod titlebar;
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
    let is_app_mode = !is_bench && options.document.is_none() && options.epub.is_none();

    let result = tauri::Builder::default()
        .manage(AppState::new(options))
        .manage(links::OpenedLinks::default())
        .manage(calibre::ChosenFolders::default())
        .manage(annotations::CharCache::default())
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
        .register_asynchronous_uri_scheme_protocol(covers::SCHEME, |context, request, responder| {
            covers::handle(context.app_handle(), request, responder);
        })
        .invoke_handler(tauri::generate_handler![
            book::open_book,
            flow::open_flow,
            flow::save_flow_position,
            flow::locate_flow,
            flow::close_book,
            links::open_in_browser,
            annotations::fixed_annotations,
            annotations::page_chars,
            annotations::create_fixed_annotation,
            annotations::create_region_annotation,
            lines::line_model,
            ocr::region_text,
            annotations::flow_annotations,
            annotations::create_flow_annotation,
            annotations::edit_annotation,
            annotations::delete_annotation,
            annotations::restore_annotation,
            annotations::search_annotations,
            pdf_annotations::pdf_annotations_export,
            pdf_annotations::pdf_annotations_import,
            pdf_annotations::xfdf_export,
            pdf_annotations::xfdf_import,
            pdf_annotations::e2e_pdf_annotations,
            exchange::annotations_export_jsonld,
            exchange::annotations_import_jsonld,
            exchange::study_report,
            exchange::e2e_pdf_text,
            links::e2e_opened_links,
            shelf::library_info,
            shelf::library_choose_root,
            shelf::library_add_root,
            shelf::library_remove_root,
            shelf::library_choose_existing,
            shelf::library_browse,
            taxonomy::taxonomy,
            taxonomy::category_create,
            taxonomy::category_rename,
            taxonomy::category_move,
            taxonomy::category_delete,
            taxonomy::category_set_works,
            taxonomy::tag_create,
            taxonomy::tag_update,
            taxonomy::tag_delete,
            taxonomy::tag_set_works,
            taxonomy::collection_create,
            taxonomy::collection_update,
            taxonomy::collection_move,
            taxonomy::collection_delete,
            taxonomy::collection_add,
            taxonomy::collection_remove,
            taxonomy::smart_create,
            taxonomy::smart_update,
            taxonomy::smart_delete,
            taxonomy::smart_preview,
            taxonomy::field_create,
            taxonomy::field_update,
            taxonomy::field_delete,
            taxonomy::work_metadata,
            taxonomy::work_edit,
            taxonomy::works_edit,
            taxonomy::works_summary,
            shelf::library_select_all,
            review::library_review,
            review::suggestion_accept,
            review::suggestion_dismiss,
            review::duplicate_dismiss,
            review::works_merge,
            fulltext::fulltext_status,
            fulltext::fulltext_search,
            fulltext::fulltext_flow_place,
            fulltext::fulltext_page_point,
            fulltext::fulltext_wake,
            calibre::calibre_choose,
            calibre::calibre_preview,
            calibre::calibre_import,
            shelf::library_folders,
            shelf::reader_setting,
            shelf::set_reader_setting,
            session::startup_options,
            session::uptime_ms,
            session::set_window_title,
            session::open_delivery_probe,
            session::open_session,
            session::close_session,
            session::document_outline,
            session::save_reading_state,
            session::pool_stats,
            session::probe_exchange,
            session::bench_mark_phase,
            session::bench_resize_window,
            session::e2e_resize_window,
            preferences::preferences_get,
            preferences::preferences_set,
            preferences::ocr_models,
            preferences::cache_report,
            preferences::cache_clear_lines,
            session::bench_finish,
            session::bench_fail,
        ])
        .setup(move |app| {
            app.state::<AppState>().shelf.set_app(app.handle().clone());
            let theme = app.state::<AppState>().preferences.current().theme;
            let window =
                WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                    .title("Musaeum")
                    .theme(preferences::window_theme(theme))
                    .inner_size(f64::from(width), f64::from(height))
                    .additional_browser_args(&window_args)
                    .always_on_top(is_bench)
                    .focused(true)
                    .build()?;
            #[cfg(windows)]
            if is_app_mode && let Err(error) = titlebar::install(&window) {
                eprintln!("custom titlebar setup failed: {error}");
            }
            Ok(())
        })
        .build(tauri::generate_context!());

    match result {
        Ok(app) => app.run(|handle, event| {
            if let tauri::RunEvent::ExitRequested { .. } = event {
                // Stop the workers while the WebView2 buffers they map are still alive.
                handle.state::<AppState>().stop_current_session();
                // And the OCR's, with the quotes it was reading.
                handle.state::<AppState>().ocr.shutdown();
                // And the indexer and the covers, finishing what they are writing.
                handle.state::<AppState>().shelf.close();
            }
        }),
        Err(error) => {
            eprintln!("could not start Musaeum: {error}");
            std::process::exit(1);
        }
    }
}

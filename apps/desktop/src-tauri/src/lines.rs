//! The lines of text the background found on a page's picture, for the compositor to snap a
//! selection on a scan to.

use musaeum_core::BackgroundWork;
use musaeum_vision::LINES_VERSION;
use tauri::State;
use tauri::ipc::Response;

use crate::session::AppState;

/// A page's line model: its picture's 64-bit hash, little-endian, then the `MLIN` bytes of
/// `musaeum_vision::model`. Empty while the page is not traced yet, in which case it is put
/// ahead of the rest of the background, and the compositor asks again a moment later. Empty too
/// for a file the library does not have, which has nowhere to keep one.
#[tauri::command(async)]
pub fn line_model(state: State<'_, AppState>, session: u32, page: u32) -> Result<Response, String> {
    let session = state
        .session(session)
        .ok_or("that document is no longer open")?;
    if page as usize >= session.pool.pages().len() {
        return Err(format!("there is no page {page}"));
    }
    let (Some(library), Some(blake3)) = (state.library(), session.blake3) else {
        return Ok(Response::new(Vec::new()));
    };
    match library
        .line_model(&blake3, page, LINES_VERSION)
        .map_err(|error| error.to_string())?
    {
        Some(record) => {
            let mut bytes = Vec::with_capacity(8 + record.model.len());
            bytes.extend_from_slice(&record.page_hash.to_le_bytes());
            bytes.extend_from_slice(&record.model);
            Ok(Response::new(bytes))
        }
        None => {
            session.pool.add_background_first(BackgroundWork {
                lines: vec![page],
                ..BackgroundWork::default()
            });
            Ok(Response::new(Vec::new()))
        }
    }
}

//! What waits for a person's word: two documents the library noticed may be one (a file that came
//! back changed, one that took another's place with other identifiers inside), and two works that
//! may be one book. Nothing here acts by itself: the person accepts or turns down each one.

use std::sync::Arc;

use musaeum_library::{Library, Review, SuggestionId, WorkId};
use tauri::State;

use crate::session::AppState;

fn library_of(state: &AppState) -> Result<Arc<Library>, String> {
    state
        .shelf
        .library()
        .ok_or_else(|| "there is no library open".to_owned())
}

fn text<T>(result: musaeum_library::Result<T>) -> Result<T, String> {
    result.map_err(|error| error.to_string())
}

#[tauri::command(async)]
pub fn library_review(state: State<'_, AppState>) -> Result<Review, String> {
    text(library_of(&state)?.review())
}

/// The two documents are one: the newer one's revisions join the older one, with everything on
/// them.
#[tauri::command(async)]
pub fn suggestion_accept(state: State<'_, AppState>, id: SuggestionId) -> Result<i64, String> {
    text(library_of(&state)?.accept_suggestion(id))
}

/// They are two: it is not suggested again.
#[tauri::command(async)]
pub fn suggestion_dismiss(state: State<'_, AppState>, id: SuggestionId) -> Result<(), String> {
    text(library_of(&state)?.dismiss_suggestion(id))
}

/// The two works are different books: they are not suggested as one again.
#[tauri::command(async)]
pub fn duplicate_dismiss(state: State<'_, AppState>, a: WorkId, b: WorkId) -> Result<(), String> {
    text(library_of(&state)?.dismiss_duplicate(a, b))
}

/// The others become `keep`, with their documents and all that is on them.
#[tauri::command(async)]
pub fn works_merge(
    state: State<'_, AppState>,
    keep: WorkId,
    others: Vec<WorkId>,
) -> Result<WorkId, String> {
    text(library_of(&state)?.merge_works(keep, &others))
}

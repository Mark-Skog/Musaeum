//! What the library page asks of the library's taxonomy and of a work's metadata: the categories,
//! tags, collections, smart collections and custom fields it shows beside the books, the changes a
//! person makes to them, and a work as the editor shows and changes it. Every change goes through
//! `musaeum_library`, which checks it before writing.

use std::sync::Arc;

use musaeum_library::{
    Category, CategoryId, Collection, CollectionId, CustomField, CustomFieldId, Library, Place,
    SmartCollection, SmartCollectionId, SmartRules, Tag, TagId, WorkEdit, WorkId, WorkMetadata,
};
use serde::Serialize;
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

/// Everything the side of the library lists besides its folders.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Taxonomy {
    categories: Vec<Category>,
    tags: Vec<Tag>,
    collections: Vec<Collection>,
    smart: Vec<SmartCollection>,
    fields: Vec<CustomField>,
}

#[tauri::command(async)]
pub fn taxonomy(state: State<'_, AppState>) -> Result<Taxonomy, String> {
    let library = library_of(&state)?;
    Ok(Taxonomy {
        categories: text(library.categories())?,
        tags: text(library.tags())?,
        collections: text(library.collections())?,
        smart: text(library.smart_collections())?,
        fields: text(library.custom_fields())?,
    })
}

#[tauri::command(async)]
pub fn category_create(
    state: State<'_, AppState>,
    parent: Option<CategoryId>,
    name: String,
    place: Place,
) -> Result<CategoryId, String> {
    text(library_of(&state)?.create_category(parent, &name, place))
}

#[tauri::command(async)]
pub fn category_rename(
    state: State<'_, AppState>,
    id: CategoryId,
    name: String,
) -> Result<(), String> {
    text(library_of(&state)?.rename_category(id, &name))
}

#[tauri::command(async)]
pub fn category_move(
    state: State<'_, AppState>,
    id: CategoryId,
    parent: Option<CategoryId>,
    place: Place,
) -> Result<(), String> {
    text(library_of(&state)?.move_category(id, parent, place))
}

#[tauri::command(async)]
pub fn category_delete(state: State<'_, AppState>, id: CategoryId) -> Result<(), String> {
    text(library_of(&state)?.delete_category(id))
}

#[tauri::command(async)]
pub fn category_set_works(
    state: State<'_, AppState>,
    works: Vec<WorkId>,
    category: CategoryId,
    member: bool,
) -> Result<(), String> {
    text(library_of(&state)?.set_category(&works, category, member))
}

#[tauri::command(async)]
pub fn tag_create(
    state: State<'_, AppState>,
    name: String,
    color: Option<String>,
) -> Result<TagId, String> {
    text(library_of(&state)?.create_tag(&name, color.as_deref()))
}

/// Renames a tag, or changes its color; `clearColor` takes it away.
#[tauri::command(async)]
pub fn tag_update(
    state: State<'_, AppState>,
    id: TagId,
    name: Option<String>,
    color: Option<String>,
    clear_color: bool,
) -> Result<(), String> {
    let color = if clear_color {
        Some(None)
    } else {
        color.as_deref().map(Some)
    };
    text(library_of(&state)?.update_tag(id, name.as_deref(), color))
}

#[tauri::command(async)]
pub fn tag_delete(state: State<'_, AppState>, id: TagId) -> Result<(), String> {
    text(library_of(&state)?.delete_tag(id))
}

#[tauri::command(async)]
pub fn tag_set_works(
    state: State<'_, AppState>,
    works: Vec<WorkId>,
    tag: TagId,
    member: bool,
) -> Result<(), String> {
    text(library_of(&state)?.set_tag(&works, tag, member))
}

#[tauri::command(async)]
pub fn collection_create(
    state: State<'_, AppState>,
    name: String,
    description: String,
    place: Place,
) -> Result<CollectionId, String> {
    text(library_of(&state)?.create_collection(&name, &description, place))
}

#[tauri::command(async)]
pub fn collection_update(
    state: State<'_, AppState>,
    id: CollectionId,
    name: Option<String>,
    description: Option<String>,
) -> Result<(), String> {
    text(library_of(&state)?.update_collection(id, name.as_deref(), description.as_deref()))
}

#[tauri::command(async)]
pub fn collection_move(
    state: State<'_, AppState>,
    id: CollectionId,
    place: Place,
) -> Result<(), String> {
    text(library_of(&state)?.move_collection(id, place))
}

#[tauri::command(async)]
pub fn collection_delete(state: State<'_, AppState>, id: CollectionId) -> Result<(), String> {
    text(library_of(&state)?.delete_collection(id))
}

#[tauri::command(async)]
pub fn collection_add(
    state: State<'_, AppState>,
    collection: CollectionId,
    works: Vec<WorkId>,
    place: Place,
) -> Result<(), String> {
    text(library_of(&state)?.add_to_collection(collection, &works, place))
}

#[tauri::command(async)]
pub fn collection_remove(
    state: State<'_, AppState>,
    collection: CollectionId,
    works: Vec<WorkId>,
) -> Result<(), String> {
    text(library_of(&state)?.remove_from_collection(collection, &works))
}

#[tauri::command(async)]
pub fn smart_create(
    state: State<'_, AppState>,
    name: String,
    rules: SmartRules,
    place: Place,
) -> Result<SmartCollectionId, String> {
    text(library_of(&state)?.create_smart_collection(&name, &rules, place))
}

#[tauri::command(async)]
pub fn smart_update(
    state: State<'_, AppState>,
    id: SmartCollectionId,
    name: Option<String>,
    rules: Option<SmartRules>,
) -> Result<(), String> {
    text(library_of(&state)?.update_smart_collection(id, name.as_deref(), rules.as_ref()))
}

#[tauri::command(async)]
pub fn smart_delete(state: State<'_, AppState>, id: SmartCollectionId) -> Result<(), String> {
    text(library_of(&state)?.delete_smart_collection(id))
}

/// How many works a rule finds now, before it is kept.
#[tauri::command(async)]
pub fn smart_preview(state: State<'_, AppState>, rules: SmartRules) -> Result<usize, String> {
    text(library_of(&state)?.smart_preview(&rules)).map(|works| works.len())
}

#[tauri::command(async)]
pub fn field_create(
    state: State<'_, AppState>,
    name: String,
    kind: String,
    choices: Vec<String>,
) -> Result<CustomFieldId, String> {
    text(library_of(&state)?.create_custom_field(&name, &kind, &choices))
}

#[tauri::command(async)]
pub fn field_update(
    state: State<'_, AppState>,
    id: CustomFieldId,
    name: Option<String>,
    choices: Option<Vec<String>>,
) -> Result<(), String> {
    text(library_of(&state)?.update_custom_field(id, name.as_deref(), choices.as_deref()))
}

#[tauri::command(async)]
pub fn field_delete(state: State<'_, AppState>, id: CustomFieldId) -> Result<(), String> {
    text(library_of(&state)?.delete_custom_field(id))
}

#[tauri::command(async)]
pub fn work_metadata(state: State<'_, AppState>, work: WorkId) -> Result<WorkMetadata, String> {
    text(library_of(&state)?.work_metadata(work))
}

/// Changes a work, and answers with it as it is now.
#[tauri::command(async)]
pub fn work_edit(
    state: State<'_, AppState>,
    work: WorkId,
    edit: WorkEdit,
) -> Result<WorkMetadata, String> {
    let library = library_of(&state)?;
    text(library.edit_work(work, edit))?;
    text(library.work_metadata(work))
}

//! What reading takes from the library: where the reader was, and the pages already measured.
//!
//! The library is optional. Without one, a document still opens and reads, and only what would be
//! remembered is not. Opening a file never creates a library by itself.

use std::path::Path;

use musaeum_core::{Direction, DocumentHints, Orientation, PageLayout};
use musaeum_library::{
    DocumentGeometry, Library, OpenedFile, PageOrientation, PageRecord, ReadingMode, ReadingState,
    Spreads,
};
use musaeum_shm::PageGeometry;
use serde::{Deserialize, Serialize};

/// The saved position, as the page sends and receives it.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedReading {
    page: u32,
    u: f64,
    v: f64,
    progress: f64,
    mode: ReadingMode,
    page_turn_animation: bool,
    zoom: f64,
    spreads: Spreads,
}

impl SavedReading {
    fn from_state(state: ReadingState) -> Self {
        Self {
            page: state.page,
            u: state.u,
            v: state.v,
            progress: state.progress,
            mode: state.mode,
            page_turn_animation: state.page_turn_animation,
            zoom: state.zoom,
            spreads: state.spreads,
        }
    }

    /// The state to keep, taken on `revision`.
    pub fn into_state(self, revision: i64) -> ReadingState {
        ReadingState {
            revision_id: Some(revision),
            page: self.page,
            u: self.u,
            v: self.v,
            progress: self.progress,
            mode: self.mode,
            page_turn_animation: self.page_turn_animation,
            zoom: self.zoom,
            spreads: self.spreads,
            updated_at: 0,
        }
    }
}

/// What the library knows about a file being opened.
#[derive(Debug, Default)]
pub struct Known {
    pub file: Option<OpenedFile>,
    pub geometry: Option<DocumentGeometry>,
    pub reading: Option<SavedReading>,
}

/// Looks a file up. Anything that fails just means less is known.
pub fn look_up(library: &Library, document: &Path) -> Known {
    let Ok(Some(file)) = library.opened_file(document) else {
        return Known::default();
    };
    let geometry = file
        .blake3
        .and_then(|hash| library.page_geometry(&hash).ok().flatten());
    let reading = library
        .reading_state(file.document)
        .ok()
        .flatten()
        // A position on a page this revision does not have is from some other version of it.
        .filter(|state| {
            geometry
                .as_ref()
                .is_none_or(|known| (state.page as usize) < known.pages.len())
        })
        .map(SavedReading::from_state);
    Known {
        file: Some(file),
        geometry,
        reading,
    }
}

/// The pages and hints of a measured document, as the render pool takes them.
pub fn known_geometry(geometry: &DocumentGeometry) -> (Vec<PageGeometry>, DocumentHints) {
    let pages = geometry
        .pages
        .iter()
        .map(|page| PageGeometry {
            width_pt: page.width_pt,
            height_pt: page.height_pt,
        })
        .collect();
    let hints = DocumentHints {
        page_layout: geometry
            .page_layout
            .as_deref()
            .and_then(|name| PageLayout::from_name(name.as_bytes())),
        direction: geometry
            .direction
            .as_deref()
            .and_then(|name| Direction::from_name(name.as_bytes())),
    };
    (pages, hints)
}

/// What a worker measured, as the cache keeps it.
pub fn measured(pages: &[PageGeometry], hints: DocumentHints) -> DocumentGeometry {
    DocumentGeometry {
        pages: pages
            .iter()
            .map(|&page| PageRecord {
                width_pt: page.width_pt,
                height_pt: page.height_pt,
                orientation: match Orientation::of(page) {
                    Orientation::Portrait => PageOrientation::Portrait,
                    Orientation::Landscape => PageOrientation::Landscape,
                    Orientation::Square => PageOrientation::Square,
                },
            })
            .collect(),
        page_layout: hints.page_layout.map(|layout| layout.as_str().to_owned()),
        direction: hints
            .direction
            .map(|direction| direction.as_str().to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `apps/desktop/src/compositor/reader.ts` sends, field for field.
    const FROM_THE_PAGE: &str = r#"{"page":41,"u":0.25,"v":0.75,"progress":0.5,"mode":"continuous","pageTurnAnimation":false,"zoom":1.37,"spreads":"double"}"#;

    #[test]
    fn the_page_and_the_library_agree_on_a_saved_position() {
        let saved: SavedReading = serde_json::from_str(FROM_THE_PAGE).unwrap();
        let state = saved.clone().into_state(7);
        assert_eq!(state.revision_id, Some(7));
        assert_eq!((state.page, state.u, state.v), (41, 0.25, 0.75));
        assert_eq!(state.mode, ReadingMode::Continuous);
        assert!(!state.page_turn_animation);
        assert_eq!(state.spreads, Spreads::Double);

        // And back out to the page in the same shape it came in.
        let back = serde_json::to_value(SavedReading::from_state(state)).unwrap();
        let sent: serde_json::Value = serde_json::from_str(FROM_THE_PAGE).unwrap();
        assert_eq!(back, sent);
    }

    #[test]
    fn a_position_the_page_could_not_have_sent_is_refused() {
        for broken in [
            r#"{"page":-1,"u":0,"v":0,"progress":0,"mode":"continuous","pageTurnAnimation":true,"zoom":1,"spreads":"auto"}"#,
            r#"{"page":1,"u":0,"v":0,"progress":0,"mode":"scroll","pageTurnAnimation":true,"zoom":1,"spreads":"auto"}"#,
            r#"{"page":1,"u":0,"v":0,"progress":0,"mode":"paginated","pageTurnAnimation":true,"zoom":1,"spreads":"triple"}"#,
        ] {
            assert!(
                serde_json::from_str::<SavedReading>(broken).is_err(),
                "{broken}"
            );
        }
    }

    #[test]
    fn measured_pages_come_back_from_the_cache_as_they_went_in() {
        let pages = [
            PageGeometry {
                width_pt: 595.0,
                height_pt: 842.0,
            },
            PageGeometry {
                width_pt: 1190.0,
                height_pt: 842.0,
            },
            PageGeometry {
                width_pt: 600.0,
                height_pt: 600.0,
            },
        ];
        let hints = DocumentHints {
            page_layout: Some(PageLayout::TwoPageRight),
            direction: Some(Direction::R2L),
        };
        let kept = measured(&pages, hints);
        assert_eq!(
            kept.pages
                .iter()
                .map(|page| page.orientation)
                .collect::<Vec<_>>(),
            [
                PageOrientation::Portrait,
                PageOrientation::Landscape,
                PageOrientation::Square
            ]
        );
        assert_eq!(kept.summary(), "mixed");
        assert_eq!(kept.page_layout.as_deref(), Some("TwoPageRight"));
        assert_eq!(known_geometry(&kept), (pages.to_vec(), hints));
    }
}

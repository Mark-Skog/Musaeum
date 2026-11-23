//! Anchoring a mark on the picture of a page: lines the detector found on a scan, a box, or a
//! stroke drawn by hand.
//!
//! There is no text to find again, so the segment keeps where it was, in the unrotated crop box,
//! and what the page looked like: the 64-bit difference hash of the whole page's picture, and of
//! the region. On a later revision the mark stays on the same page at the same place; it is exact
//! when the page's picture there is still the same one (the page hashes within
//! [`MAX_SAME_PAGE_DISTANCE`] bits), approximate when it is not or cannot be told, and an orphan
//! when the page is gone.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::annotations::{AnchorState, Segment, Selector, SelectorKind};
use crate::model::RevisionId;

/// How many of the 64 bits two hashes of the same page may differ in: what rendering the page
/// again, at another size, with another build of the renderer, changes. A different page of text
/// differs in about half of them.
pub const MAX_SAME_PAGE_DISTANCE: u32 = 10;
/// The most quads a mark may be drawn with, and points a stroke may have.
pub const MAX_REGION_QUADS: usize = 4096;
pub const MAX_PATH_POINTS: usize = 4096;
/// The most lines of the detector a mark may cover.
pub const MAX_REGION_LINES: usize = 1024;

/// How a mark on a picture was drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegionTool {
    /// Along lines the detector found.
    Lines,
    /// A box.
    Rect,
    /// A stroke.
    Free,
    /// A note pinned at a point, its icon's box: a PDF's sticky note.
    Point,
    /// Over quads given from outside, with no lines of the detector or text under them: a PDF's
    /// highlight or underline on a page whose text cannot be read.
    Quads,
}

/// One of the detector's lines a mark covers, from and to fractions along it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegionLine {
    pub index: u32,
    pub from: f32,
    pub to: f32,
}

/// What a `page_region` selector holds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Region {
    pub page: u32,
    pub tool: RegionTool,
    /// What the mark is drawn with: top left, top right, bottom right, bottom left, as fractions
    /// of the unrotated crop box.
    pub quads: Vec<[f32; 8]>,
    /// Its box: left, top, right, bottom.
    pub clip: [f32; 4],
    #[serde(default)]
    pub lines: Vec<RegionLine>,
    /// A stroke's points, u and v in turn.
    #[serde(default)]
    pub path: Vec<f32>,
    /// A stroke's width in points, when it is not the reader's own (a stroke from a PDF).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f32>,
    /// Sixteen hex digits, or nothing when the page had not been looked at yet.
    #[serde(default)]
    pub page_hash: Option<String>,
    #[serde(default)]
    pub region_hash: Option<String>,
}

/// A hash as the selector spells it.
pub fn hash_text(hash: u64) -> String {
    format!("{hash:016x}")
}

fn parse_hash(text: &str) -> Option<u64> {
    (text.len() == 16)
        .then(|| u64::from_str_radix(text, 16).ok())
        .flatten()
}

fn in_page(value: f32) -> bool {
    value.is_finite() && (-0.01..=1.01).contains(&value)
}

impl Region {
    /// Why this cannot be a mark, if it cannot.
    pub fn problem(&self) -> Option<String> {
        if self.quads.is_empty() || self.quads.len() > MAX_REGION_QUADS {
            return Some(format!("a mark of {} quads", self.quads.len()));
        }
        if !self.quads.iter().flatten().all(|&value| in_page(value)) {
            return Some("a mark off the page".to_owned());
        }
        let [left, top, right, bottom] = self.clip;
        if !(self.clip.iter().all(|&value| in_page(value)) && left < right && top < bottom) {
            return Some("a box that is not one".to_owned());
        }
        if !self.path.len().is_multiple_of(2)
            || self.path.len() > MAX_PATH_POINTS * 2
            || !self.path.iter().all(|&value| in_page(value))
        {
            return Some("a stroke that is not one".to_owned());
        }
        if (self.tool == RegionTool::Free) != !self.path.is_empty() {
            return Some("only a stroke has a path".to_owned());
        }
        if self.width.is_some_and(|width| {
            self.tool != RegionTool::Free || !(width.is_finite() && width > 0.0 && width <= 200.0)
        }) {
            return Some("only a stroke has a width, and a sensible one".to_owned());
        }
        if self.lines.len() > MAX_REGION_LINES
            || (self.tool == RegionTool::Lines) == self.lines.is_empty()
            || !self.lines.iter().all(|line| {
                line.from.is_finite()
                    && line.to.is_finite()
                    && 0.0 <= line.from
                    && line.from <= line.to
                    && line.to <= 1.0
            })
        {
            return Some("the lines of a mark along lines".to_owned());
        }
        for hash in [&self.page_hash, &self.region_hash].into_iter().flatten() {
            if parse_hash(hash).is_none() {
                return Some(format!("{hash:?} is not a hash"));
            }
        }
        None
    }

    /// The page hash, if it has one.
    pub fn page_hash(&self) -> Option<u64> {
        self.page_hash.as_deref().and_then(parse_hash)
    }

    /// As a selector worked out on `revision`. Lines count in the detector's `lines_version`.
    pub fn selector(&self, revision: RevisionId, lines_version: u32) -> Selector {
        Selector {
            kind: SelectorKind::PageRegion,
            data: serde_json::to_value(self).unwrap_or(Value::Null),
            priority: 0,
            normalizer_version: (self.tool == RegionTool::Lines).then_some(lines_version),
            revision_id: Some(revision),
        }
    }

    /// As the one segment of an annotation.
    pub fn segment(&self, ordinal: u32, revision: RevisionId, lines_version: u32) -> Segment {
        let [left, top, right, bottom] = self.clip.map(f64::from);
        Segment {
            ordinal,
            page: Some(self.page),
            href: None,
            clip: Some([left, top, right, bottom]),
            anchor_state: AnchorState::Exact,
            selectors: vec![self.selector(revision, lines_version)],
        }
    }
}

/// The region a segment keeps, if it is a mark on a picture.
pub fn region_of(segment: &Segment) -> Option<Region> {
    let selector = segment
        .selectors
        .iter()
        .find(|selector| selector.kind == SelectorKind::PageRegion)?;
    serde_json::from_value(selector.data.clone()).ok()
}

/// Where a mark on a picture is on the document as it is now.
#[derive(Clone, Debug, PartialEq)]
pub enum Resolution {
    /// On its page, as sure as the picture says.
    Found(Region, AnchorState),
    /// Its page is gone.
    Orphan,
    /// Not told yet: the page of the new revision has not been looked at. Shown where it was,
    /// and checked again later.
    Undecided(Region),
}

/// Where a mark on a picture is now. `page_count` is the document's pages now and `page_hash` the
/// hash of that page now, if known.
pub fn resolve(region: Region, page_count: u32, page_hash: Option<u64>) -> Resolution {
    if region.page >= page_count {
        return Resolution::Orphan;
    }
    match (region.page_hash(), page_hash) {
        (Some(was), Some(now)) if (was ^ now).count_ones() <= MAX_SAME_PAGE_DISTANCE => {
            Resolution::Found(region, AnchorState::Exact)
        }
        (Some(_), None) => Resolution::Undecided(region),
        _ => Resolution::Found(region, AnchorState::Approximate),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(tool: RegionTool) -> Region {
        Region {
            page: 3,
            tool,
            quads: vec![[0.1, 0.2, 0.5, 0.2, 0.5, 0.25, 0.1, 0.25]],
            clip: [0.1, 0.2, 0.5, 0.25],
            lines: if tool == RegionTool::Lines {
                vec![RegionLine {
                    index: 4,
                    from: 0.0,
                    to: 0.6,
                }]
            } else {
                Vec::new()
            },
            path: if tool == RegionTool::Free {
                vec![0.1, 0.2, 0.5, 0.25]
            } else {
                Vec::new()
            },
            width: None,
            page_hash: Some(hash_text(0xF0F0_0000_FFFF_1234)),
            region_hash: None,
        }
    }

    #[test]
    fn a_region_goes_through_its_selector_and_back() {
        for tool in [
            RegionTool::Lines,
            RegionTool::Rect,
            RegionTool::Free,
            RegionTool::Point,
            RegionTool::Quads,
        ] {
            let region = region(tool);
            assert_eq!(region.problem(), None, "{tool:?}");
            let segment = region.segment(0, 7, 1);
            assert_eq!(segment.clip, Some([0.1f32, 0.2, 0.5, 0.25].map(f64::from)));
            assert_eq!(
                segment.selectors[0].normalizer_version,
                (tool == RegionTool::Lines).then_some(1)
            );
            assert_eq!(region_of(&segment), Some(region));
        }
        assert_eq!(
            region(RegionTool::Rect).page_hash(),
            Some(0xF0F0_0000_FFFF_1234),
            "all 64 bits, through JSON"
        );
    }

    #[test]
    fn what_is_not_a_mark_is_refused() {
        let broken = [
            Region {
                quads: Vec::new(),
                ..region(RegionTool::Rect)
            },
            Region {
                clip: [0.5, 0.2, 0.1, 0.25],
                ..region(RegionTool::Rect)
            },
            Region {
                quads: vec![[f32::NAN; 8]],
                ..region(RegionTool::Rect)
            },
            Region {
                path: vec![0.1],
                ..region(RegionTool::Free)
            },
            Region {
                path: Vec::new(),
                ..region(RegionTool::Free)
            },
            Region {
                lines: Vec::new(),
                ..region(RegionTool::Lines)
            },
            Region {
                page_hash: Some("not a hash".to_owned()),
                ..region(RegionTool::Rect)
            },
            Region {
                width: Some(3.0),
                ..region(RegionTool::Rect)
            },
            Region {
                width: Some(f32::NAN),
                ..region(RegionTool::Free)
            },
        ];
        assert_eq!(
            Region {
                width: Some(3.0),
                ..region(RegionTool::Free)
            }
            .problem(),
            None
        );
        for region in broken {
            assert!(region.problem().is_some(), "{region:?}");
        }
    }

    #[test]
    fn the_same_picture_is_exact_another_is_approximate_and_a_lost_page_an_orphan() {
        let was = 0xF0F0_0000_FFFF_1234u64;
        let found = |page_count, now| resolve(region(RegionTool::Rect), page_count, now);
        let state = |resolution| match resolution {
            Resolution::Found(_, state) => Some(state),
            _ => None,
        };
        assert_eq!(
            state(found(10, Some(was ^ 0b1011))),
            Some(AnchorState::Exact),
            "three bits apart: the same page drawn again"
        );
        assert_eq!(state(found(10, Some(!was))), Some(AnchorState::Approximate));
        assert_eq!(found(3, Some(was)), Resolution::Orphan, "the page is gone");
        assert!(
            matches!(found(10, None), Resolution::Undecided(_)),
            "not looked at yet: told later"
        );
        let unhashed = Region {
            page_hash: None,
            ..region(RegionTool::Rect)
        };
        assert_eq!(
            state(resolve(unhashed, 10, Some(was))),
            Some(AnchorState::Approximate),
            "nothing to compare with"
        );
    }
}

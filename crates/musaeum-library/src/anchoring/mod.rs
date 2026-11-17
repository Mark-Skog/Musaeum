//! How an annotation finds its text again: on the revision it was made on, after the file changed,
//! after the normalizer or the text layer changed, and never by deleting anything when it cannot.
//!
//! Each kind of layout has its cascade of selectors ([`fixed`], [`flow`]), and both end at the
//! quote, which [`text`] finds again with what surrounds it, through spacing and light edits.

pub mod fixed;
pub mod flow;
pub mod region;
pub mod text;

use crate::annotations::AnchorState;

/// The state of an annotation from those of its segments: exact only if every one is, an orphan
/// only if every one is.
pub fn overall(states: impl IntoIterator<Item = AnchorState>) -> AnchorState {
    let mut any = false;
    let mut all_exact = true;
    let mut all_orphan = true;
    for state in states {
        any = true;
        all_exact &= state == AnchorState::Exact;
        all_orphan &= state == AnchorState::Orphan;
    }
    match (any, all_exact, all_orphan) {
        (false, ..) | (true, true, _) => AnchorState::Exact,
        (true, false, true) => AnchorState::Orphan,
        _ => AnchorState::Approximate,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use AnchorState::{Approximate, Exact, Orphan};

    #[test]
    fn an_annotation_is_as_found_as_its_segments() {
        assert_eq!(overall([Exact, Exact]), Exact);
        assert_eq!(overall([Exact, Orphan]), Approximate);
        assert_eq!(overall([Approximate]), Approximate);
        assert_eq!(overall([Orphan, Orphan]), Orphan);
        assert_eq!(
            overall([]),
            Exact,
            "a note on the whole document has nothing to lose"
        );
    }
}

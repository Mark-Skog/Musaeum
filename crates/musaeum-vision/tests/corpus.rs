//! The detector against the labeled corpus of `musaeum_vision::synth`.
//!
//! Phase 5, Part 1 asked F1 ≥ 0.95 on straight pages (square, skewed, two columns, two columns above a
//! photograph) and F1 ≥ 0.85 on pages curved toward the spine. Part 2 adds three layouts and asks:
//! F1 ≥ 0.90 on crooked lines, tilting and waving differently down the page; recall ≥ 0.90 of the lines of
//! blocks turned 90°, 270°, 45° and −30°, read the right way round, with F1 ≥ 0.95 on those pages;
//! and on pages with a halftone plate, a chart and a ruled table, no line found inside them that is
//! not one of their labels or cells, with F1 ≥ 0.95 on the page's text: its prose, the plate's
//! caption and the table's rows. The chart's own labels (numbers of one to three letters up its axis,
//! a word of three to five letters at 45° under each bar, the axis title at 90°) are too short for
//! the detector, which needs a few letters in a row to see a line: their recall is reported, and only
//! held to what it is, a known limit (ADR 0009). The time per page is measured by
//! `corpus-check --lines-synthetic`, in a release build; here only the answers count.

use musaeum_vision::score::{Score, matches, score};
use musaeum_vision::synth::{Layout, SynthPage, page};
use musaeum_vision::{DetectOptions, Detection, detect};

const PAGES: u64 = 8;

/// The columns `synth` gives a chart's labels: its numbers, its words at 45°, its axis title.
const CHART_LABELS: std::ops::RangeInclusive<u16> = 5..=12;

/// The totals of a layout, with what Part 2 counts on top of F1.
#[derive(Default)]
struct Measured {
    total: Score,
    /// Without the chart's labels: neither the labels nor the lines found on them count.
    text: Score,
    labels_truth: usize,
    labels_matched: usize,
    turned_truth: usize,
    turned_matched: usize,
    false_in_graphics: usize,
}

fn false_in_graphics(page: &SynthPage, found: &Detection) -> usize {
    let matched = matches(&found.pixel_lines, &page.lines);
    found
        .pixel_lines
        .iter()
        .zip(matched)
        .filter(|(line, matched)| {
            matched.is_none()
                && page.graphics.iter().any(|[left, top, right, bottom]| {
                    let inside = line
                        .baseline
                        .iter()
                        .filter(|[x, y]| x >= left && x <= right && y >= top && y <= bottom)
                        .count();
                    inside * 2 >= line.baseline.len()
                })
        })
        .count()
}

fn measure(layout: Layout) -> Measured {
    let mut out = Measured::default();
    for seed in 0..PAGES {
        let synthetic = page(layout, seed);
        let found = detect(&synthetic.gray, &DetectOptions::default());
        let result = score(&found.pixel_lines, &synthetic.lines);
        let matched = matches(&found.pixel_lines, &synthetic.lines);
        let turned_truth = synthetic.lines.iter().filter(|line| line.turned).count();
        let turned_matched = matched
            .iter()
            .flatten()
            .filter(|&&t| synthetic.lines[t].turned)
            .count();
        let in_graphics = false_in_graphics(&synthetic, &found);
        eprintln!(
            "{} seed {seed}: {} true, {} found, {} matched; turned {turned_matched}/{turned_truth}, {in_graphics} false in graphics (skew {:.2}°, curve {:.0} px); blocks {:?}, dropped {:?}, picture cells {}",
            layout.name(),
            result.truth,
            result.found,
            result.matched,
            synthetic.skew_degrees,
            synthetic.curve_depth,
            found
                .stats
                .turned_blocks
                .iter()
                .map(|a| a.round())
                .collect::<Vec<_>>(),
            found.stats.dropped,
            found.stats.picture_cells,
        );
        out.total.add(result);
        let label = |t: usize| CHART_LABELS.contains(&synthetic.lines[t].column);
        let on_labels = matched.iter().flatten().filter(|&&t| label(t)).count();
        let labels = (0..synthetic.lines.len()).filter(|&t| label(t)).count();
        out.text.add(Score {
            truth: result.truth - labels,
            found: result.found - on_labels,
            matched: result.matched - on_labels,
        });
        out.labels_truth += labels;
        out.labels_matched += on_labels;
        out.turned_truth += turned_truth;
        out.turned_matched += turned_matched;
        out.false_in_graphics += in_graphics;
    }
    eprintln!(
        "{}: precision {:.3}, recall {:.3}, F1 {:.3}; turned {}/{}; {} false in graphics; without the chart's labels F1 {:.3}, labels {}/{}",
        layout.name(),
        out.total.precision(),
        out.total.recall(),
        out.total.f1(),
        out.turned_matched,
        out.turned_truth,
        out.false_in_graphics,
        out.text.f1(),
        out.labels_matched,
        out.labels_truth,
    );
    out
}

#[test]
fn straight_pages_reach_an_f1_of_0_95() {
    for layout in [
        Layout::Straight,
        Layout::Skewed,
        Layout::TwoColumns,
        Layout::Mixed,
    ] {
        let total = measure(layout).total;
        assert!(
            total.f1() >= 0.95,
            "{}: {total:?}, F1 {:.3}",
            layout.name(),
            total.f1()
        );
    }
}

#[test]
fn pages_curved_toward_the_spine_reach_an_f1_of_0_85() {
    let total = measure(Layout::Curved).total;
    assert!(total.f1() >= 0.85, "{total:?}, F1 {:.3}", total.f1());
}

#[test]
fn crooked_lines_reach_an_f1_of_0_90() {
    let total = measure(Layout::Crooked).total;
    assert!(total.f1() >= 0.90, "{total:?}, F1 {:.3}", total.f1());
}

#[test]
fn turned_blocks_are_found_and_read_the_right_way_round() {
    let measured = measure(Layout::Turned);
    let recall = measured.turned_matched as f64 / measured.turned_truth.max(1) as f64;
    assert!(
        recall >= 0.90,
        "{}/{}",
        measured.turned_matched,
        measured.turned_truth
    );
    assert!(
        measured.total.f1() >= 0.95,
        "{:?}, F1 {:.3}",
        measured.total,
        measured.total.f1()
    );
}

#[test]
fn nothing_in_pictures_charts_or_tables_is_taken_for_a_line() {
    let measured = measure(Layout::Graphics);
    assert_eq!(measured.false_in_graphics, 0);
    assert!(
        measured.text.f1() >= 0.95,
        "{:?}, F1 {:.3}",
        measured.text,
        measured.text.f1()
    );
    // A known limit, held where it is so that it gets no worse unnoticed.
    assert!(
        measured.labels_matched * 100 >= measured.labels_truth * 14,
        "chart labels {}/{}",
        measured.labels_matched,
        measured.labels_truth
    );
}

#[test]
fn the_corpus_is_the_same_for_the_same_seed() {
    let a = page(Layout::Curved, 3);
    let b = page(Layout::Curved, 3);
    assert_eq!(a.gray, b.gray);
    assert_eq!(a.lines, b.lines);
    assert!(a.lines.len() > 30, "{}", a.lines.len());
    let turned = page(Layout::Turned, 1);
    assert!(turned.lines.iter().filter(|line| line.turned).count() >= 10);
    assert_eq!(page(Layout::Graphics, 2).graphics.len(), 3);
}

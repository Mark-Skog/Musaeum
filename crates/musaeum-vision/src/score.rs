//! How well the lines found on a page match the lines it really has: precision, recall and F1.
//!
//! A found line counts for a true one when they run the same way, cover most of each other along
//! the true line and their baselines are close where they overlap, all measured along the true
//! line's own direction, so a line at 90° or 45° is judged as a level one is. Each line counts at most once, so a line cut in two
//! is one miss and two false finds, and two lines run together are two misses and one false find:
//! what a reader would see as a selection that stops halfway, or that takes two lines at once.

use crate::lines::PixelLine;
use crate::synth::TruthLine;

/// How much of each line the other must cover along the true line.
pub const MIN_OVERLAP: f32 = 0.7;
/// How far apart the baselines may be, on average where they overlap, in x-heights.
pub const MAX_OFFSET: f32 = 0.5;

/// The result on one page or summed over many.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Score {
    pub truth: usize,
    pub found: usize,
    pub matched: usize,
}

impl Score {
    pub fn precision(&self) -> f64 {
        if self.found == 0 {
            return if self.truth == 0 { 1.0 } else { 0.0 };
        }
        self.matched as f64 / self.found as f64
    }

    pub fn recall(&self) -> f64 {
        if self.truth == 0 {
            return 1.0;
        }
        self.matched as f64 / self.truth as f64
    }

    pub fn f1(&self) -> f64 {
        let (p, r) = (self.precision(), self.recall());
        if p + r == 0.0 {
            0.0
        } else {
            2.0 * p * r / (p + r)
        }
    }

    pub fn add(&mut self, other: Score) {
        self.truth += other.truth;
        self.found += other.found;
        self.matched += other.matched;
    }
}

/// The way a polyline runs: its first point and the unit direction to its last.
fn axis(points: &[[f32; 2]]) -> ([f32; 2], [f32; 2]) {
    let first = points[0];
    let last = points[points.len() - 1];
    let length = (last[0] - first[0]).hypot(last[1] - first[1]).max(1e-3);
    (
        first,
        [(last[0] - first[0]) / length, (last[1] - first[1]) / length],
    )
}

/// The polyline in the frame of `origin` and `direction`: along it and across it.
fn project(points: &[[f32; 2]], origin: [f32; 2], direction: [f32; 2]) -> Vec<[f32; 2]> {
    let mut out: Vec<[f32; 2]> = points
        .iter()
        .map(|p| {
            let (dx, dy) = (p[0] - origin[0], p[1] - origin[1]);
            [
                dx * direction[0] + dy * direction[1],
                -dx * direction[1] + dy * direction[0],
            ]
        })
        .collect();
    out.sort_by(|a, b| a[0].total_cmp(&b[0]));
    out
}

/// How far across at `t` along, held level past the ends.
fn across_at(points: &[[f32; 2]], t: f32) -> f32 {
    let Some(first) = points.first() else {
        return 0.0;
    };
    if t <= first[0] {
        return first[1];
    }
    for pair in points.windows(2) {
        let ([at, ad], [bt, bd]) = (pair[0], pair[1]);
        if t <= bt {
            let k = if bt > at { (t - at) / (bt - at) } else { 0.0 };
            return ad + (bd - ad) * k;
        }
    }
    points[points.len() - 1][1]
}

/// The mean distance between two baselines where they overlap, if they run the same way and
/// overlap enough to be the same line.
fn offset(found: &PixelLine, truth: &TruthLine) -> Option<f32> {
    if found.baseline.len() < 2 || truth.baseline.len() < 2 {
        return None;
    }
    let (origin, direction) = axis(&truth.baseline);
    let (_, found_direction) = axis(&found.baseline);
    // The same way, within 20°.
    if found_direction[0] * direction[0] + found_direction[1] * direction[1] < 0.94 {
        return None;
    }
    let real = project(&truth.baseline, origin, direction);
    let seen = project(&found.baseline, origin, direction);
    let (tl, tr) = (real[0][0], real[real.len() - 1][0]);
    let (fl, fr) = (seen[0][0], seen[seen.len() - 1][0]);
    let (left, right) = (fl.max(tl), fr.min(tr));
    let overlap = right - left;
    if overlap <= 0.0
        || overlap < MIN_OVERLAP * (fr - fl).max(1.0)
        || overlap < MIN_OVERLAP * (tr - tl).max(1.0)
    {
        return None;
    }
    let samples = 24;
    let mut sum = 0.0;
    for step in 0..=samples {
        let t = left + overlap * step as f32 / samples as f32;
        sum += (across_at(&seen, t) - across_at(&real, t)).abs();
    }
    let mean = sum / (samples + 1) as f32;
    (mean <= MAX_OFFSET * truth.x_height).then_some(mean)
}

/// Matches found lines to true ones, closest first, each at most once: for each found line, the
/// true line it matched, if any.
pub fn matches(found: &[PixelLine], truth: &[TruthLine]) -> Vec<Option<usize>> {
    let mut pairs = Vec::new();
    for (f, line) in found.iter().enumerate() {
        for (t, real) in truth.iter().enumerate() {
            if let Some(distance) = offset(line, real) {
                pairs.push((distance, f, t));
            }
        }
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out = vec![None; found.len()];
    let mut used_truth = vec![false; truth.len()];
    for (_, f, t) in pairs {
        if out[f].is_none() && !used_truth[t] {
            out[f] = Some(t);
            used_truth[t] = true;
        }
    }
    out
}

/// Matches found lines to true ones, closest first, each at most once.
pub fn score(found: &[PixelLine], truth: &[TruthLine]) -> Score {
    let mut pairs = Vec::new();
    for (f, line) in found.iter().enumerate() {
        for (t, real) in truth.iter().enumerate() {
            if let Some(distance) = offset(line, real) {
                pairs.push((distance, f, t));
            }
        }
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut used_found = vec![false; found.len()];
    let mut used_truth = vec![false; truth.len()];
    let mut matched = 0;
    for (_, f, t) in pairs {
        if !used_found[f] && !used_truth[t] {
            used_found[f] = true;
            used_truth[t] = true;
            matched += 1;
        }
    }
    Score {
        truth: truth.len(),
        found: found.len(),
        matched,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(points: Vec<[f32; 2]>) -> PixelLine {
        PixelLine {
            column: 0,
            left: points[0][0],
            right: points[points.len() - 1][0],
            baseline: points,
            x_height: 10.0,
            ascent: 14.0,
            descent: 4.0,
            gaps: Vec::new(),
            gaps_are_arc: false,
        }
    }

    fn truth(y: f32) -> TruthLine {
        TruthLine {
            column: 0,
            baseline: vec![[100.0, y], [900.0, y]],
            x_height: 10.0,
            turned: false,
        }
    }

    #[test]
    fn a_line_cut_in_two_or_run_into_the_next_does_not_count() {
        let real = [truth(100.0), truth(130.0)];
        let whole = [
            found(vec![[102.0, 101.0], [898.0, 101.0]]),
            found(vec![[100.0, 129.0], [900.0, 131.0]]),
        ];
        assert_eq!(score(&whole, &real).f1(), 1.0);
        let cut = [
            found(vec![[100.0, 100.0], [480.0, 100.0]]),
            found(vec![[520.0, 100.0], [900.0, 100.0]]),
            whole[1].clone(),
        ];
        let result = score(&cut, &real);
        assert_eq!((result.matched, result.found, result.truth), (1, 3, 2));
        let off = [found(vec![[100.0, 108.0], [900.0, 108.0]])];
        assert_eq!(
            score(&off, &real[..1]).matched,
            0,
            "most of a line's height away"
        );
        assert_eq!(score(&[], &[]).f1(), 1.0);
    }

    #[test]
    fn a_turned_line_is_judged_along_itself_and_must_read_the_same_way() {
        let up = TruthLine {
            column: 0,
            baseline: vec![[50.0, 900.0], [50.0, 100.0]],
            x_height: 10.0,
            turned: true,
        };
        let found_up = found(vec![[52.0, 895.0], [51.0, 105.0]]);
        assert_eq!(score(&[found_up], std::slice::from_ref(&up)).matched, 1);
        let found_down = found(vec![[51.0, 105.0], [52.0, 895.0]]);
        assert_eq!(
            score(&[found_down], &[up]).matched,
            0,
            "read the other way round"
        );
    }
}

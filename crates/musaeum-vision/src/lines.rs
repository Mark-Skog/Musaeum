//! From the ink to the lines of text: columns, then, column by column, the rows where the ink
//! is, strip by strip across the width, linked into lines whose baselines follow the paper.
//!
//! A page is read in vertical strips. In each strip the ink is projected onto the vertical axis
//! (sheared by the page's skew, so a turned page still gives sharp peaks) and every run of rows
//! with ink is a band: a piece of a line. Bands of neighbouring strips are linked by proximity,
//! following the trend of the line so far, which is what lets a baseline bend near the spine of a
//! scanned book. The baseline of a band is the bottom of its densest rows, where the small
//! letters sit; the tallest and lowest ink of the band give the ascent and the descent.

use crate::components::Component;
use crate::sauvola::Bitmap;

/// A line of text, in pixels of the picture it was found on.
#[derive(Clone, Debug, PartialEq)]
pub struct PixelLine {
    pub column: u16,
    /// The baseline, left to right.
    pub baseline: Vec<[f32; 2]>,
    pub x_height: f32,
    pub ascent: f32,
    pub descent: f32,
    /// Word gaps as fractions of the line from its first point to its last.
    pub gaps: Vec<[f32; 2]>,
    /// The first and last pixel column with ink.
    pub left: f32,
    pub right: f32,
    /// Whether `gaps` are already fractions of the baseline's length rather than of its x-span, as
    /// they are once a turned line is carried back onto the page.
    pub gaps_are_arc: bool,
}

/// A column of text, in pixels: its x range, and the y range of its ink.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PixelColumn {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

/// What the line finder is told about the page.
#[derive(Clone, Copy, Debug)]
pub struct Context {
    /// The typical height of a letter, in pixels.
    pub glyph_height: f32,
    /// The page's skew, radians, positive when lines go down to the right.
    pub skew: f32,
    pub width: u32,
    pub height: u32,
}

/// A rectangle of the page that is one column of text over some height of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Area {
    pub left: u32,
    pub right: u32,
    pub top: u32,
    pub bottom: u32,
}

/// How many lines, about, a gutter has to stay open beside to be one.
const GUTTER_LINES: usize = 4;

/// The page cut into bands, top to bottom, each band into its columns, left to right: reading
/// order. Columns are found where they are, not over the whole page: two columns of text above a
/// figure or a table across the page, then text in one column again, are three bands.
///
/// The text's height is cut into slices about a line high, and in each slice every glyph, padded
/// by half a letter, blocks the pixels across it. A gutter is a run of pixels, wide enough, open
/// in slices one after another, beside text on both sides, over at least [`GUTTER_LINES`] lines. A
/// band is a stretch of height where the same gutters are open; its edges move to the emptiest
/// row nearby, so they fall between lines.
pub fn find_regions(glyphs: &[Component], context: &Context) -> Vec<Vec<Area>> {
    find_regions_with_tables(glyphs, context, &[], &[])
}

/// A horizontal rule of the page: from x, to x, at y.
pub type Rule = (u32, u32, u32);

/// [`find_regions`], with the ruled tables of the page: inside a table's frame, or where two rules
/// or more cross a gap between cells, the gap is no gutter, so a row is read across its cells, as
/// a text layer reads it.
pub fn find_regions_with_tables(
    glyphs: &[Component],
    context: &Context,
    tables: &[Area],
    rules: &[Rule],
) -> Vec<Vec<Area>> {
    let width = context.width as usize;
    if glyphs.is_empty() || width == 0 {
        return Vec::new();
    }
    let g = context.glyph_height.max(4.0);
    let left = glyphs.iter().map(|c| c.left).min().unwrap_or(0);
    let right = glyphs.iter().map(|c| c.right).max().unwrap_or(0);
    let top = glyphs.iter().map(|c| c.top).min().unwrap_or(0);
    let bottom = glyphs.iter().map(|c| c.bottom).max().unwrap_or(0);
    let whole = Area {
        left,
        right: right.max(left + 1),
        top,
        bottom: bottom.max(top + 1),
    };
    if right <= left || bottom <= top {
        return vec![vec![whole]];
    }
    let slice_height = (g * 2.0).max(6.0);
    let slices = (((bottom - top) as f32 / slice_height).ceil() as usize).clamp(1, 1000);
    let mut blocked = vec![false; width * slices];
    let mut inked = vec![false; slices];
    let pad = (g * 0.5) as usize;
    // Rows covered by glyphs, for putting the bands' edges between lines.
    let mut cover = vec![0i32; (bottom - top) as usize + 2];
    for glyph in glyphs {
        let h = glyph.height() as f32;
        // Only body text decides the columns: big letters are headings or ornaments, and blobs
        // much smaller than a letter are the grain of the paper or a photograph, which would
        // close every gutter of a scan.
        if h > g * 2.2 || h < g * 0.4 {
            continue;
        }
        let first = (((glyph.top - top) as f32 / slice_height) as usize).min(slices - 1);
        let last = (((glyph.bottom - top) as f32 - 0.01) / slice_height).max(0.0) as usize;
        let last = last.min(slices - 1);
        let x0 = (glyph.left as usize).saturating_sub(pad);
        let x1 = (glyph.right as usize + pad).min(width);
        for slice in first..=last {
            inked[slice] = true;
            for x in x0..x1 {
                blocked[x * slices + slice] = true;
            }
        }
        cover[(glyph.top - top) as usize] += 1;
        cover[(glyph.bottom - top) as usize] -= 1;
    }
    // A floor from the short side, not the width: a spread of two pages is twice as wide, its gutters
    // are not.
    let min_gutter = (g * 1.2).max(context.width.min(context.height) as f32 * 0.01) as usize;

    // Gutters, followed down the slices.
    struct Track {
        x0: usize,
        x1: usize,
        first: usize,
        last: usize,
        count: usize,
    }
    let mut open_tracks: Vec<Track> = Vec::new();
    let mut gutters: Vec<Track> = Vec::new();
    for slice in 0..slices {
        if !inked[slice] {
            continue;
        }
        // The open runs of this slice that have text on both sides.
        let is_blocked = |x: usize| blocked[x * slices + slice];
        let text_left = (left as usize..right as usize).find(|&x| is_blocked(x));
        let text_right = (left as usize..right as usize)
            .rev()
            .find(|&x| is_blocked(x));
        let mut runs = Vec::new();
        if let (Some(from), Some(to)) = (text_left, text_right) {
            let mut x = from;
            while x < to {
                if is_blocked(x) {
                    x += 1;
                    continue;
                }
                let start = x;
                while x < to && !is_blocked(x) {
                    x += 1;
                }
                if x - start >= min_gutter {
                    runs.push((start, x));
                }
            }
        }
        let mut used = vec![false; runs.len()];
        let mut still = Vec::new();
        for mut track in open_tracks.drain(..) {
            let next = runs.iter().enumerate().find(|(index, run)| {
                !used[*index] && run.1.min(track.x1) >= run.0.max(track.x0) + min_gutter
            });
            match next {
                Some((index, run)) => {
                    used[index] = true;
                    track.x0 = track.x0.max(run.0);
                    track.x1 = track.x1.min(run.1);
                    track.last = slice;
                    track.count += 1;
                    still.push(track);
                }
                None => gutters.push(track),
            }
        }
        for (index, run) in runs.iter().enumerate() {
            if !used[index] {
                still.push(Track {
                    x0: run.0,
                    x1: run.1,
                    first: slice,
                    last: slice,
                    count: 1,
                });
            }
        }
        open_tracks = still;
    }
    gutters.extend(open_tracks);
    gutters.retain(|track| track.count >= GUTTER_LINES);

    // The bands: where gutters start and end, moved to the emptiest row nearby.
    let mut running = 0i32;
    let rows: Vec<i32> = cover
        .iter()
        .map(|delta| {
            running += delta;
            running
        })
        .collect();
    let emptiest = |y: f32| -> u32 {
        let reach = slice_height as i64;
        let center = (y - top as f32) as i64;
        let from = (center - reach).max(0) as usize;
        let to = ((center + reach).max(0) as usize).min(rows.len().saturating_sub(1));
        let mut best = center.clamp(0, rows.len() as i64 - 1) as usize;
        for row in from..=to {
            let distance = (row as i64 - center).abs();
            let best_distance = (best as i64 - center).abs();
            if rows[row] < rows[best] || (rows[row] == rows[best] && distance < best_distance) {
                best = row;
            }
        }
        top + best as u32
    };
    let edges_of = |track: &Track| {
        (
            emptiest(top as f32 + track.first as f32 * slice_height),
            emptiest(top as f32 + (track.last + 1) as f32 * slice_height),
        )
    };
    let slack = slice_height as u32;
    let mut spans: Vec<(u32, u32, u32, usize)> = gutters
        .iter()
        .map(|track| {
            let (from, to) = edges_of(track);
            (((track.x0 + track.x1) / 2) as u32, from, to, track.count)
        })
        .filter(|&(x, from, to, _)| {
            let framed = tables.iter().any(|table| {
                x > table.left
                    && x < table.right
                    && from + slack >= table.top
                    && to <= table.bottom + slack
            });
            let reach = min_gutter as u32;
            // Rules between the rows: a rule over the first line (a heading's underline) or under
            // the last (the edge of a picture below the text) runs across columns too, and is no
            // table's.
            let inside = (g * 1.5) as u32;
            let crossed = rules
                .iter()
                .filter(|&&(x0, x1, y)| {
                    x0 + reach < x && x1 > x + reach && y > from + inside && y + inside < to
                })
                .count()
                >= 2;
            !framed && !crossed
        })
        .collect();
    let build = |spans: &[(u32, u32, u32, usize)]| -> Vec<Vec<Area>> {
        let mut cuts = vec![top, bottom + 1];
        for (_, from, to, _) in spans {
            cuts.push(*from);
            cuts.push(*to);
        }
        cuts.sort_unstable();
        cuts.dedup();
        let mut bands: Vec<(u32, u32, Vec<u32>)> = Vec::new();
        for pair in cuts.windows(2) {
            let (from, to) = (pair[0], pair[1]);
            if to <= from {
                continue;
            }
            let middle = (from + to) / 2;
            let mut splits: Vec<u32> = spans
                .iter()
                .filter(|(_, start, end, _)| *start <= middle && middle < *end)
                .map(|(x, ..)| *x)
                .collect();
            splits.sort_unstable();
            splits.dedup_by(|a, b| a.abs_diff(*b) < min_gutter as u32 * 2);
            match bands.last_mut() {
                Some(last) if last.2 == splits => last.1 = to,
                _ => bands.push((from, to, splits)),
            }
        }

        // Each band's columns; a column with almost nothing in it is not one.
        let mut out = Vec::new();
        for (from, to, splits) in bands {
            let mut columns = Vec::new();
            let mut x = left;
            for split in splits {
                columns.push((x, split));
                x = split;
            }
            columns.push((x, right));
            // Letters of body text only: the grain of a photograph is many small specks, and a
            // column of text beside one is not "almost empty".
            let in_band = |c: &&Component| {
                let middle = (c.top + c.bottom) / 2;
                let h = c.height() as f32;
                middle >= from && middle < to && h >= g * 0.5 && h <= g * 2.2
            };
            let count = |x0: u32, x1: u32| {
                glyphs
                    .iter()
                    .filter(in_band)
                    .filter(|c| (c.left + c.right) / 2 >= x0 && (c.left + c.right) / 2 < x1)
                    .count()
            };
            let counts: Vec<usize> = columns.iter().map(|c| count(c.0, c.1)).collect();
            let fullest = counts.iter().copied().max().unwrap_or(0);
            if glyphs.iter().filter(in_band).count() == 0 {
                continue;
            }
            let mut merged: Vec<(u32, u32)> = Vec::new();
            for (column, here) in columns.into_iter().zip(counts) {
                if here * 6 < fullest && !merged.is_empty() {
                    merged.last_mut().expect("not empty").1 = column.1;
                } else {
                    merged.push(column);
                }
            }
            let bottom = to.min(context.height);
            // A band whose columns, once an almost empty one is given back, are the band above's
            // is part of it: the gutter that cut it ran beside a scribble in the margin or a speck
            // of the scan, not between columns, and a cut across a tilted page cuts its lines.
            let same = out.last().is_some_and(|above: &Vec<Area>| {
                above.len() == merged.len()
                    && above.iter().zip(&merged).all(|(area, &(x0, x1))| {
                        area.left == x0 && area.right == x1 && area.bottom == from
                    })
            });
            if same {
                for area in out.last_mut().expect("there is a band above") {
                    area.bottom = bottom;
                }
                continue;
            }
            out.push(
                merged
                    .into_iter()
                    .map(|(x0, x1)| Area {
                        left: x0,
                        right: x1,
                        top: from,
                        bottom,
                    })
                    .collect(),
            );
        }
        out
    };
    // The model has room for 64 columns. A page cut into more has false gutters, in the light and
    // shade of photographs: the weakest go first, those open beside the fewest lines.
    let mut out = build(&spans);
    while out.iter().map(Vec::len).sum::<usize>() > 64 && !spans.is_empty() {
        // Every gutter tied for weakest at once: one layout per round, not per gutter.
        let weakest = spans.iter().map(|span| span.3).min().expect("not empty");
        spans.retain(|span| span.3 > weakest);
        out = build(&spans);
    }
    if out.is_empty() {
        return vec![vec![whole]];
    }
    out
}

/// The columns of text over the whole page, left to right: those of the band with the most.
pub fn find_columns(glyphs: &[Component], context: &Context) -> Vec<(u32, u32)> {
    find_regions(glyphs, context)
        .into_iter()
        .max_by_key(|band| band.len())
        .map(|band| band.iter().map(|area| (area.left, area.right)).collect())
        .unwrap_or_default()
}

/// Joins lines the column boundary cut in two: a running head or a title centered over two
/// columns. The piece in one column and the piece in the next are one line when their baselines
/// meet and less than a gutter separates them; lines of the two columns side by side are a
/// gutter apart, which is more. The joined line stays in the first column.
pub fn join_across_columns(columns: &mut [Vec<PixelLine>], glyph_height: f32) {
    let g = glyph_height.max(4.0);
    for at in 1..columns.len() {
        let (before, after) = columns.split_at_mut(at);
        let (left_column, right_column) = (&mut before[at - 1], &mut after[0]);
        let mut taken = vec![false; right_column.len()];
        for line in left_column.iter_mut() {
            let end = *line.baseline.last().expect("a line has points");
            let found = right_column.iter().enumerate().position(|(index, other)| {
                let start = other.baseline[0];
                !taken[index]
                    && start[0] > end[0] - g
                    && start[0] - end[0] < g * 1.2
                    && (start[1] - end[1]).abs() < g * 0.3
            });
            let Some(index) = found else {
                continue;
            };
            taken[index] = true;
            let other = &right_column[index];
            let first = line.baseline[0][0];
            let old_end = end[0];
            let start = other.baseline[0][0];
            let other_end = other.baseline.last().expect("a line has points")[0];
            let extent = (other_end - first).max(1.0);
            let own = old_end - first;
            let theirs = other_end - start;
            let mut gaps: Vec<[f32; 2]> = line
                .gaps
                .iter()
                .map(|gap| [gap[0] * own / extent, gap[1] * own / extent])
                .collect();
            gaps.push([(old_end - first) / extent, (start - first) / extent]);
            gaps.extend(other.gaps.iter().map(|gap| {
                [
                    (start + gap[0] * theirs - first) / extent,
                    (start + gap[1] * theirs - first) / extent,
                ]
            }));
            line.gaps = gaps;
            line.baseline.extend_from_slice(&other.baseline);
            line.right = other.right;
            line.x_height = (line.x_height + other.x_height) / 2.0;
            line.ascent = line.ascent.max(other.ascent);
            line.descent = line.descent.max(other.descent);
        }
        let mut index = 0;
        right_column.retain(|_| {
            let keep = !taken[index];
            index += 1;
            keep
        });
    }
}

/// One run of rows with ink in one strip, in sheared coordinates.
#[derive(Clone, Copy, Debug)]
struct Band {
    strip: usize,
    top: f32,
    bottom: f32,
    /// Bottom of the densest rows: where the small letters sit.
    baseline: f32,
    /// Top of the densest rows.
    x_top: f32,
    center: f32,
}

struct Building {
    bands: Vec<Band>,
}

impl Building {
    fn last(&self) -> &Band {
        self.bands.last().expect("a line has a band")
    }

    /// Where the next band's center should be, following the line's trend.
    fn predicted(&self) -> f32 {
        self.predicted_by(|band| band.center)
    }

    /// Where the next band's baseline should be, following the line's trend.
    fn predicted_baseline(&self) -> f32 {
        self.predicted_by(|band| band.baseline)
    }

    fn predicted_by(&self, of: impl Fn(&Band) -> f32) -> f32 {
        let n = self.bands.len();
        if n >= 2 {
            let a = &self.bands[n - 2];
            let b = &self.bands[n - 1];
            let per_strip = (of(b) - of(a)) / (b.strip - a.strip).max(1) as f32;
            of(b) + per_strip * 0.8
        } else {
            of(self.last())
        }
    }
}

/// The lines of one column.
pub fn find_lines(
    mask: &Bitmap,
    column: (u32, u32),
    column_index: u16,
    context: &Context,
) -> Vec<PixelLine> {
    find_lines_in(
        mask,
        Area {
            left: column.0,
            right: column.1,
            top: 0,
            bottom: mask.height,
        },
        column_index,
        context,
    )
}

/// The lines of one area of the page: only the ink between its top and bottom rows counts.
pub fn find_lines_in(
    mask: &Bitmap,
    area: Area,
    column_index: u16,
    context: &Context,
) -> Vec<PixelLine> {
    find_lines_with_tables(mask, area, column_index, context, &[])
}

/// [`find_lines_in`], with the ruled tables of the page: inside a table, a line goes on across the
/// empty stretch between one cell and the next, however wide, so a row is one line, as a text layer
/// reads it.
pub fn find_lines_with_tables(
    mask: &Bitmap,
    area: Area,
    column_index: u16,
    context: &Context,
    tables: &[Area],
) -> Vec<PixelLine> {
    let column = (area.left, area.right);
    let width = mask.width as usize;
    let height = mask.height as usize;
    let (area_top, area_bottom) = (
        (area.top as usize).min(height),
        (area.bottom as usize).min(height),
    );
    let (left, right) = (column.0 as usize, (column.1 as usize).min(width));
    if right <= left || height == 0 {
        return Vec::new();
    }
    let g = context.glyph_height.max(4.0);
    let slope = context.skew.tan();
    let center_x = context.width as f32 / 2.0;
    let shear = |x: usize| (x as f32 - center_x) * slope;
    let strips = (((right - left) as f32 / (6.0 * g)).round() as usize).clamp(3, 24);
    let strip_width = (right - left) as f32 / strips as f32;
    // Rows of the sheared profile, with room for what the shear moves out of the page.
    let pad = (context.width as f32 * slope.abs() / 2.0).ceil() as usize + 2;
    let rows = height + 2 * pad;
    let smooth = ((g / 4.0) as usize).max(1) | 1;

    let mut bands: Vec<Band> = Vec::new();
    let mut profile = vec![0u32; rows];
    let strip_bounds = |strip: usize| -> (usize, usize) {
        let x0 = left + (strip as f32 * strip_width) as usize;
        let x1 = if strip + 1 == strips {
            right
        } else {
            left + ((strip + 1) as f32 * strip_width) as usize
        };
        (x0, x1)
    };
    for strip in 0..strips {
        let (x0, x1) = strip_bounds(strip);
        profile.fill(0);
        for y in area_top..area_bottom {
            let row = &mask.bits[y * width..(y + 1) * width];
            for (x, &bit) in (x0..x1).zip(&row[x0..x1]) {
                if bit != 0 {
                    let sheared = y as f32 - shear(x) + pad as f32;
                    let index = (sheared.round() as isize).clamp(0, rows as isize - 1) as usize;
                    profile[index] += 1;
                }
            }
        }
        let smoothed = box_smooth(&profile, smooth);
        let peak = smoothed.iter().copied().fold(0.0f32, f32::max);
        if peak <= 0.0 {
            continue;
        }
        let threshold = (peak * 0.12).max(1.5);
        // An accent or the dot of an i alone in a strip is no piece of a line: it would pull the
        // line it is linked to off its letters.
        let min_ink = g * g * 0.35;
        // The runs of rows above the threshold.
        let mut runs: Vec<(usize, usize)> = Vec::new();
        let mut y = 0;
        while y < rows {
            if smoothed[y] <= threshold {
                y += 1;
                continue;
            }
            let start = y;
            while y < rows && smoothed[y] > threshold {
                y += 1;
            }
            runs.push((start, y));
        }
        let ink_of = |(start, end): (usize, usize)| -> f32 {
            profile[start..end].iter().map(|&v| v as f32).sum()
        };
        let whole = |run: (usize, usize)| -> bool {
            (run.1 - run.0) as f32 >= g * 0.45 && ink_of(run) >= min_ink
        };
        // A short gap between two runs is inside one line (the bar of a t, a thin stroke): where
        // the strip holds only a letter or two of a line, the last of a long line, its rows may
        // fall below the threshold the strip's other lines set, and the letters come apart in
        // runs too small to be a line alone. Two such pieces, close, are one.
        let join = (g * 0.3).ceil() as usize;
        let mut at = 0;
        while at < runs.len() {
            let mut run = runs[at];
            at += 1;
            if !whole(run) && at < runs.len() {
                let next = runs[at];
                if next.0 - run.1 <= join && !whole(next) {
                    run = (run.0, next.1);
                    at += 1;
                }
            }
            let (start, end) = run;
            let band_peak = smoothed[start..end].iter().copied().fold(0.0f32, f32::max);
            let ink = ink_of(run);
            if (end - start) as f32 >= g * 0.45 && ink >= min_ink {
                let dense = band_peak * 0.5;
                let x_top = (start..end)
                    .find(|&r| smoothed[r] >= dense)
                    .unwrap_or(start);
                let baseline = (start..end)
                    .rev()
                    .find(|&r| smoothed[r] >= dense)
                    .unwrap_or(end - 1)
                    + 1;
                bands.push(Band {
                    strip,
                    top: start as f32 - pad as f32,
                    bottom: end as f32 - pad as f32,
                    baseline: baseline as f32 - pad as f32,
                    x_top: x_top as f32 - pad as f32,
                    center: (start + end) as f32 / 2.0 - pad as f32,
                });
            }
        }
    }
    if bands.is_empty() {
        return Vec::new();
    }
    // The usual distance between lines, from the bands of each strip.
    let mut pitches: Vec<f32> = Vec::new();
    for strip in 0..strips {
        let mut centers: Vec<f32> = bands
            .iter()
            .filter(|b| b.strip == strip)
            .map(|b| b.center)
            .collect();
        centers.sort_by(f32::total_cmp);
        pitches.extend(centers.windows(2).map(|w| w[1] - w[0]));
    }
    pitches.sort_by(f32::total_cmp);
    let pitch = pitches
        .get(pitches.len() / 2)
        .copied()
        .unwrap_or(g * 2.0)
        .max(g * 1.2);
    let tolerance = pitch * 0.45;

    // Whether a band, by its strip and its centre, is inside a table.
    let in_table = |strip: usize, center: f32| -> bool {
        let (x0, x1) = strip_bounds(strip);
        let x = (x0 + x1) as f32 / 2.0;
        let y = center + shear(x as usize);
        tables.iter().any(|table| {
            x >= table.left as f32
                && x < table.right as f32
                && y >= table.top as f32
                && y < table.bottom as f32
        })
    };
    // Linking, strip by strip.
    let mut lines: Vec<Building> = Vec::new();
    for strip in 0..strips {
        let mut here: Vec<Band> = bands.iter().filter(|b| b.strip == strip).copied().collect();
        here.sort_by(|a, b| a.center.total_cmp(&b.center));
        let mut taken: Vec<bool> = vec![false; lines.len()];
        for band in here {
            let mut best: Option<(usize, f32)> = None;
            for (index, line) in lines.iter().enumerate() {
                let last = line.last();
                let skip = if in_table(last.strip, last.center) && in_table(band.strip, band.center)
                {
                    usize::MAX
                } else {
                    2
                };
                if taken[index] || strip - last.strip > skip {
                    continue;
                }
                // The band's middle where the line leads, and its foot too: two lines run
                // together in a strip by a scribble down the margin make one band whose middle
                // may be near, but whose foot is the lower line's.
                let distance = (line.predicted() - band.center).abs();
                let foot = (line.predicted_baseline() - band.baseline).abs();
                if distance <= tolerance
                    && foot <= tolerance
                    && best.is_none_or(|(_, d)| distance < d)
                {
                    best = Some((index, distance));
                }
            }
            match best {
                Some((index, _)) => {
                    lines[index].bands.push(band);
                    taken[index] = true;
                }
                None => {
                    lines.push(Building { bands: vec![band] });
                    taken.push(true);
                }
            }
        }
    }

    let mut out: Vec<PixelLine> = Vec::new();
    for line in &lines {
        let bands = &line.bands;
        // Where the line's ink is, column by column, inside its bands.
        let mut counts = vec![0u32; right - left];
        for band in bands {
            let (x0, x1) = strip_bounds(band.strip);
            for x in x0..x1 {
                let top = (band.top + shear(x)).floor().max(0.0) as usize;
                let bottom = ((band.bottom + shear(x)).ceil().max(0.0) as usize).min(height);
                let mut count = 0u32;
                for y in top..bottom {
                    count += u32::from(mask.bits[y * width + x]);
                }
                counts[x - left] = count;
            }
        }
        let Some(first) = counts.iter().position(|&c| c > 0) else {
            continue;
        };
        let last = counts.iter().rposition(|&c| c > 0).expect("there is ink");
        let (ink_left, ink_right) = ((left + first) as f32, (left + last + 1) as f32);
        let extent = (ink_right - ink_left).max(1.0);
        // Word gaps: empty runs wide enough not to be the space between two letters.
        let gap_min = (g * 0.3).max(2.0) as usize;
        let mut gaps = Vec::new();
        let mut x = first;
        while x <= last {
            if counts[x] == 0 {
                let start = x;
                while x <= last && counts[x] == 0 {
                    x += 1;
                }
                if x - start >= gap_min {
                    gaps.push([
                        ((left + start) as f32 - ink_left) / extent,
                        ((left + x) as f32 - ink_left) / extent,
                    ]);
                }
            } else {
                x += 1;
            }
        }
        // Heights, from the bands: the typical x-height, the taller ascent, the deeper descent.
        let mut x_heights: Vec<f32> = bands.iter().map(|b| b.baseline - b.x_top).collect();
        x_heights.sort_by(f32::total_cmp);
        let x_height = x_heights[x_heights.len() / 2].max(1.0);
        let mut ascents: Vec<f32> = bands.iter().map(|b| b.baseline - b.top).collect();
        ascents.sort_by(f32::total_cmp);
        let ascent =
            ascents[(ascents.len() * 3 / 4).min(ascents.len() - 1)].clamp(x_height, x_height * 2.2);
        let mut descents: Vec<f32> = bands.iter().map(|b| b.bottom - b.baseline).collect();
        descents.sort_by(f32::total_cmp);
        let descent = descents[(descents.len() * 3 / 4).min(descents.len() - 1)]
            .clamp(x_height * 0.1, x_height);
        // The baseline, sampled along the ink in windows two letters wide, so a bend inside a
        // strip is followed and the ends are where the ink ends.
        let baseline = trace_baseline(
            mask,
            bands,
            &strip_bounds,
            &shear,
            (left + first, left + last + 1),
            g,
        );
        out.push(PixelLine {
            column: column_index,
            baseline,
            x_height,
            ascent,
            descent,
            gaps,
            left: ink_left,
            right: ink_right,
            gaps_are_arc: false,
        });
    }
    out.sort_by(|a, b| {
        let ya = a.baseline[0][1] - (a.baseline[0][0] - center_x) * slope;
        let yb = b.baseline[0][1] - (b.baseline[0][0] - center_x) * slope;
        ya.total_cmp(&yb)
    });
    out
}

/// The box of a column's ink.
pub fn column_box(lines: &[PixelLine], column: (u32, u32)) -> PixelColumn {
    let mut top = f32::MAX;
    let mut bottom = f32::MIN;
    for line in lines {
        for point in &line.baseline {
            top = top.min(point[1] - line.ascent);
            bottom = bottom.max(point[1] + line.descent);
        }
    }
    if top > bottom {
        top = 0.0;
        bottom = 0.0;
    }
    PixelColumn {
        left: column.0 as f32,
        top,
        right: column.1 as f32,
        bottom,
    }
}

/// The baseline of a line, sampled along its ink. In each window of about two letters, the
/// ink is projected onto the rows (sheared by the skew) and the baseline is the bottom of the
/// densest rows, as for a band; the rows looked at are those of the line's band in that strip,
/// with half a letter of margin. Windows with too little ink (a gap, a dot) give no sample. The
/// ends are extrapolated from the last two samples, within half a letter, and the polyline is
/// simplified so a straight line is two points.
fn trace_baseline(
    mask: &Bitmap,
    bands: &[Band],
    strip_bounds: &dyn Fn(usize) -> (usize, usize),
    shear: &dyn Fn(usize) -> f32,
    ink: (usize, usize),
    g: f32,
) -> Vec<[f32; 2]> {
    let width = mask.width as usize;
    let height = mask.height as usize;
    let (ink_left, ink_right) = ink;
    let extent = ink_right.saturating_sub(ink_left).max(1);
    let window = ((2.0 * g).round() as usize).max(6);
    let count = (extent as f32 / window as f32).round().max(1.0) as usize;
    let margin = g * 0.5;
    let smooth = ((g / 4.0) as usize).max(1) | 1;
    let min_ink = g * g * 0.08;
    let band_at = |x: usize| -> &Band {
        // The band of the strip the window is in, or the nearest one.
        bands
            .iter()
            .min_by_key(|b| {
                let (x0, x1) = strip_bounds(b.strip);
                if x >= x0 && x < x1 {
                    0
                } else if x < x0 {
                    x0 - x
                } else {
                    x - x1 + 1
                }
            })
            .expect("a line has a band")
    };
    let mut samples: Vec<[f32; 2]> = Vec::with_capacity(count + 2);
    let mut profile: Vec<u32> = Vec::new();
    for i in 0..count {
        let x0 = ink_left + i * extent / count;
        let x1 = (ink_left + (i + 1) * extent / count).min(width);
        if x1 <= x0 {
            continue;
        }
        let center = (x0 + x1) as f32 / 2.0;
        let band = band_at(x0 + (x1 - x0) / 2);
        // Rows in sheared coordinates, a margin around the band.
        let r0 = (band.top - margin).floor() as isize;
        let r1 = (band.bottom + margin).ceil() as isize;
        if r1 <= r0 {
            continue;
        }
        let rows = (r1 - r0) as usize;
        profile.clear();
        profile.resize(rows, 0);
        let mut ink_total = 0u32;
        for x in x0..x1 {
            let offset = shear(x);
            for (r, cell) in profile.iter_mut().enumerate() {
                let y = (r0 as f32 + r as f32 + offset).round() as isize;
                if y < 0 || y >= height as isize {
                    continue;
                }
                let bit = u32::from(mask.bits[y as usize * width + x]);
                *cell += bit;
                ink_total += bit;
            }
        }
        if (ink_total as f32) < min_ink {
            continue;
        }
        let smoothed = box_smooth(&profile, smooth);
        let peak = smoothed.iter().copied().fold(0.0f32, f32::max);
        if peak <= 0.0 {
            continue;
        }
        let dense = peak * 0.5;
        let bottom = (0..rows)
            .rev()
            .find(|&r| smoothed[r] >= dense)
            .unwrap_or(rows - 1)
            + 1;
        let sheared = r0 as f32 + bottom as f32;
        samples.push([center, sheared + shear(center.round() as usize)]);
    }
    if samples.is_empty() {
        // No window had ink enough: the bands' own baselines.
        let at = |x: f32, band: &Band| [x, band.baseline + shear(x.round() as usize)];
        return vec![
            at(ink_left as f32, &bands[0]),
            at(ink_right as f32, &bands[bands.len() - 1]),
        ];
    }
    // A median of three takes out a sample thrown off by a descender or a speck. At the ends there
    // is no third: an end that strays from where its two neighbours lead by more than two fifths of
    // a letter is put back on that path, so the g that starts a line does not pull its start down,
    // while a line that curves toward the spine keeps its curve.
    if samples.len() >= 3 {
        let ys: Vec<f32> = samples.iter().map(|p| p[1]).collect();
        for i in 1..samples.len() - 1 {
            let mut three = [ys[i - 1], ys[i], ys[i + 1]];
            three.sort_by(f32::total_cmp);
            samples[i][1] = three[1];
        }
        let n = samples.len();
        let along = |a: [f32; 2], b: [f32; 2], x: f32| {
            let dx = b[0] - a[0];
            if dx.abs() > 0.5 {
                b[1] + (b[1] - a[1]) / dx * (x - b[0])
            } else {
                b[1]
            }
        };
        let first = along(samples[2], samples[1], samples[0][0]);
        if (samples[0][1] - first).abs() > g * 0.4 {
            samples[0][1] = first;
        }
        let last = along(samples[n - 3], samples[n - 2], samples[n - 1][0]);
        if (samples[n - 1][1] - last).abs() > g * 0.4 {
            samples[n - 1][1] = last;
        }
    }
    // No step bends the line by more than MAX_BEND_DEGREES from the way it was going, walked from
    // its middle out both ways: a sample that would is put back on that way. A line that curls
    // toward the spine bends a few degrees a step; one whose foot was taken along an underline, or
    // down a scribble onto the line below, jumps.
    if samples.len() >= 3 {
        let bend = MAX_BEND_DEGREES.to_radians().tan();
        let slope = |a: [f32; 2], b: [f32; 2]| {
            let dx = b[0] - a[0];
            if dx.abs() > 0.5 {
                (b[1] - a[1]) / dx
            } else {
                0.0
            }
        };
        let mut slopes: Vec<f32> = samples.windows(2).map(|w| slope(w[0], w[1])).collect();
        slopes.sort_by(f32::total_cmp);
        let middle_slope = slopes[slopes.len() / 2];
        let n = samples.len();
        let middle = n / 2;
        for outward in [true, false] {
            // The way the line goes: the median of its last three steps taken, starting from its
            // own median slope, so one step that a formula's exponent or a speck throws off does
            // not turn it, and a curl toward the spine does after two steps.
            let mut recent = vec![middle_slope, middle_slope];
            let steps: Vec<usize> = if outward {
                (middle + 1..n).collect()
            } else {
                (0..middle).rev().collect()
            };
            for i in steps {
                let mut sorted = recent[recent.len().saturating_sub(3)..].to_vec();
                sorted.sort_by(f32::total_cmp);
                let trend = sorted[sorted.len() / 2];
                let from = if outward {
                    samples[i - 1]
                } else {
                    samples[i + 1]
                };
                let dx = samples[i][0] - from[0];
                let expected = from[1] + trend * dx;
                if (samples[i][1] - expected).abs() > dx.abs() * bend + g * BEND_SLACK {
                    samples[i][1] = expected;
                } else {
                    recent.push(slope(from, samples[i]));
                }
            }
        }
    }
    // The ends, following the slope of the last two samples, but not by more than half a letter.
    let extend = |from: [f32; 2], toward: [f32; 2], x: f32| -> [f32; 2] {
        let dx = toward[0] - from[0];
        let slope = if dx.abs() > 0.5 {
            (toward[1] - from[1]) / dx
        } else {
            0.0
        };
        let dy = (slope * (x - toward[0])).clamp(-margin, margin);
        [x, toward[1] + dy]
    };
    let n = samples.len();
    let first = if n >= 2 {
        extend(samples[1], samples[0], ink_left as f32)
    } else {
        [ink_left as f32, samples[0][1]]
    };
    let last = if n >= 2 {
        extend(samples[n - 2], samples[n - 1], ink_right as f32)
    } else {
        [ink_right as f32, samples[0][1]]
    };
    let mut points = Vec::with_capacity(n + 2);
    points.push(first);
    points.extend(
        samples
            .into_iter()
            .filter(|p| p[0] > first[0] + 0.5 && p[0] < last[0] - 0.5),
    );
    points.push(last);
    simplify(&points, (g * 0.15).max(0.75))
}

/// The most a line's baseline bends from one sample to the next, two letters apart, and how far
/// beyond that a sample may stray, in letters: what a line jumps when its foot is taken along an
/// underline or down a scribble onto the next line is more than half a letter in one step; a
/// formula's exponents and fractions move its samples less.
const MAX_BEND_DEGREES: f32 = 12.0;
const BEND_SLACK: f32 = 0.5;

/// Douglas-Peucker: the points of a polyline that matter, within `epsilon` pixels.
pub fn simplify(points: &[[f32; 2]], epsilon: f32) -> Vec<[f32; 2]> {
    if points.len() <= 2 {
        return points.to_vec();
    }
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    let mut stack = vec![(0usize, points.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (pa, pb) = (points[a], points[b]);
        let (dx, dy) = (pb[0] - pa[0], pb[1] - pa[1]);
        let length = (dx * dx + dy * dy).sqrt().max(1e-6);
        let mut worst = (a, 0.0f32);
        for (i, p) in points.iter().enumerate().take(b).skip(a + 1) {
            let distance = ((p[0] - pa[0]) * dy - (p[1] - pa[1]) * dx).abs() / length;
            if distance > worst.1 {
                worst = (i, distance);
            }
        }
        if worst.1 > epsilon {
            keep[worst.0] = true;
            stack.push((a, worst.0));
            stack.push((worst.0, b));
        }
    }
    points
        .iter()
        .zip(keep)
        .filter(|(_, k)| *k)
        .map(|(p, _)| *p)
        .collect()
}

fn box_smooth(values: &[u32], window: usize) -> Vec<f32> {
    let half = window / 2;
    let n = values.len();
    let mut out = vec![0.0f32; n];
    if n == 0 {
        return out;
    }
    let mut sum = 0u32;
    let mut count = 0usize;
    // A running window, clipped at the ends.
    for &value in &values[..=half.min(n - 1)] {
        sum += value;
        count += 1;
    }
    for i in 0..n {
        out[i] = sum as f32 / count.max(1) as f32;
        if i + half + 1 < n {
            sum += values[i + half + 1];
            count += 1;
        }
        if i >= half {
            sum -= values[i - half];
            count -= 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lines of solid "words" on a mask, with a bend near the right edge.
    fn page() -> (Bitmap, Context) {
        let (w, h) = (600u32, 400u32);
        let mut mask = Bitmap::blank(w, h);
        let g = 10.0f32;
        for line in 0..8 {
            let base = 60.0 + line as f32 * 30.0;
            let mut x = 50;
            while x < 540 {
                let word = 20 + (x * 7 % 30);
                for column in x..(x + word).min(540) {
                    let drop = if column > 450 {
                        ((column - 450) as f32 / 90.0).powi(2) * 18.0
                    } else {
                        0.0
                    };
                    let baseline = base + drop;
                    for y in (baseline - g) as usize..baseline as usize {
                        mask.bits[y * w as usize + column] = 1;
                    }
                }
                x += word + 6;
            }
        }
        (
            mask,
            Context {
                glyph_height: g,
                skew: 0.0,
                width: w,
                height: h,
            },
        )
    }

    #[test]
    fn lines_follow_the_bend_and_know_their_words() {
        let (mask, context) = page();
        let lines = find_lines(&mask, (40, 560), 0, &context);
        assert_eq!(lines.len(), 8, "{lines:#?}");
        let third = &lines[2];
        assert!(
            (third.left - 50.0).abs() <= 1.0 && (third.right - 540.0).abs() <= 1.0,
            "{:?}",
            (third.left, third.right)
        );
        assert!((third.x_height - 10.0).abs() <= 2.0, "{}", third.x_height);
        // At the start the baseline is level; at the end it has dropped by about 18 px.
        let first = third.baseline[0];
        let last = third.baseline[third.baseline.len() - 1];
        assert!((first[1] - 120.0).abs() <= 2.0, "{first:?}");
        assert!((last[1] - 138.0).abs() <= 4.0, "{last:?}");
        assert!(third.gaps.len() >= 8, "{:?}", third.gaps);
        // Reading order: top to bottom.
        assert!(
            lines
                .windows(2)
                .all(|w| w[0].baseline[0][1] < w[1].baseline[0][1])
        );
    }

    #[test]
    fn two_columns_are_told_apart_by_their_gutter() {
        let glyph = |x: u32, y: u32| Component {
            left: x,
            top: y,
            right: x + 8,
            bottom: y + 12,
            area: 60,
        };
        let mut glyphs = Vec::new();
        for row in 0..30 {
            for word in 0..12 {
                glyphs.push(glyph(60 + word * 20, 50 + row * 24));
                glyphs.push(glyph(400 + word * 20, 50 + row * 24));
            }
        }
        // A heading across both columns does not close the gutter.
        for word in 0..25 {
            glyphs.push(Component {
                left: 60 + word * 24,
                top: 10,
                right: 70 + word * 24,
                bottom: 30,
                area: 150,
            });
        }
        let context = Context {
            glyph_height: 12.0,
            skew: 0.0,
            width: 700,
            height: 800,
        };
        let columns = find_columns(&glyphs, &context);
        assert_eq!(columns.len(), 2, "{columns:?}");
        assert!(columns[0].1 > 300 && columns[0].1 < 400, "{columns:?}");
        let one = find_columns(
            &glyphs[..30 * 24]
                .iter()
                .step_by(2)
                .copied()
                .collect::<Vec<_>>(),
            &context,
        );
        assert_eq!(one.len(), 1);
    }

    #[test]
    fn a_scribble_down_the_margin_does_not_pull_a_line_onto_the_next() {
        let (w, h) = (600u32, 200u32);
        let mut mask = Bitmap::blank(w, h);
        let g = 10.0f32;
        for line in 0..3 {
            let base = 60 + line * 30;
            let mut x = 50;
            while x < 540 {
                let word = 20 + (x * 7 % 30);
                for column in x..(x + word).min(540) {
                    for y in base - 10..base {
                        mask.bits[y * w as usize + column] = 1;
                    }
                }
                x += word + 6;
            }
        }
        // A pencil stroke down the right of the column, across all three lines and the gaps
        // between them, thick enough to run their last strips together.
        for y in 40..135 {
            for x in 520..528 {
                mask.bits[y * w as usize + x] = 1;
            }
        }
        let context = Context {
            glyph_height: g,
            skew: 0.0,
            width: w,
            height: h,
        };
        let lines = find_lines(&mask, (40, 560), 0, &context);
        for (index, base) in [60.0f32, 90.0, 120.0].into_iter().enumerate() {
            let line = lines
                .iter()
                .find(|line| line.left < 60.0 && (line.baseline[0][1] - base).abs() < 5.0)
                .unwrap_or_else(|| panic!("line {index}: {lines:#?}"));
            for point in &line.baseline {
                assert!(
                    (point[1] - base).abs() <= 4.0,
                    "line {index}: {:?}",
                    line.baseline
                );
            }
        }
    }
}

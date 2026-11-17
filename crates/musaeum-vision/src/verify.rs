//! Whether a line the strips found is a line of text. The strips see ink in rows; text is more than
//! that: letters of about one height, most of them standing on the baseline, and not in a picture.
//! A row of halftone dots, the bars of a chart, the edge of a photograph and the shading of a fold
//! fail one of these; a line of text, even a single word or a number under a chart, does not.

use crate::components::Component;
use crate::lines::PixelLine;
use crate::near::center;
use crate::orient::baseline_at;

/// Why a line was kept or left out, for reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Text,
    /// No letter inside it.
    Empty,
    /// Its blobs are not of a letter's height.
    WrongHeight,
    /// Too few of them stand on its baseline.
    NotAligned,
    /// Most of it is in a picture.
    InPicture,
    /// A blob or two that are not shaped as letters.
    NotLetters,
}

/// The fewest letters a line has for its low marks (a leader's dots and dashes, punctuation) to be
/// left out of how its letters are judged.
const LETTERS_BESIDE_MARKS: usize = 3;

/// Judges `line`, whose letters may be any of `glyphs` (in the same frame as the line); `in_picture`
/// says whether a point of that frame is in a picture.
pub fn judge(
    line: &PixelLine,
    glyphs: &[Component],
    in_picture: &dyn Fn(f32, f32) -> bool,
) -> Verdict {
    let xh = line.x_height.max(2.0);
    let reach = xh * 0.5;
    let members: Vec<&Component> = glyphs
        .iter()
        .filter(|c| {
            let [cx, cy] = center(c);
            if cx < line.left - reach || cx > line.right + reach {
                return false;
            }
            let base = baseline_at(&line.baseline, cx);
            cy >= base - line.ascent - reach * 0.5 && cy <= base + line.descent + reach * 0.5
        })
        .collect();
    if members.is_empty() {
        return Verdict::Empty;
    }
    // Most of the line in a picture: sampled along it, at the middle of its small letters.
    let samples = 9;
    let inside = (0..samples)
        .filter(|&step| {
            let x = line.left + (line.right - line.left) * (step as f32 + 0.5) / samples as f32;
            in_picture(x, baseline_at(&line.baseline, x) - xh * 0.5)
        })
        .count();
    if inside * 2 > samples {
        return Verdict::InPicture;
    }
    // The leader of a table of contents (dots, dashes, an underscore between a title and its page)
    // and a line's punctuation are marks low on the line, not letters: where the line has letters
    // enough, its letters are judged without them, so a long leader does not make a line of
    // capitals look too short. A row of a screen's dots, or of specks, has no letters to judge.
    let low_mark = |c: &&Component| -> bool {
        let [cx, cy] = center(c);
        let base = baseline_at(&line.baseline, cx);
        (c.height() as f32) < xh * 0.45 && cy >= base - xh * 0.75 && cy <= base + line.descent
    };
    let letters: Vec<&Component> = members.iter().copied().filter(|c| !low_mark(c)).collect();
    let members = if letters.len() >= LETTERS_BESIDE_MARKS {
        letters
    } else {
        members
    };
    let mut heights: Vec<f32> = members.iter().map(|c| c.height() as f32).collect();
    heights.sort_by(f32::total_cmp);
    let median = heights[heights.len() / 2];
    if median < xh * 0.45 || median > xh * 3.2 {
        return Verdict::WrongHeight;
    }
    // A letter or a word stands in the band of the small letters: it covers most of it, and reaches
    // no further than the line's ascent and descent. A dot, an accent, a speck of a photograph or a
    // blob of a drawing does not.
    let tolerance = (xh * 0.35).max(1.5);
    let aligned = members
        .iter()
        .filter(|c| {
            let [cx, _] = center(c);
            let base = baseline_at(&line.baseline, cx);
            let band = (base - xh, base);
            let covered = (c.bottom as f32).min(band.1) - (c.top as f32).max(band.0);
            covered >= xh * 0.7
                && c.bottom as f32 <= base + line.descent + tolerance
                && c.top as f32 >= base - line.ascent - tolerance
        })
        .count();
    if members.len() <= 2 {
        // A number under a chart, a short word: every blob a letter, on the line.
        let letters = members.iter().all(|c| {
            let (w, h) = (c.width() as f32, c.height() as f32);
            let fill = c.area as f32 / (w * h).max(1.0);
            (0.08..=0.85).contains(&fill) && w <= h * 6.0
        });
        if !letters {
            return Verdict::NotLetters;
        }
        if aligned == 0 {
            return Verdict::NotAligned;
        }
        return Verdict::Text;
    }
    if (aligned as f32) < members.len() as f32 * 0.4 {
        return Verdict::NotAligned;
    }
    Verdict::Text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(y: f32) -> PixelLine {
        PixelLine {
            column: 0,
            baseline: vec![[100.0, y], [400.0, y]],
            x_height: 10.0,
            ascent: 14.0,
            descent: 4.0,
            gaps: Vec::new(),
            left: 100.0,
            right: 400.0,
            gaps_are_arc: false,
        }
    }

    fn blob(x: u32, top: u32, w: u32, h: u32, area: u32) -> Component {
        Component {
            left: x,
            top,
            right: x + w,
            bottom: top + h,
            area,
        }
    }

    #[test]
    fn letters_on_a_baseline_are_text_and_texture_is_not() {
        let nowhere = |_: f32, _: f32| false;
        let letters: Vec<Component> = (0..20).map(|i| blob(100 + i * 15, 90, 8, 10, 40)).collect();
        assert_eq!(judge(&line(100.0), &letters, &nowhere), Verdict::Text);
        // Dots of a screen, far smaller than the band's letters would be.
        let dots: Vec<Component> = (0..20).map(|i| blob(100 + i * 15, 94, 4, 4, 12)).collect();
        assert_eq!(judge(&line(100.0), &dots, &nowhere), Verdict::WrongHeight);
        // Blobs of a letter's height hanging above the baseline: a row of a drawing.
        let hanging: Vec<Component> = (0..20).map(|i| blob(100 + i * 15, 82, 8, 10, 40)).collect();
        assert_eq!(judge(&line(100.0), &hanging, &nowhere), Verdict::NotAligned);
        // The same letters inside a photograph.
        let everywhere = |_: f32, _: f32| true;
        assert_eq!(
            judge(&line(100.0), &letters, &everywhere),
            Verdict::InPicture
        );
        // A solid square marker alone is no word; one digit is.
        assert_eq!(
            judge(&line(100.0), &[blob(200, 90, 10, 10, 100)], &nowhere),
            Verdict::NotLetters
        );
        assert_eq!(
            judge(&line(100.0), &[blob(200, 90, 6, 10, 30)], &nowhere),
            Verdict::Text
        );
        assert_eq!(judge(&line(100.0), &[], &nowhere), Verdict::Empty);
    }

    #[test]
    fn a_leader_of_dashes_does_not_make_a_line_of_capitals_too_short() {
        let nowhere = |_: f32, _: f32| false;
        // ENEMY USE, capitals as tall as the line's ascent, then a leader of twelve dashes on the
        // baseline and a page number: more dashes than letters.
        let mut blobs: Vec<Component> =
            (0..8).map(|i| blob(100 + i * 14, 86, 10, 14, 70)).collect();
        blobs.extend((0..12).map(|i| blob(230 + i * 10, 97, 7, 2, 14)));
        blobs.extend((0..2).map(|i| blob(370 + i * 12, 86, 8, 14, 60)));
        assert_eq!(judge(&line(100.0), &blobs, &nowhere), Verdict::Text);
        // Dashes alone, or beside a letter or two, are still no line.
        let dashes: Vec<Component> = (0..12).map(|i| blob(230 + i * 10, 97, 7, 2, 14)).collect();
        assert_ne!(judge(&line(100.0), &dashes, &nowhere), Verdict::Text);
        let mut few = dashes.clone();
        few.push(blob(370, 86, 8, 14, 60));
        assert_ne!(judge(&line(100.0), &few, &nowhere), Verdict::Text);
    }
}

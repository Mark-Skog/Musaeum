//! Connected components of the ink: every blob with its box and its size, which is what tells a
//! letter from a speck, a picture, a rule or the dark edge of a scan.

use crate::sauvola::Bitmap;

/// One blob of ink.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Component {
    pub left: u32,
    pub top: u32,
    /// Exclusive.
    pub right: u32,
    /// Exclusive.
    pub bottom: u32,
    pub area: u32,
}

impl Component {
    pub fn width(&self) -> u32 {
        self.right - self.left
    }

    pub fn height(&self) -> u32 {
        self.bottom - self.top
    }
}

/// The components of a bitmap, and the label of every pixel (0 for paper, else the component's
/// index plus one).
pub struct Labeled {
    pub width: u32,
    pub height: u32,
    pub labels: Vec<u32>,
    pub components: Vec<Component>,
}

struct UnionFind {
    parent: Vec<u32>,
}

impl UnionFind {
    fn make(&mut self) -> u32 {
        let id = self.parent.len() as u32;
        self.parent.push(id);
        id
    }

    fn find(&mut self, mut id: u32) -> u32 {
        while self.parent[id as usize] != id {
            let up = self.parent[id as usize];
            self.parent[id as usize] = self.parent[up as usize];
            id = up;
        }
        id
    }

    fn union(&mut self, a: u32, b: u32) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            let (low, high) = if ra < rb { (ra, rb) } else { (rb, ra) };
            self.parent[high as usize] = low;
        }
    }
}

/// Labels the ink with 8-connectivity, in two passes.
pub fn label(bitmap: &Bitmap) -> Labeled {
    let width = bitmap.width as usize;
    let height = bitmap.height as usize;
    let mut labels = vec![0u32; width * height];
    let mut sets = UnionFind { parent: Vec::new() };
    sets.make(); // 0 is paper.
    for y in 0..height {
        for x in 0..width {
            if !bitmap.is_ink(x, y) {
                continue;
            }
            let at = y * width + x;
            let mut best = 0u32;
            let mut consider = |label: u32, sets: &mut UnionFind| {
                if label != 0 {
                    if best == 0 {
                        best = label;
                    } else if label != best {
                        sets.union(best, label);
                    }
                }
            };
            if x > 0 {
                consider(labels[at - 1], &mut sets);
            }
            if y > 0 {
                consider(labels[at - width], &mut sets);
                if x > 0 {
                    consider(labels[at - width - 1], &mut sets);
                }
                if x + 1 < width {
                    consider(labels[at - width + 1], &mut sets);
                }
            }
            labels[at] = if best == 0 { sets.make() } else { best };
        }
    }
    // Second pass: every pixel gets its set's root, roots get dense indices, boxes grow.
    let mut dense = vec![0u32; sets.parent.len()];
    let mut components: Vec<Component> = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let at = y * width + x;
            if labels[at] == 0 {
                continue;
            }
            let root = sets.find(labels[at]);
            let index = if dense[root as usize] == 0 {
                components.push(Component {
                    left: x as u32,
                    top: y as u32,
                    right: x as u32 + 1,
                    bottom: y as u32 + 1,
                    area: 0,
                });
                dense[root as usize] = components.len() as u32;
                components.len() as u32
            } else {
                dense[root as usize]
            };
            labels[at] = index;
            let component = &mut components[index as usize - 1];
            component.left = component.left.min(x as u32);
            component.right = component.right.max(x as u32 + 1);
            component.top = component.top.min(y as u32);
            component.bottom = component.bottom.max(y as u32 + 1);
            component.area += 1;
        }
    }
    Labeled {
        width: bitmap.width,
        height: bitmap.height,
        labels,
        components,
    }
}

/// The second moments of every component's ink: how many pixels, their sums of x, y, x², y² and
/// xy, from one pass over the labels.
pub fn moments(labels: &[u32], width: u32, count: usize) -> Vec<[f64; 6]> {
    let mut out = vec![[0.0f64; 6]; count];
    for (y, row) in labels.chunks_exact(width.max(1) as usize).enumerate() {
        let y = y as f64;
        for (x, &label) in row.iter().enumerate() {
            if label == 0 {
                continue;
            }
            let x = x as f64;
            let m = &mut out[label as usize - 1];
            m[0] += 1.0;
            m[1] += x;
            m[2] += y;
            m[3] += x * x;
            m[4] += y * y;
            m[5] += x * y;
        }
    }
    out
}

/// The way a blob's ink runs, from its moments: the direction of its long axis in degrees folded
/// into [0, 180), how many times longer than thick it is, and its thickness in pixels (about the
/// height of a band of that thickness).
pub fn principal_axis(m: &[f64; 6]) -> Option<(f32, f32, f32)> {
    let n = m[0];
    if n < 4.0 {
        return None;
    }
    let (mx, my) = (m[1] / n, m[2] / n);
    let sxx = m[3] / n - mx * mx;
    let syy = m[4] / n - my * my;
    let sxy = m[5] / n - mx * my;
    let half = (sxx + syy) / 2.0;
    let spread = (((sxx - syy) / 2.0).powi(2) + sxy * sxy).sqrt();
    let (major, minor) = (half + spread, (half - spread).max(1e-6));
    let angle = (0.5 * (2.0 * sxy).atan2(sxx - syy))
        .to_degrees()
        .rem_euclid(180.0);
    Some((
        angle as f32,
        (major / minor).sqrt() as f32,
        (minor.sqrt() * 12f64.sqrt()) as f32,
    ))
}

/// What a component looks like, by its shape and place on the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// Part of the text: a letter, a word of touching letters, a mark.
    Glyph,
    /// Too small to be anything but dust.
    Speck,
    /// A picture, a filled box, a border: far too big for text.
    Figure,
    /// A long thin horizontal thing: a rule, an underline of a whole width.
    Rule,
    /// A long thin vertical thing: a frame, a fold, the edge of a scan.
    Edge,
}

/// Decides what each component is. `glyph_height` is the typical height of a letter, found by
/// [`typical_glyph_height`], or zero before it is known.
pub fn classify(
    component: &Component,
    page_width: u32,
    page_height: u32,
    glyph_height: f32,
) -> Shape {
    let w = component.width() as f32;
    let h = component.height() as f32;
    let pw = page_width as f32;
    let ph = page_height as f32;
    if component.area < 4 || h < 2.0 {
        return Shape::Speck;
    }
    if h < ph * 0.012 && w > pw * 0.22 && h < w * 0.08 {
        return Shape::Rule;
    }
    if w < pw * 0.012 && h > ph * 0.12 && w < h * 0.08 {
        return Shape::Edge;
    }
    let touches_edge = component.left == 0
        || component.top == 0
        || component.right == page_width
        || component.bottom == page_height;
    if touches_edge && (w > pw * 0.3 || h > ph * 0.3) {
        return Shape::Edge;
    }
    if (h > ph * 0.08 && w > pw * 0.25) || (h > ph * 0.25) || (w > pw * 0.6) {
        return Shape::Figure;
    }
    if glyph_height > 0.0 {
        let g = glyph_height;
        let fill = component.area as f32 / (w * h).max(1.0);
        if h < g * 0.15 && w < g * 0.15 {
            return Shape::Speck;
        }
        // Measured against a letter, not the page: the axis of a chart, a rule under a heading,
        // the line beside a photograph.
        if w >= g * 6.0 && h <= g * 0.35 {
            return Shape::Rule;
        }
        if h >= g * 3.0 && w <= g * 0.35 {
            return Shape::Edge;
        }
        // Solid ink bigger than a letter: a bar of a chart, a filled box. Letters, even bold, leave
        // a third of their box empty, and the solid ones (l, I, |) are thin.
        if fill > 0.78 && w > g * 1.1 && h > g * 1.1 {
            return Shape::Figure;
        }
        // Lines around nothing: the frame of a picture, the grid of a table.
        if w > g * 4.0 && h > g * 2.0 && fill < 0.12 {
            return Shape::Figure;
        }
        if h > g * 3.5 && w > g * 3.5 {
            return Shape::Figure;
        }
    }
    Shape::Glyph
}

/// Which components sit as letters do: beside a neighbour of about their size on the same
/// baseline, and with nothing of their size just above or below them. Halftone dots and the grain
/// of a photograph have neighbours on every side; a letter has its line's neighbours only.
/// `candidates` are the components to judge; the others are never neighbours.
pub fn text_like(
    components: &[Component],
    candidates: &[usize],
    width: u32,
    height: u32,
) -> Vec<bool> {
    let mut out = vec![false; components.len()];
    if candidates.is_empty() {
        return out;
    }
    let mut sizes: Vec<f32> = candidates
        .iter()
        .map(|&i| components[i].height() as f32)
        .collect();
    sizes.sort_by(f32::total_cmp);
    let typical = sizes[sizes.len() / 2].max(4.0);
    let grid = crate::near::Grid::new(
        components,
        candidates.iter().copied(),
        typical * 2.0,
        width,
        height,
    );
    for &index in candidates {
        let c = &components[index];
        let h = c.height() as f32;
        let [cx, cy] = crate::near::center(c);
        let mut aligned = false;
        let mut stacked = false;
        grid.around(cx, cy, h * 3.0 + typical, |other| {
            if other == index || stacked {
                return;
            }
            let o = &components[other];
            let oh = o.height() as f32;
            let (gx, gy) = crate::near::gaps(c, o);
            let low = h.min(oh);
            // Beside it, on the same line: most of the shorter one level with the other, which a
            // word with a descender beside one with an ascender still is.
            let overlap = (c.bottom.min(o.bottom) as f32 - c.top.max(o.top) as f32).max(0.0);
            if (0.4..=2.5).contains(&(oh / h))
                && overlap >= low * 0.5
                && gx <= h.max(oh) * 1.2
                && gx > 0.0
            {
                aligned = true;
            }
            // Just above or below it, about its size, and not on its line.
            let [_, oy] = crate::near::center(o);
            if (0.5..=2.0).contains(&(oh / h))
                && (oy - cy).abs() > low * 0.5
                && gy <= h * 0.25
                && gx <= h * 0.25
            {
                stacked = true;
            }
        });
        out[index] = aligned && !stacked;
    }
    out
}

/// Which components are dots of a screen: close to several others of their size in two directions
/// at once, as a halftone's lattice is (along its rows and along its columns, or both diagonals). A
/// letter has close neighbours along its line only.
pub fn lattice(
    components: &[Component],
    candidates: &[usize],
    width: u32,
    height: u32,
) -> Vec<bool> {
    let mut out = vec![false; components.len()];
    if candidates.is_empty() {
        return out;
    }
    let mut sizes: Vec<f32> = candidates
        .iter()
        .map(|&i| crate::near::size(&components[i]))
        .collect();
    sizes.sort_by(f32::total_cmp);
    let typical = sizes[sizes.len() / 2].max(4.0);
    let grid = crate::near::Grid::new(
        components,
        candidates.iter().copied(),
        typical * 2.0,
        width,
        height,
    );
    // Dots are round or square; a word of letters run together is long and is no dot.
    let roundish = |c: &Component| {
        let (w, h) = (c.width() as f32, c.height() as f32);
        w.max(h) <= w.min(h) * 1.8
    };
    for &index in candidates {
        let c = &components[index];
        if !roundish(c) {
            continue;
        }
        let own = crate::near::size(c);
        let [cx, cy] = crate::near::center(c);
        // Close neighbours of about this size, by direction in four bins folded into half a turn.
        let mut bins = [0u32; 4];
        grid.around(cx, cy, own * 2.5, |other| {
            if other == index {
                return;
            }
            let o = &components[other];
            if !roundish(o) || !(0.6..=1.6).contains(&(crate::near::size(o) / own)) {
                return;
            }
            let (gx, gy) = crate::near::gaps(c, o);
            if gx.hypot(gy) > own * 0.6 {
                return;
            }
            let [ox, oy] = crate::near::center(o);
            let angle = (oy - cy).atan2(ox - cx).to_degrees().rem_euclid(180.0);
            bins[(((angle + 22.5) / 45.0) as usize) % 4] += 1;
        });
        // Two neighbours along each of two directions.
        out[index] = bins.iter().filter(|&&count| count >= 2).count() >= 2;
    }
    out
}

/// The typical height of a letter, from the components that sit as letters do ([`text_like`]), so
/// that the dots of a halftone photograph, however many, do not decide it. Falls back to
/// [`typical_glyph_height`] when too few components look like text.
pub fn robust_glyph_height(components: &[Component], page_width: u32, page_height: u32) -> f32 {
    let candidates: Vec<usize> = components
        .iter()
        .enumerate()
        .filter(|(_, c)| classify(c, page_width, page_height, 0.0) == Shape::Glyph)
        .map(|(index, _)| index)
        .collect();
    let like = text_like(components, &candidates, page_width, page_height);
    let mut heights: Vec<u32> = candidates
        .iter()
        .filter(|&&index| like[index])
        .map(|&index| components[index].height())
        .collect();
    if heights.len() < 20 {
        return typical_glyph_height(components, page_width, page_height);
    }
    heights.sort_unstable();
    let median = heights[heights.len() / 2] as f32;
    let mut near: Vec<u32> = heights
        .iter()
        .copied()
        .filter(|&h| (h as f32) >= median * 0.5 && (h as f32) <= median * 2.5)
        .collect();
    near.sort_unstable();
    near.get(near.len() / 2).map_or(median, |&h| h as f32)
}

/// The typical height of a letter: the median height of the components that are not plainly
/// something else, refined once around itself.
pub fn typical_glyph_height(components: &[Component], page_width: u32, page_height: u32) -> f32 {
    let mut heights: Vec<u32> = components
        .iter()
        .filter(|c| classify(c, page_width, page_height, 0.0) == Shape::Glyph)
        .map(Component::height)
        .collect();
    if heights.is_empty() {
        return 0.0;
    }
    heights.sort_unstable();
    let median = heights[heights.len() / 2] as f32;
    // Punctuation and specks pull the median down, big letters up: take the median of what is
    // within a reasonable band of the first guess.
    let mut near: Vec<u32> = heights
        .iter()
        .copied()
        .filter(|&h| (h as f32) >= median * 0.5 && (h as f32) <= median * 2.5)
        .collect();
    if near.is_empty() {
        return median;
    }
    near.sort_unstable();
    near[near.len() / 2] as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blobs_are_counted_with_their_boxes_and_eight_neighbours() {
        let mut bitmap = Bitmap::blank(10, 6);
        // A 2 × 3 block, and a diagonal pixel touching its corner.
        for y in 1..4 {
            for x in 1..3 {
                bitmap.bits[y * 10 + x] = 1;
            }
        }
        bitmap.bits[4 * 10 + 3] = 1;
        // Another blob far away.
        bitmap.bits[9] = 1;
        let labeled = label(&bitmap);
        assert_eq!(labeled.components.len(), 2);
        let big = labeled.components.iter().find(|c| c.area == 7).unwrap();
        assert_eq!((big.left, big.top, big.right, big.bottom), (1, 1, 4, 5));
        assert_eq!(labeled.labels[4 * 10 + 3], labeled.labels[10 + 1]);
        assert_ne!(labeled.labels[9], labeled.labels[11]);
    }

    #[test]
    fn dots_of_a_screen_are_a_lattice_and_letters_are_not() {
        let box_at = |x: u32, y: u32, w: u32, h: u32| Component {
            left: x,
            top: y,
            right: x + w,
            bottom: y + h,
            area: w * h / 2,
        };
        let mut components = Vec::new();
        for letter in 0..20 {
            components.push(box_at(50 + letter * 9, 50, 7, 10));
        }
        let letters = components.len();
        // A 45° screen: dots on diagonals, 7 apart along each.
        for i in 0..12u32 {
            for j in 0..12u32 {
                components.push(box_at(300 + (i + j) * 5, 200 + (12 + i - j) * 5, 6, 6));
            }
        }
        let all: Vec<usize> = (0..components.len()).collect();
        let found = lattice(&components, &all, 600, 400);
        assert!(found[..letters].iter().all(|&l| !l));
        let dots = found[letters..].iter().filter(|&&l| l).count();
        // Every dot but those on the rim.
        assert!(dots >= 100, "{dots}");
    }

    #[test]
    fn a_word_at_45_degrees_knows_its_direction() {
        // A band 40 long and 6 thick, rising at 45° as seen (y down).
        let (w, h) = (60u32, 60u32);
        let mut labels = vec![0u32; (w * h) as usize];
        for step in 0..400 {
            let t = step as f32 * 0.1;
            for across in 0..12 {
                let a = across as f32 * 0.5 - 3.0;
                let r = std::f32::consts::FRAC_1_SQRT_2;
                let x = 10.0 + t * r + a * r;
                let y = 50.0 - t * r + a * r;
                labels[y as usize * w as usize + x as usize] = 1;
            }
        }
        let m = moments(&labels, w, 1);
        let (angle, elongation, thickness) = principal_axis(&m[0]).unwrap();
        assert!((angle - 135.0).abs() < 3.0, "{angle}");
        assert!(elongation > 4.0, "{elongation}");
        assert!((thickness - 6.0).abs() < 2.0, "{thickness}");
    }

    #[test]
    fn letters_sit_in_lines_and_dots_of_a_screen_do_not() {
        let box_at = |x: u32, y: u32, w: u32, h: u32| Component {
            left: x,
            top: y,
            right: x + w,
            bottom: y + h,
            area: w * h / 2,
        };
        let mut components = Vec::new();
        // Three lines of letters.
        for line in 0..3 {
            for letter in 0..20 {
                components.push(box_at(50 + letter * 9, 50 + line * 30, 7, 10));
            }
        }
        let letters = components.len();
        // A screen of dots, every 6 pixels, rows offset as a 45° screen's are.
        for row in 0..20 {
            for dot in 0..30 {
                components.push(box_at(300 + dot * 6 + (row % 2) * 3, 50 + row * 3, 3, 3));
            }
        }
        let all: Vec<usize> = (0..components.len()).collect();
        let like = text_like(&components, &all, 600, 400);
        let letters_like = like[..letters].iter().filter(|&&l| l).count();
        let dots_like = like[letters..].iter().filter(|&&l| l).count();
        assert!(letters_like >= letters - 2, "{letters_like}");
        assert_eq!(dots_like, 0);
        // Many more dots than letters, and still the letters' height.
        assert_eq!(robust_glyph_height(&components, 600, 400), 10.0);
    }

    #[test]
    fn shapes_by_size() {
        let page = (1000, 1400);
        let c = |l, t, w, h, area| Component {
            left: l,
            top: t,
            right: l + w,
            bottom: t + h,
            area,
        };
        assert_eq!(
            classify(&c(10, 10, 2, 1, 2), page.0, page.1, 12.0),
            Shape::Speck
        );
        assert_eq!(
            classify(&c(100, 100, 7, 12, 50), page.0, page.1, 12.0),
            Shape::Glyph
        );
        assert_eq!(
            classify(&c(100, 300, 600, 8, 4000), page.0, page.1, 12.0),
            Shape::Rule
        );
        assert_eq!(
            classify(&c(0, 0, 6, 1400, 8000), page.0, page.1, 12.0),
            Shape::Edge
        );
        assert_eq!(
            classify(&c(100, 400, 400, 300, 90000), page.0, page.1, 12.0),
            Shape::Figure
        );
        // Against a letter of 12: a chart's axis, a bar, the grid of a table, a line beside a figure.
        assert_eq!(
            classify(&c(100, 100, 120, 3, 360), page.0, page.1, 12.0),
            Shape::Rule
        );
        assert_eq!(
            classify(&c(100, 100, 20, 60, 1150), page.0, page.1, 12.0),
            Shape::Figure
        );
        assert_eq!(
            classify(&c(100, 100, 80, 40, 300), page.0, page.1, 12.0),
            Shape::Figure
        );
        assert_eq!(
            classify(&c(100, 100, 3, 50, 150), page.0, page.1, 12.0),
            Shape::Edge
        );
        // A bold heading's letter and a thin solid one stay letters.
        assert_eq!(
            classify(&c(100, 100, 20, 24, 300), page.0, page.1, 12.0),
            Shape::Glyph
        );
        assert_eq!(
            classify(&c(100, 100, 2, 16, 32), page.0, page.1, 12.0),
            Shape::Glyph
        );
        let heights = [
            c(0, 0, 5, 11, 30),
            c(0, 0, 5, 12, 30),
            c(0, 0, 5, 13, 30),
            c(0, 0, 3, 3, 9),
            c(0, 0, 400, 300, 9000),
        ];
        assert_eq!(typical_glyph_height(&heights, page.0, page.1), 12.0);
    }
}

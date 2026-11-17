//! Text that runs at other angles than the page's lines: a caption up the side of a photograph, the
//! labels under a chart's bars at 45°, a table's headings turned 90°, a block set at an angle.
//!
//! Letters of a line are each other's nearest neighbours, so the direction from a letter to its
//! nearest letter of about its size is the direction of its line (O'Gorman's docstrum). The
//! directions of the whole page, folded into half a turn, pile up at the page's own angle, level
//! within a few degrees of skew; any other pile is text at another angle, even a bigger one. Letters whose nearest neighbour lies along such a pile are linked
//! to each other and to their neighbours of the same angle, and every group of them is a block of
//! turned text: it is taken out of the page, turned level in a frame of its own, and its lines found
//! there as any page's are. Folding loses which way a line reads (90° or 270°); the letters tell:
//! more ink stands above the small letters than hangs below them, in the alphabets this reads.

use std::f32::consts::PI;

use crate::components::Component;
use crate::lines::PixelLine;
use crate::near::{Grid, center, gaps, size};
use crate::sauvola::Bitmap;

/// A block of text at an angle of its own.
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    /// The direction its lines run in, radians, in the picture (y down): 0 is left to right. Taken
    /// from the fold, so possibly half a turn from the way it reads.
    pub angle: f32,
    /// Its components, by index.
    pub members: Vec<usize>,
}

/// How far apart, in degrees, two piles of turned text are at least.
const APART_DEGREES: f32 = 12.0;
/// How far from level a pile has to be to be turned text: level text's own letters, a tall one
/// beside a short one, point up to a fifth of a turn off it, and the page's skew is a few degrees.
const OFF_LEVEL_DEGREES: f32 = 20.0;
/// How near a letter's direction has to be to a pile's to belong to it.
const MATCH_DEGREES: f32 = 9.0;
/// How many times a block's area grows with the letters it takes in at the ends of its lines.
const ABSORB_PASSES: usize = 3;
/// The fewest letters a block has.
const MIN_MEMBERS: usize = 3;

/// The distance between two angles folded into half a turn, in degrees.
fn folded_apart(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(180.0);
    d.min(180.0 - d)
}

/// What the directions between letters say of a page's text.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TurnedText {
    /// The page's own text may run up or down it, not across: a table or a figure set sideways,
    /// a page printed the long way. Turned a quarter, it may read better; or it may be a page of
    /// several blocks at 90°, some up and some down, beside level text, which reads best as it
    /// is.
    pub page_on_its_side: bool,
    /// The blocks at other angles than the page's.
    pub blocks: Vec<Block>,
}

/// How much more of the directions between letters lie within 15° of upright than within 15° of
/// level for a page's own text to maybe run up or down it. On the corpus and two books of the real
/// collection, pages of level text, turned labels and charts had at most 0.9 as much upright as
/// level; pages set sideways (a table, a figure and its caption, the last page of a manual turned
/// the long way) had 1.7 times as much or more, and so did a page of blocks at 90° beside level
/// text, which is not on its side: the detector analyzes such a page both ways (`detect`).
const ON_ITS_SIDE: f32 = 1.4;
/// The fewest directions upright for that: a page of a few words says nothing.
const ON_ITS_SIDE_MIN: f32 = 40.0;

/// The blocks of text at other angles than the page's, among the letters `glyphs`. `axes` are the
/// components' long axes from their moments (`components::principal_axis`), `glyph_height` the
/// typical letter.
pub fn turned_blocks(
    components: &[Component],
    glyphs: &[usize],
    axes: &[Option<(f32, f32, f32)>],
    glyph_height: f32,
    width: u32,
    height: u32,
) -> Vec<Block> {
    turned_text(components, glyphs, axes, glyph_height, width, height, false).blocks
}

/// [`turned_blocks`], and with `ask_side`, whether the page's own text may run up or down it.
pub fn turned_text(
    components: &[Component],
    glyphs: &[usize],
    axes: &[Option<(f32, f32, f32)>],
    glyph_height: f32,
    width: u32,
    height: u32,
    ask_side: bool,
) -> TurnedText {
    let g = glyph_height.max(4.0);
    // The direction a word of letters run together runs in, at whatever angle: three times as
    // long as thick, longer than a letter and four fifths, and as thick as a line of letters, not
    // as a stroke. A tall letter alone (a d, an l) is long for its width, but not that long.
    let axis = |index: usize| -> Option<f32> {
        axes[index]
            .filter(|&(_, elongation, thickness)| {
                elongation >= 3.0
                    && elongation * thickness >= g * 1.8
                    && thickness >= g * 0.3
                    && thickness <= g * 2.6
            })
            .map(|(angle, _, _)| angle)
    };
    // Text-sized blobs: a letter, or letters run together, no thicker than a big letter.
    let letters: Vec<usize> = glyphs
        .iter()
        .copied()
        .filter(|&index| {
            let c = &components[index];
            let thickness = axes[index].map_or(c.width().min(c.height()) as f32, |(_, _, t)| t);
            size(c) >= g * 0.5 && thickness <= g * 2.6
        })
        .collect();
    if letters.len() < 12 {
        return TurnedText::default();
    }
    let grid = Grid::new(components, letters.iter().copied(), g * 2.5, width, height);
    let far = g * 6.0;
    // Blobs as near as the next letter or the next word, by the gap between their boxes, so a
    // letter beside a long word finds the word and not the line above.
    let reach = g * 1.5;
    // Directions between letters two or three apart: a tall letter beside a short one points a
    // fifth of a turn off their line, but letters farther apart, along the same line, point along
    // it within a few degrees; the next line is farther still. Every letter's neighbours at that
    // distance, with the direction to each folded into [0, 180).
    let (near_from, near_to) = (g * 1.2, g * 2.4);
    let mut around: Vec<Vec<(f32, f32)>> = vec![Vec::new(); components.len()];
    for &index in &letters {
        let c = &components[index];
        let [cx, cy] = center(c);
        let own = size(c);
        grid.around(cx, cy, near_to + own, |other| {
            if other == index {
                return;
            }
            let o = &components[other];
            if !(0.4..=2.5).contains(&(size(o) / own)) {
                return;
            }
            let [ox, oy] = center(o);
            let distance = (ox - cx).hypot(oy - cy);
            if distance < g * 0.5 || distance > near_to {
                return;
            }
            let angle = (oy - cy).atan2(ox - cx).to_degrees().rem_euclid(180.0);
            around[index].push((angle, distance));
        });
    }
    // The pile of directions, a degree a bin, smoothed over a few degrees. A word's long axis
    // counts as two neighbours.
    let mut histogram = [0f32; 180];
    for &index in &letters {
        for &(angle, distance) in &around[index] {
            if distance >= near_from {
                histogram[(angle as usize).min(179)] += 1.0;
            }
        }
        if let Some(angle) = axis(index) {
            histogram[(angle as usize).min(179)] += 2.0;
        }
    }
    let smoothed: Vec<f32> = (0..180i32)
        .map(|bin| {
            (-3i32..=3)
                .map(|d| histogram[(bin + d).rem_euclid(180) as usize])
                .sum()
        })
        .collect();
    let total: f32 = histogram.iter().sum();
    let mut page_on_its_side = false;
    if ask_side {
        let level: f32 = (0..=15).chain(165..180).map(|bin| histogram[bin]).sum();
        let upright: f32 = (75..=105).map(|bin| histogram[bin]).sum();
        if std::env::var_os("MUSAEUM_VISION_DEBUG").is_some() {
            eprintln!(
                "directions: level {level:.0}, upright {upright:.0}, total {total:.0}, letters {}",
                letters.len()
            );
        }
        page_on_its_side = upright >= ON_ITS_SIDE_MIN && upright >= level * ON_ITS_SIDE;
    }
    // The page's own direction is level, whatever pile is biggest: the picture has the page's
    // /Rotate applied, and a page with more turned text than level text (a chart's labels, a
    // sideways table) still has its turned text in blocks of their own.
    let main = 0.0f32;
    // Other piles: peaks well apart from the page's direction, with some mass to them.
    let mut peaks: Vec<f32> = Vec::new();
    for bin in 0..180usize {
        let here = smoothed[bin];
        let left = smoothed[(bin + 179) % 180];
        let right = smoothed[(bin + 1) % 180];
        if here < 8.0f32.max(total * 0.02)
            || here < left
            || here < right
            || folded_apart(bin as f32, main) < OFF_LEVEL_DEGREES
        {
            continue;
        }
        if peaks
            .iter()
            .all(|&peak| folded_apart(peak, bin as f32) >= APART_DEGREES)
        {
            peaks.push(bin as f32);
        }
    }
    if peaks.is_empty() {
        return TurnedText {
            page_on_its_side,
            blocks: Vec::new(),
        };
    }
    let pile_of = |angle: f32| -> Option<usize> {
        peaks
            .iter()
            .position(|&peak| folded_apart(peak, angle) <= MATCH_DEGREES)
    };
    // Each letter's pile: its word's long axis, or else the direction most of its neighbours lie
    // along, if that is not level: at least two of them, and more than lie level.
    let mut pile: Vec<Option<usize>> = vec![None; components.len()];
    for &index in &letters {
        if let Some(angle) = axis(index) {
            pile[index] = pile_of(angle);
            continue;
        }
        let mut votes = vec![0u32; peaks.len()];
        let mut level = 0u32;
        for &(angle, _) in &around[index] {
            if folded_apart(angle, main) <= 8.0 {
                level += 1;
            }
            for (slot, &peak) in peaks.iter().enumerate() {
                if folded_apart(angle, peak) <= 8.0 {
                    votes[slot] += 1;
                }
            }
        }
        let best = (0..peaks.len()).max_by_key(|&slot| votes[slot]);
        pile[index] = best.filter(|&slot| votes[slot] >= 2 && votes[slot] > level);
    }
    let mut parent: Vec<usize> = (0..components.len()).collect();
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let union = |parent: &mut Vec<usize>, a: usize, b: usize| {
        let (ra, rb) = (find(parent, a), find(parent, b));
        if ra != rb {
            parent[ra.max(rb)] = ra.min(rb);
        }
    };
    // Lines: blobs of one pile linked to those of the same pile that lie along it from them, near
    // enough to be the next letter or the next word.
    for &index in &letters {
        let Some(p) = pile[index] else {
            continue;
        };
        let c = &components[index];
        let [cx, cy] = center(c);
        grid.around(cx, cy, reach + size(c) + far, |other| {
            if other == index || pile[other] != Some(p) {
                return;
            }
            let o = &components[other];
            let (gx, gy) = gaps(c, o);
            if gx.hypot(gy) > reach {
                return;
            }
            let [ox, oy] = center(o);
            let angle = (oy - cy).atan2(ox - cx).to_degrees().rem_euclid(180.0);
            if folded_apart(angle, peaks[p]) <= 15.0 {
                union(&mut parent, index, other);
            }
        });
    }
    // A line is three letters or more; a blob of letters run together counts as three.
    let weight = |index: usize| if axis(index).is_some() { 3 } else { 1 };
    let mut chain = vec![0usize; components.len()];
    for &index in &letters {
        if pile[index].is_some() {
            let root = find(&mut parent, index);
            chain[root] += weight(index);
        }
    }
    let lined: Vec<bool> = (0..components.len())
        .map(|index| pile[index].is_some() && chain[find(&mut parent, index)] >= MIN_MEMBERS)
        .collect();
    // Lines of one pile as near as a block's lines are become one block.
    for &index in &letters {
        if !lined[index] {
            continue;
        }
        let p = pile[index];
        let c = &components[index];
        let [cx, cy] = center(c);
        grid.around(cx, cy, g * 2.0 + size(c) + far, |other| {
            if other != index && lined[other] && pile[other] == p {
                let (gx, gy) = gaps(c, &components[other]);
                if gx.hypot(gy) <= g * 2.0 {
                    union(&mut parent, index, other);
                }
            }
        });
    }
    let mut groups: std::collections::BTreeMap<usize, Vec<usize>> =
        std::collections::BTreeMap::new();
    for &index in &letters {
        if lined[index] {
            let root = find(&mut parent, index);
            groups.entry(root).or_default().push(index);
        }
    }
    let mut blocks = Vec::new();
    for members in groups.into_values() {
        // One blob alone is a block only if it is a word, long enough to be three letters; a tall
        // letter of a heading is not.
        if members.len() == 1
            && axes[members[0]]
                .is_none_or(|(_, elongation, thickness)| elongation * thickness < g * 3.0)
        {
            continue;
        }
        let p = pile[members[0]].expect("grouped by pile");
        // The block's own angle: the mean of its letters' directions to letters two or three
        // along, and of its words' long axes, near the pile's, doubled so 1° and 179° average to
        // 0° rather than 90°; the pile's own when there is nothing to average.
        let (mut sx, mut sy) = (0.0f32, 0.0f32);
        let mut add = |angle: f32, weight: f32| {
            if folded_apart(angle, peaks[p]) <= MATCH_DEGREES {
                let doubled = (angle * 2.0).to_radians();
                sx += doubled.cos() * weight;
                sy += doubled.sin() * weight;
            }
        };
        for &index in &members {
            if let Some(angle) = axis(index) {
                add(angle, 2.0);
            }
            for &(angle, distance) in &around[index] {
                if distance >= near_from {
                    add(angle, 1.0);
                }
            }
        }
        let angle = if sx.hypot(sy) > 0.0 {
            (sy.atan2(sx) / 2.0).rem_euclid(PI)
        } else {
            peaks[p].to_radians()
        };
        blocks.push(Block { angle, members });
    }
    TurnedText {
        page_on_its_side,
        blocks,
    }
}

/// Blocks at the same angle whose areas touch become one, until none do. A member touches a block
/// when its box, turned with the block, comes within a letter of the block's area: a word is one
/// blob as long as several letters, and its middle may be far from where it begins.
fn merge_touching(mut blocks: Vec<Block>, components: &[Component], g: f32) -> Vec<Block> {
    loop {
        let mut merged = false;
        'outer: for a in 0..blocks.len() {
            let frame = Frame::around(components, &blocks[a].members, blocks[a].angle, g);
            for b in a + 1..blocks.len() {
                // Pieces of one pile differ by less than piles do.
                let apart =
                    folded_apart(blocks[a].angle.to_degrees(), blocks[b].angle.to_degrees());
                if apart >= APART_DEGREES {
                    continue;
                }
                let touches = blocks[b].members.iter().any(|&index| {
                    let c = &components[index];
                    let (mut u0, mut v0, mut u1, mut v1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
                    for (x, y) in [
                        (c.left, c.top),
                        (c.right, c.top),
                        (c.left, c.bottom),
                        (c.right, c.bottom),
                    ] {
                        let [u, v] = frame.to_frame(x as f32, y as f32);
                        u0 = u0.min(u);
                        v0 = v0.min(v);
                        u1 = u1.max(u);
                        v1 = v1.max(v);
                    }
                    u1 >= -g
                        && v1 >= -g
                        && u0 <= frame.width as f32 + g
                        && v0 <= frame.height as f32 + g
                });
                if touches {
                    // The angle of the two, each as much as it has members, doubled so 1° and 179°
                    // average to 0°.
                    let (wa, wb) = (
                        blocks[a].members.len() as f32,
                        blocks[b].members.len() as f32,
                    );
                    let (sa, ca) = (blocks[a].angle * 2.0).sin_cos();
                    let (sb, cb) = (blocks[b].angle * 2.0).sin_cos();
                    blocks[a].angle =
                        ((sa * wa + sb * wb).atan2(ca * wa + cb * wb) / 2.0).rem_euclid(PI);
                    let taken = std::mem::take(&mut blocks[b].members);
                    blocks[a].members.extend(taken);
                    blocks.remove(b);
                    merged = true;
                    break 'outer;
                }
            }
        }
        if !merged {
            return blocks;
        }
    }
}

/// Makes blocks whole: blocks at the same angle whose areas touch become one, and every letter whose
/// centre lies in a block's area, turned with it, joins it. A lone letter of a turned line, whose
/// neighbours did not agree on a direction, and a short word, belong to the block they are in, not
/// to the page's level text. Pieces of a block that the letters taken in bring together are merged
/// once more.
pub fn absorb(
    blocks: Vec<Block>,
    components: &[Component],
    glyphs: &[usize],
    level: &[bool],
    glyph_height: f32,
) -> Vec<Block> {
    let g = glyph_height.max(4.0);
    let mut blocks = merge_touching(blocks, components, g);
    let mut taken = vec![false; components.len()];
    for block in &blocks {
        for &member in &block.members {
            taken[member] = true;
        }
    }
    for block in &mut blocks {
        // Its area, and a letter and a half more at both ends of its lines, where the first and
        // last letters, with neighbours on one side only, may not have joined it. A last word of
        // several letters reaches further: the area grows with what it took, a few times over, but
        // after the first time only by letters that do not sit as the page's level text does
        // (`level`), so a block never runs on into a paragraph beside it.
        let mut heights: Vec<f32> = block
            .members
            .iter()
            .map(|&index| size(&components[index]))
            .collect();
        heights.sort_by(f32::total_cmp);
        let typical = heights.get(heights.len() / 2).copied().unwrap_or(g);
        for pass in 0..ABSORB_PASSES {
            let frame = Frame::around(components, &block.members, block.angle, g * 0.25);
            let along = g * 1.5;
            let mut took = false;
            for &index in glyphs {
                if taken[index] || (pass > 0 && level[index]) {
                    continue;
                }
                let c = &components[index];
                // Further out, a letter of the block's own size, not a heading's or a speck.
                if pass > 0 && !(0.4..=2.5).contains(&(size(c) / typical)) {
                    continue;
                }
                let [cx, cy] = center(c);
                let [u, v] = frame.to_frame(cx, cy);
                if u >= -along
                    && v >= 0.0
                    && u <= frame.width as f32 + along
                    && v <= frame.height as f32
                {
                    block.members.push(index);
                    taken[index] = true;
                    took = true;
                }
            }
            if !took {
                break;
            }
        }
    }
    merge_touching(blocks, components, g)
}

/// How many times letters taken for dots are looked for, each time also beside those found before.
const DOT_LETTER_PASSES: usize = 3;

/// The letters of a turned block that the halftone test took for dots (`lattice`). The lines of a
/// block at 45° lie on a grid of diagonals, and two letters run together into one roundish blob
/// have neighbours on that grid two ways, as a screen's dots do. Kept out of the block, they leave
/// holes in its frame, and a line whose last word is such a pair ends before it. Those inside the
/// block's own area, the size of its letters and against one of them, are its letters: a screen's
/// dots beside the block are outside its area, or apart from its letters.
pub fn letters_taken_for_dots(
    block: &Block,
    components: &[Component],
    lattice: &[bool],
    glyph_height: f32,
) -> Vec<usize> {
    let g = glyph_height.max(4.0);
    let frame = Frame::around(components, &block.members, block.angle, 0.0);
    let mut sizes: Vec<f32> = block
        .members
        .iter()
        .map(|&index| size(&components[index]))
        .collect();
    sizes.sort_by(f32::total_cmp);
    let typical = sizes.get(sizes.len() / 2).copied().unwrap_or(g);
    let mut open: Vec<usize> = (0..components.len())
        .filter(|&index| lattice[index])
        .filter(|&index| {
            let c = &components[index];
            let [u, v] = frame.to_frame(center(c)[0], center(c)[1]);
            (0.4..=2.5).contains(&(size(c) / typical))
                && u >= 0.0
                && v >= 0.0
                && u <= frame.width as f32
                && v <= frame.height as f32
        })
        .collect();
    let mut letters: Vec<usize> = block.members.clone();
    let mut found: Vec<usize> = Vec::new();
    for _ in 0..DOT_LETTER_PASSES {
        let before = found.len();
        open.retain(|&index| {
            let c = &components[index];
            let against = letters.iter().any(|&letter| {
                let (gx, gy) = gaps(c, &components[letter]);
                gx <= g * 0.5 && gy <= g * 0.5
            });
            if against {
                found.push(index);
            }
            !against
        });
        if found.len() == before {
            break;
        }
        letters.extend_from_slice(&found[before..]);
    }
    found
}

/// What a block's letters and its area look like, to tell turned text from what only lines up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockLooks {
    /// The share of its members that sit as the page's level text does (`components::text_like`):
    /// letters of level lines stacked one above the other line up at 90° too.
    pub level: f32,
    /// The share of its area, turned with it, that is ink of any kind: a halftone's dots merge into
    /// blobs the size of letters, and lie on a screen's diagonals, in an area mostly ink.
    pub ink: f32,
    /// Dots of a halftone's screen inside its area, for each of its members.
    pub screen: f32,
}

/// How a block looks; `level` is `components::text_like` of every component.
pub fn block_looks(
    block: &Block,
    components: &[Component],
    level: &[bool],
    lattice: &[bool],
    glyph_height: f32,
) -> BlockLooks {
    let members = block.members.len().max(1) as f32;
    let level_members = block.members.iter().filter(|&&index| level[index]).count() as f32;
    let frame = Frame::around(components, &block.members, block.angle, 0.0);
    let area = (frame.width as f32 * frame.height as f32).max(glyph_height * glyph_height);
    let (mut ink, mut screen) = (0.0f32, 0.0f32);
    for (index, c) in components.iter().enumerate() {
        let [cx, cy] = center(c);
        let [u, v] = frame.to_frame(cx, cy);
        if u >= 0.0 && v >= 0.0 && u <= frame.width as f32 && v <= frame.height as f32 {
            ink += c.area as f32;
            if lattice[index] {
                screen += 1.0;
            }
        }
    }
    BlockLooks {
        level: level_members / members,
        ink: (ink / area).min(1.0),
        screen: screen / members,
    }
}

/// A block's own frame: the page turned so the block's lines run left to right, cut to the block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    /// The direction of the block's lines, radians, in the picture.
    pub angle: f32,
    cos: f32,
    sin: f32,
    /// Where the frame's (0, 0) is in the turned page.
    origin: [f32; 2],
    pub width: u32,
    pub height: u32,
}

impl Frame {
    /// The frame of `members` at `angle`, `pad` pixels around them.
    pub fn around(components: &[Component], members: &[usize], angle: f32, pad: f32) -> Self {
        let (sin, cos) = angle.sin_cos();
        let (mut u0, mut v0, mut u1, mut v1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for &index in members {
            let c = &components[index];
            for (x, y) in [
                (c.left, c.top),
                (c.right, c.top),
                (c.left, c.bottom),
                (c.right, c.bottom),
            ] {
                let (x, y) = (x as f32, y as f32);
                let u = x * cos + y * sin;
                let v = -x * sin + y * cos;
                u0 = u0.min(u);
                v0 = v0.min(v);
                u1 = u1.max(u);
                v1 = v1.max(v);
            }
        }
        Self {
            angle,
            cos,
            sin,
            origin: [u0 - pad, v0 - pad],
            width: ((u1 - u0) + 2.0 * pad).ceil().max(1.0) as u32,
            height: ((v1 - v0) + 2.0 * pad).ceil().max(1.0) as u32,
        }
    }

    /// The same block, read the other way round.
    pub fn turned_around(&self) -> Self {
        // Half a turn more: what was the frame's far corner is its origin.
        let angle = self.angle + PI;
        let (sin, cos) = angle.sin_cos();
        Self {
            angle,
            cos,
            sin,
            origin: [
                -(self.origin[0] + self.width as f32),
                -(self.origin[1] + self.height as f32),
            ],
            width: self.width,
            height: self.height,
        }
    }

    /// A point of the frame as a point of the page.
    pub fn to_page(&self, u: f32, v: f32) -> [f32; 2] {
        let (u, v) = (u + self.origin[0], v + self.origin[1]);
        [u * self.cos - v * self.sin, u * self.sin + v * self.cos]
    }

    /// A point of the page as a point of the frame.
    pub fn to_frame(&self, x: f32, y: f32) -> [f32; 2] {
        [
            x * self.cos + y * self.sin - self.origin[0],
            -x * self.sin + y * self.cos - self.origin[1],
        ]
    }

    /// The ink of the components whose labels `keep` marks, as the frame sees it.
    pub fn mask(&self, labels: &[u32], page_width: u32, page_height: u32, keep: &[bool]) -> Bitmap {
        let mut mask = Bitmap::blank(self.width, self.height);
        let w = page_width as usize;
        for v in 0..self.height {
            for u in 0..self.width {
                let [x, y] = self.to_page(u as f32 + 0.5, v as f32 + 0.5);
                if x < 0.0 || y < 0.0 || x >= page_width as f32 || y >= page_height as f32 {
                    continue;
                }
                let label = labels[y as usize * w + x as usize];
                if label != 0 && keep[label as usize - 1] {
                    mask.bits[(v * self.width + u) as usize] = 1;
                }
            }
        }
        mask
    }
}

/// How much the lines found in a frame look upside down, from -1 (surely upright) to 1 (surely
/// upside down): how much more ink hangs below their small letters than stands above them.
/// Upright, ascenders, capitals and accents outweigh descenders.
///
/// Each line's ink is profiled across it, row by row from its baseline, and the band of the small
/// letters is where the profile is at least half its peak: measured here, not taken from the line
/// finder, whose band is a pixel or two off and would put the letters' own feet in the count. The
/// ink just above that band and just below it, each as deep as half the band, is compared.
pub fn upside_down(mask: &Bitmap, lines: &[PixelLine]) -> f32 {
    let (mut above, mut below) = (0f32, 0f32);
    let w = mask.width as usize;
    let h = mask.height as i64;
    for line in lines {
        let xh = line.x_height.max(2.0);
        let reach_up = (xh * 2.5).ceil() as i64;
        let reach_down = (xh * 1.5).ceil() as i64;
        let mut profile = vec![0f32; (reach_up + reach_down + 1) as usize];
        let x0 = line.left.max(0.0) as usize;
        let x1 = (line.right.max(0.0) as usize).min(w);
        for x in x0..x1 {
            let base = baseline_at(&line.baseline, x as f32).round() as i64;
            for (slot, offset) in (-reach_up..=reach_down).enumerate() {
                let y = base + offset;
                if y >= 0 && y < h {
                    profile[slot] += f32::from(mask.bits[y as usize * w + x]);
                }
            }
        }
        let peak = profile.iter().copied().fold(0.0f32, f32::max);
        if peak <= 0.0 {
            continue;
        }
        let at_peak = profile.iter().position(|&v| v == peak).unwrap_or(0);
        let mut top = at_peak;
        while top > 0 && profile[top - 1] >= peak * 0.5 {
            top -= 1;
        }
        let mut bottom = at_peak;
        while bottom + 1 < profile.len() && profile[bottom + 1] >= peak * 0.5 {
            bottom += 1;
        }
        let band = (bottom - top + 1) as f32;
        let depth = (band * 0.5).ceil() as usize;
        let gap = (band * 0.1).ceil() as usize;
        let sum = |from: usize, to: usize| -> f32 {
            profile[from.min(profile.len())..to.min(profile.len())]
                .iter()
                .sum()
        };
        above += sum(top.saturating_sub(gap + depth), top.saturating_sub(gap));
        below += sum(bottom + 1 + gap, bottom + 1 + gap + depth);
    }
    if above + below <= 0.0 {
        return 0.0;
    }
    (below - above) / (below + above)
}

/// The height of a baseline at `x`, held level past its ends.
pub fn baseline_at(points: &[[f32; 2]], x: f32) -> f32 {
    let Some(first) = points.first() else {
        return 0.0;
    };
    if x <= first[0] {
        return first[1];
    }
    for pair in points.windows(2) {
        let ([ax, ay], [bx, by]) = (pair[0], pair[1]);
        if x <= bx {
            let t = if bx > ax { (x - ax) / (bx - ax) } else { 0.0 };
            return ay + (by - ay) * t;
        }
    }
    points[points.len() - 1][1]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn letter(x: f32, y: f32) -> Component {
        Component {
            left: x as u32,
            top: y as u32,
            right: x as u32 + 7,
            bottom: y as u32 + 10,
            area: 35,
        }
    }

    #[test]
    fn a_column_of_letters_up_the_side_is_a_block_of_its_own() {
        let mut components = Vec::new();
        // Level lines.
        for line in 0..10 {
            for at in 0..30 {
                components.push(letter(100.0 + at as f32 * 9.0, 100.0 + line as f32 * 25.0));
            }
        }
        let level = components.len();
        // A line going up the left side: letters one above another.
        for at in 0..20 {
            components.push(Component {
                left: 40,
                top: 400 - at * 9,
                right: 50,
                bottom: 407 - at * 9,
                area: 35,
            });
        }
        let glyphs: Vec<usize> = (0..components.len()).collect();
        let axes = vec![None; components.len()];
        let blocks = turned_blocks(&components, &glyphs, &axes, 10.0, 600, 600);
        assert_eq!(blocks.len(), 1, "{blocks:?}");
        assert!(blocks[0].members.iter().all(|&m| m >= level));
        assert_eq!(blocks[0].members.len(), 20);
        assert!(
            (blocks[0].angle.to_degrees() - 90.0).abs() < 2.0,
            "{}",
            blocks[0].angle.to_degrees()
        );
    }

    #[test]
    fn letters_taken_for_dots_go_back_to_their_block_but_dots_beside_it_do_not() {
        // A line at 45°, a letter every 7 px each way, and a second line under it.
        let mut components = Vec::new();
        for line in 0..2 {
            for at in 0..12 {
                let shift = line as f32 * 18.0;
                components.push(letter(
                    100.0 + at as f32 * 7.0 - shift,
                    100.0 + at as f32 * 7.0 + shift,
                ));
            }
        }
        let members: Vec<usize> = (0..components.len())
            .filter(|&i| i != 5 && i != 11)
            .collect();
        let block = Block {
            angle: PI / 4.0,
            members,
        };
        // A screen's dot beside the block, outside its area.
        components.push(letter(300.0, 100.0));
        let mut lattice = vec![false; components.len()];
        for index in [5, 11, components.len() - 1] {
            lattice[index] = true;
        }
        let mut found = letters_taken_for_dots(&block, &components, &lattice, 10.0);
        found.sort_unstable();
        // The middle letter, and the last one of the line, beside the letter before it.
        assert_eq!(found, vec![5, 11]);
    }

    #[test]
    fn a_frame_turns_points_both_ways() {
        let components = [letter(100.0, 100.0), letter(100.0, 200.0)];
        let frame = Frame::around(&components, &[0, 1], PI / 2.0, 5.0);
        for (x, y) in [(103.0, 105.0), (106.0, 210.0)] {
            let [u, v] = frame.to_frame(x, y);
            let [bx, by] = frame.to_page(u, v);
            assert!((bx - x).abs() < 1e-3 && (by - y).abs() < 1e-3);
        }
        // Down the page is along the frame's lines.
        let [u0, _] = frame.to_frame(103.0, 100.0);
        let [u1, _] = frame.to_frame(103.0, 200.0);
        assert!(u1 > u0);
        let around = frame.turned_around();
        let [u2, v2] = around.to_frame(103.0, 150.0);
        let [x, y] = around.to_page(u2, v2);
        assert!((x - 103.0).abs() < 1e-3 && (y - 150.0).abs() < 1e-3);
        assert!(u2 >= 0.0 && v2 >= 0.0 && u2 <= around.width as f32 && v2 <= around.height as f32);
    }
}

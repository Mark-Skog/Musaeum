//! The whole way from a gray picture of a page to its [`LineModel`].
//!
//! Ink is binarized and labeled into components. The typical letter is measured on the components
//! that sit as letters do, so a halftone photograph's dots, however many, do not decide it; every
//! component is then sorted by its shape against that letter (letters, specks, rules, edges,
//! figures), and the cells of the page that hold texture rather than text make a picture mask whose
//! blobs are figures too. Letters whose neighbours run at another angle than the page's lines form
//! turned blocks. The page's own text is traced as before: skew, bands and columns, lines strip by
//! strip; each turned block is traced the same way in a frame of its own, turned level, and read the
//! other way round when its letters stand upside down. Every line is then checked to be text
//! (`verify`) before it goes into the model.
//!
//! A page whose own text runs up or down it (a table or a figure set sideways, the last page of a
//! manual printed the long way) is turned a quarter, so its text runs level, and analyzed again:
//! its lines are traced as a page's own are, in columns and bands, and what stays level on the
//! page as shown (a folio, a scanner's mark) becomes a turned block there. Its lines are turned
//! back onto the page as shown before they go into the model.

use std::time::Instant;

use crate::components::{self, Component, Shape};
use crate::gray::Gray;
use crate::lines::{self, Context, PixelLine};
use crate::model::{Column, LINE_TURNED, LINES_VERSION, LineModel, VisualLine};
use crate::orient::{self, Frame};
use crate::picture::{self, PictureMask};
use crate::sauvola::{self, Bitmap, SauvolaOptions};
use crate::skew;
use crate::verify::{self, Verdict};

/// How a page is analyzed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DetectOptions {
    /// Quarter turns clockwise the page is shown with, which the picture already has applied.
    pub rotation: u8,
    pub sauvola: SauvolaOptions,
}

/// What the analysis found on the way, for reports and tests.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stats {
    pub ink_pixels: usize,
    pub components: usize,
    pub glyphs: usize,
    pub specks: usize,
    pub figures: usize,
    pub rules: usize,
    pub edges: usize,
    pub glyph_height: f32,
    pub skew_degrees: f32,
    pub columns: usize,
    pub lines: usize,
    /// Cells of the picture mask, and blobs it turned into figures.
    pub picture_cells: usize,
    pub picture_blobs: usize,
    /// Blocks of text at other angles, by their angle in degrees, in the picture as analyzed.
    pub turned_blocks: Vec<f32>,
    /// Quarter turns clockwise the picture was given for its own text to run level: 1 or 3 for a
    /// page on its side, else 0.
    pub page_quarters: u8,
    /// Lines found and then left out, by why.
    pub dropped: [usize; 5],
    /// Wall time, in milliseconds: binarizing, labeling and sorting, the skew, the lines.
    pub ms: [f32; 4],
}

/// A page analyzed: its lines in pixels of the picture, as the model, and the numbers.
#[derive(Clone, Debug)]
pub struct Detection {
    pub model: LineModel,
    /// Every line of the model, in pixels of the picture, in the model's order.
    pub pixel_lines: Vec<PixelLine>,
    /// The bands of the page's own text, each cut into its columns, in pixels.
    pub regions: Vec<Vec<lines::Area>>,
    /// Where the picture mask is, in the picture as analyzed: turned `stats.page_quarters`
    /// quarters clockwise from the one given ([`Gray::quarter_point`]).
    pub pictures: PictureMask,
    pub stats: Stats,
}

/// A point of the page as shown, in fractions, as a point of the page as stored.
pub fn to_stored(x: f32, y: f32, rotation: u8) -> [f32; 2] {
    match rotation % 4 {
        1 => [y, 1.0 - x],
        2 => [1.0 - x, 1.0 - y],
        3 => [1.0 - y, x],
        _ => [x, y],
    }
}

/// A point of the page as stored, in fractions, as a point of the page as shown.
pub fn to_shown(u: f32, v: f32, rotation: u8) -> [f32; 2] {
    match rotation % 4 {
        1 => [1.0 - v, u],
        2 => [1.0 - u, 1.0 - v],
        3 => [v, 1.0 - u],
        _ => [u, v],
    }
}

/// The most dots of a halftone's screen a turned block may have in its area, for each of its
/// letters. Where a plate's screen runs dark its dots merge into blobs the size of letters, which lie
/// on the screen's diagonals like turned text; but turned text on paper has none of the screen's
/// dots around it. On the corpus, text had at most 0.13 a letter, and blocks inside plates mostly
/// 0.2 to 5.
const SCREEN_AROUND_TEXT: f32 = 0.15;

/// A line this short, in letter heights, beside this many dots of a halftone's screen is the cut
/// edge of a plate, whose part-dots line up like a word, not a word: a caption or a number on paper
/// has no screen around it.
const EDGE_LINE_GLYPHS: f32 = 4.0;
const EDGE_SCREEN_DOTS: usize = 3;

/// How many scans, each way, look for a table's grid inside its frame.
const TABLE_SCANS: u32 = 9;

/// How clearly a turned block's letters have to say it is upside down for it to be read the other
/// way round: on the corpus the sign of what they say was right on every block, from 0.12 up, and a
/// smudge says less.
const UPSIDE_DOWN_EVIDENCE: f32 = 0.1;
/// How near upright a turned line stands, as the cosine of its angle, for the way it is usually set
/// to be upward rather than to the right: within about 15° of upright.
const UPRIGHT_COS: f32 = 0.26;

/// A stroke drawn, not a letter: at most this thick, in letters, and at least this long. The thin
/// letters (l, i, a slash) are thinner but less than two letters long, a long dash is a letter long,
/// and words of letters run together are as thick as their letters.
const STROKE_THICKNESS: f32 = 0.25;
const STROKE_LENGTH: f32 = 2.5;
/// How much of its length times its thickness a stroke's ink fills, at least.
const STROKE_FILL: f32 = 0.6;

/// An underline: ink across at most this thick, in letters, at least this long.
const UNDERLINE_THICKNESS: f32 = 0.22;
const UNDERLINE_LENGTH: f32 = 1.5;

/// How much more of a page's letters, as a share of them all, its lines have to take in turned a
/// quarter for the page to be read turned. On a manual of the real collection, the pages set
/// sideways (tables, a list, the last page printed the long way) went from 0.41 to 0.64 of their
/// letters as they are to 0.95 to 0.99 turned; a drawing whose labels stand at 90° went from 0.61
/// to 0.71, and the test book's page of blocks at 90° both ways beside level text read 0.93 to 1.00
/// either way.
const TURNED_BETTER: f32 = 0.15;

/// How many fewer of a page's letters, as a share of them all, its lines may take in turned than
/// as it is for a page whose upright blocks all read one way to be read turned: about as many.
/// Turned, the page's columns and bands are found as a page's are; in blocks, each is traced
/// alone, and the lines of a page read down it were found a little less well (F1 0.949 against
/// 0.987 on the labeled corpus).
const TURNED_AS_WELL: f32 = 0.02;

/// The share of the letters of a page's upright blocks that the blocks reading the other way may
/// have, for all of them to read one way.
const ONE_WAY: f32 = 0.1;

/// How far off level, as the tangent of the angle, a line of a page turned for its analysis runs
/// to be a turned line on the page as shown: a fifth of a right angle.
const TURNED_TAN: f32 = 0.325;

/// Lines of one area of text, in pixels of the page, with the area's box.
struct Traced {
    lines: Vec<PixelLine>,
    turned: bool,
}

fn verdict_slot(verdict: Verdict) -> Option<usize> {
    match verdict {
        Verdict::Text => None,
        Verdict::Empty => Some(0),
        Verdict::WrongHeight => Some(1),
        Verdict::NotAligned => Some(2),
        Verdict::InPicture => Some(3),
        Verdict::NotLetters => Some(4),
    }
}

/// What one analysis of a picture found, before its lines go into a model.
struct Analysis {
    traced: Vec<Traced>,
    regions: Vec<Vec<lines::Area>>,
    pictures: PictureMask,
    stats: Stats,
    skew: f32,
    /// How upside down the page's own lines look, from -1 to 1 (`orient::upside_down`), when the
    /// picture was turned for them.
    upside_down: f32,
    /// Whether the page's own text may run up or down it, when asked.
    on_its_side: bool,
    /// The share of the page's letters its lines take in, when the page may be on its side or
    /// was turned for its analysis.
    explained: f32,
    /// Its blocks within a fifth of a right angle of upright read one way, up the page or down
    /// it, but for a few of their letters; or there are none.
    one_way: bool,
}

/// The lines of `mask`, whose letters are `glyphs`, in reading order: band by band, column by
/// column. Each area's lines come together.
fn trace(
    mask: &Bitmap,
    glyphs: &[Component],
    glyph_height: f32,
    skew: f32,
    tables: &[lines::Area],
    rules: &[lines::Rule],
) -> (Vec<Vec<lines::Area>>, Vec<Vec<PixelLine>>) {
    let context = Context {
        glyph_height,
        skew,
        width: mask.width,
        height: mask.height,
    };
    let regions = lines::find_regions_with_tables(glyphs, &context, tables, rules);
    let mut areas = Vec::new();
    for band in &regions {
        let first = areas.len();
        let mut found: Vec<Vec<PixelLine>> = band
            .iter()
            .enumerate()
            .map(|(at, area)| {
                lines::find_lines_with_tables(mask, *area, (first + at) as u16, &context, tables)
            })
            .collect();
        lines::join_across_columns(&mut found, glyph_height);
        areas.extend(found);
    }
    (regions, areas)
}

/// Finds the lines of text on `gray`.
pub fn detect(gray: &Gray, options: &DetectOptions) -> Detection {
    let (width, height) = (gray.width, gray.height);
    if width < 8 || height < 8 {
        return Detection {
            model: LineModel::empty(options.rotation),
            pixel_lines: Vec::new(),
            regions: Vec::new(),
            pictures: PictureMask::empty(width.max(1), height.max(1), 8),
            stats: Stats::default(),
        };
    }
    let upright = analyze(gray, options, true);
    // Turned, the page could only read better if its upright blocks read one way, or if as it is
    // its lines leave enough of its letters out for the turn to take in clearly more.
    let could_be_better = upright.one_way || upright.explained < 1.0 - TURNED_BETTER;
    if !upright.on_its_side || !could_be_better {
        return into_detection(upright, 0, width, height, options.rotation);
    }
    // A page that may be on its side. Text set sideways most often runs up the page, and reads
    // level once the picture turns a quarter clockwise; if its letters then stand upside down, it
    // ran down the page, and the other quarter is the one. The page is turned only if, turned, its
    // lines take in clearly more of its letters: a page of blocks at 90° both ways beside level
    // text reads as well as it is, its blocks turned in frames of their own.
    let mut turned = analyze(&gray.quarter_turned(1), options, false);
    let mut quarters = 1;
    if turned.upside_down >= UPSIDE_DOWN_EVIDENCE {
        turned = analyze(&gray.quarter_turned(3), options, false);
        quarters = 3;
    }
    // Read turned when that takes in clearly more of the letters; or as many, when the blocks
    // standing upright on the page all read one way, as a page set sideways does, and not some up
    // and some down, as blocks beside level text may.
    let clearly = turned.explained >= upright.explained + TURNED_BETTER;
    let as_well = turned.explained >= upright.explained - TURNED_AS_WELL && upright.one_way;
    if std::env::var_os("MUSAEUM_VISION_DEBUG").is_some() {
        eprintln!(
            "on its side? letters taken in: {:.2} as it is, {:.2} turned {quarters}; upright blocks one way: {}",
            upright.explained, turned.explained, upright.one_way
        );
    }
    if clearly || as_well {
        into_detection(turned, quarters, width, height, options.rotation)
    } else {
        into_detection(upright, 0, width, height, options.rotation)
    }
}

/// The analysis of `gray` as it is. With `ask_side`, it says whether the page's own text may run
/// up or down it; turned for that, how much of its letters its lines take in, and how upside down
/// its own lines look.
fn analyze(gray: &Gray, options: &DetectOptions, ask_side: bool) -> Analysis {
    let mut stats = Stats::default();
    let (width, height) = (gray.width, gray.height);
    let started = Instant::now();
    let bitmap = sauvola::binarize(gray, &options.sauvola);
    stats.ink_pixels = bitmap.ink_count();
    stats.ms[0] = started.elapsed().as_secs_f32() * 1000.0;

    let started = Instant::now();
    let labeled = components::label(&bitmap);
    let all = &labeled.components;
    stats.components = all.len();
    let glyph_height = components::robust_glyph_height(all, width, height);
    stats.glyph_height = glyph_height;
    let mut shapes: Vec<Shape> = all
        .iter()
        .map(|c| components::classify(c, width, height, glyph_height))
        .collect();
    let candidates: Vec<usize> = (0..all.len())
        .filter(|&i| shapes[i] == Shape::Glyph)
        .collect();
    // Turned text first: its letters do not sit beside each other along x, and would otherwise be
    // taken for texture.
    // Dots of a halftone's screen are no letters, however letter-sized they are.
    let lattice = components::lattice(all, &candidates, width, height);
    let candidates: Vec<usize> = candidates.into_iter().filter(|&i| !lattice[i]).collect();
    let moments = components::moments(&labeled.labels, width, all.len());
    let axes: Vec<Option<(f32, f32, f32)>> =
        moments.iter().map(components::principal_axis).collect();
    drop(moments);
    let turned = orient::turned_text(
        all,
        &candidates,
        &axes,
        glyph_height,
        width,
        height,
        ask_side,
    );
    let on_its_side = turned.page_on_its_side;
    let blocks = turned.blocks;
    let level = components::text_like(all, &candidates, width, height);
    let blocks = orient::absorb(blocks, all, &candidates, &level, glyph_height);
    let blocks: Vec<orient::Block> = blocks
        .into_iter()
        .filter(|block| {
            let looks = orient::block_looks(block, all, &level, &lattice, glyph_height);
            if std::env::var_os("MUSAEUM_VISION_DEBUG").is_some() {
                let c = crate::near::center(&all[block.members[0]]);
                eprintln!(
                    "turned block candidate {:.0}° at ({:.0}, {:.0}): {} members, level {:.2}, ink {:.2}, screen {:.2}",
                    block.angle.to_degrees(),
                    c[0],
                    c[1],
                    block.members.len(),
                    looks.level,
                    looks.ink,
                    looks.screen
                );
            }
            looks.screen < SCREEN_AROUND_TEXT
        })
        .collect();
    let mut in_block = vec![false; all.len()];
    for block in &blocks {
        for &member in &block.members {
            in_block[member] = true;
        }
    }
    let mut text = level;
    for (like, &member) in text.iter_mut().zip(&in_block) {
        *like |= member;
    }
    let pictures = picture::find(
        width,
        height,
        &labeled.labels,
        picture::Parts {
            components: all,
            shapes: &shapes,
            text: &text,
            lattice: &lattice,
        },
        glyph_height,
    );
    stats.picture_cells = pictures.count();
    // Blobs in a picture that do not sit as letters are part of it; dots of a screen anywhere.
    for (index, &dot) in lattice.iter().enumerate() {
        if dot && shapes[index] == Shape::Glyph {
            shapes[index] = Shape::Figure;
            stats.picture_blobs += 1;
        }
    }
    for &index in &candidates {
        let [cx, cy] = crate::near::center(&all[index]);
        if !text[index] && pictures.at(cx, cy) {
            shapes[index] = Shape::Figure;
            stats.picture_blobs += 1;
        }
    }
    // A stroke of a pen or a pencil (a scribble in the margin, a line drawn under a word or down
    // beside a paragraph) is long and thinner than any letter, and solid along its length: a line
    // drawn, not text. A word of small letters run together may be as thin, but its letters
    // leave much of its length and thickness empty. In a picture, where a screen's dots run
    // together into strokes too, the picture has them already.
    let g = glyph_height.max(4.0);
    for &index in &candidates {
        let [cx, cy] = crate::near::center(&all[index]);
        if shapes[index] != Shape::Glyph || pictures.at(cx, cy) {
            continue;
        }
        let stroke = axes[index].is_some_and(|(_, elongation, thickness)| {
            let length = elongation * thickness;
            thickness <= g * STROKE_THICKNESS
                && length >= g * STROKE_LENGTH
                && all[index].area as f32 >= length * thickness * STROKE_FILL
        });
        if stroke {
            shapes[index] = Shape::Edge;
        }
    }
    for shape in &shapes {
        match shape {
            Shape::Glyph => stats.glyphs += 1,
            Shape::Speck => stats.specks += 1,
            Shape::Figure => stats.figures += 1,
            Shape::Rule => stats.rules += 1,
            Shape::Edge => stats.edges += 1,
        }
    }
    stats.ms[1] = started.elapsed().as_secs_f32() * 1000.0;

    // The page's own text: its letters, without those of the turned blocks.
    let started = Instant::now();
    let main_keep: Vec<bool> = (0..all.len())
        .map(|i| shapes[i] == Shape::Glyph && !in_block[i])
        .collect();
    let main_glyphs: Vec<Component> = (0..all.len())
        .filter(|&i| main_keep[i])
        .map(|i| all[i])
        .collect();
    let mut mask = Bitmap::blank(width, height);
    for (bit, &label) in mask.bits.iter_mut().zip(&labeled.labels) {
        if label != 0 && main_keep[label as usize - 1] {
            *bit = 1;
        }
    }
    erase_underlines(&mut mask, glyph_height);
    let skew = skew::estimate(
        &main_glyphs,
        width as f32 / 2.0,
        (glyph_height / 6.0).max(1.0),
    );
    stats.skew_degrees = skew.to_degrees();
    stats.ms[2] = started.elapsed().as_secs_f32() * 1000.0;

    let started = Instant::now();
    let in_picture = |x: f32, y: f32| pictures.at(x, y);
    // Ruled tables: frames of thin lines around letters, with a grid inside. Scanned straight
    // across the frame, down and along, a grid's rules are crossed three times or more (the frame's
    // two sides and a rule between), on most of the scans, however the scan tilts or curls the
    // page. The dark edge of a scan, the shadow of a spine or the border of a panel goes around text
    // too, but is crossed twice, or by something thick.
    let crossed_inside = |index: usize| -> bool {
        let c = &all[index];
        let label = index as u32 + 1;
        let thin = (glyph_height * 0.6).max(3.0) as usize;
        let at = |x: u32, y: u32| labeled.labels[(y * width + x) as usize] == label;
        // The thin runs of the component along one scan.
        let thin_runs = |inked: &mut dyn Iterator<Item = bool>| -> usize {
            let (mut runs, mut run) = (0usize, 0usize);
            for is_inked in inked.chain(std::iter::once(false)) {
                if is_inked {
                    run += 1;
                } else {
                    if run > 0 && run <= thin {
                        runs += 1;
                    }
                    run = 0;
                }
            }
            runs
        };
        let scans = TABLE_SCANS;
        let gridded = |down: bool| -> bool {
            (1..=scans)
                .filter(|&step| {
                    if down {
                        let x = c.left + (c.right - c.left) * step / (scans + 1);
                        thin_runs(&mut (c.top..c.bottom).map(|y| at(x, y))) >= 3
                    } else {
                        let y = c.top + (c.bottom - c.top) * step / (scans + 1);
                        thin_runs(&mut (c.left..c.right).map(|x| at(x, y))) >= 3
                    }
                })
                .count()
                * 2
                > scans as usize
        };
        gridded(true) && gridded(false)
    };
    let tables: Vec<lines::Area> = (0..all.len())
        .filter(|&i| {
            let c = &all[i];
            let (w, h) = (c.width() as f32, c.height() as f32);
            shapes[i] == Shape::Figure
                && w > glyph_height * 4.0
                && h > glyph_height * 2.0
                && (c.area as f32) < w * h * 0.12
                && crossed_inside(i)
                && main_glyphs
                    .iter()
                    .filter(|g| {
                        let [x, y] = crate::near::center(g);
                        x > c.left as f32
                            && x < c.right as f32
                            && y > c.top as f32
                            && y < c.bottom as f32
                    })
                    .take(2)
                    .count()
                    == 2
        })
        .map(|i| lines::Area {
            left: all[i].left,
            right: all[i].right,
            top: all[i].top,
            bottom: all[i].bottom,
        })
        .collect();
    if std::env::var_os("MUSAEUM_VISION_DEBUG").is_some() {
        for table in &tables {
            eprintln!(
                "table frame {},{}-{},{}",
                table.left, table.top, table.right, table.bottom
            );
        }
    }
    // Horizontal rules: rules themselves, and the long rows of frames, which a scan may have broken
    // into pieces.
    let mut rules: Vec<lines::Rule> = Vec::new();
    for (index, c) in all.iter().enumerate() {
        let (w, h) = (c.width() as f32, c.height() as f32);
        match shapes[index] {
            Shape::Rule => rules.push((c.left, c.right, (c.top + c.bottom) / 2)),
            Shape::Figure if (c.area as f32) < w * h * 0.12 && w > glyph_height * 3.0 => {
                let label = index as u32 + 1;
                for y in c.top..c.bottom {
                    let row = &labeled.labels
                        [(y * width + c.left) as usize..(y * width + c.right) as usize];
                    let inked = row.iter().filter(|&&l| l == label).count();
                    if inked as f32 >= w * 0.6
                        && rules.last().is_none_or(|r| r.2 + 2 < y || r.0 != c.left)
                    {
                        rules.push((c.left, c.right, y));
                    }
                }
            }
            _ => {}
        }
    }
    let (regions, areas) = trace(&mask, &main_glyphs, glyph_height, skew, &tables, &rules);
    stats.columns = regions.iter().map(Vec::len).max().unwrap_or(0);
    let mut traced: Vec<Traced> = Vec::new();
    // Letters of the blocks that stand upright, read down the page and up it.
    let mut upright_letters = [0usize; 2];
    // The dots of halftone screens, to tell the cut edge of a plate from a short word.
    let screen = crate::near::Grid::new(
        all,
        (0..all.len()).filter(|&index| lattice[index]),
        glyph_height.max(4.0) * 2.0,
        width,
        height,
    );
    // A short line, at any angle, in the page's pixels: whether a screen's dots are around it.
    let at_a_screen = |line: &PixelLine| -> bool {
        let g = glyph_height.max(4.0);
        let length: f32 = line
            .baseline
            .windows(2)
            .map(|pair| (pair[1][0] - pair[0][0]).hypot(pair[1][1] - pair[0][1]))
            .sum();
        if length > g * EDGE_LINE_GLYPHS {
            return false;
        }
        let (Some(first), Some(last)) = (line.baseline.first(), line.baseline.last()) else {
            return false;
        };
        let middle = [(first[0] + last[0]) / 2.0, (first[1] + last[1]) / 2.0];
        let mut dots = 0usize;
        screen.around(middle[0], middle[1], length / 2.0 + g, |_| dots += 1);
        dots >= EDGE_SCREEN_DOTS
    };
    for found in areas {
        let judge = |line: &PixelLine| match verify::judge(line, &main_glyphs, &in_picture) {
            Verdict::Text if at_a_screen(line) => Verdict::InPicture,
            verdict => verdict,
        };
        let mut kept = keep_lines(found, &judge, &mut stats);
        to_column_edges(&mut kept, glyph_height);
        traced.push(Traced {
            lines: kept,
            turned: false,
        });
    }
    let upside_down = if ask_side {
        0.0
    } else {
        let own: Vec<PixelLine> = traced
            .iter()
            .flat_map(|area| area.lines.iter().cloned())
            .collect();
        orient::upside_down(&mask, &own)
    };
    drop(mask);

    // Each turned block in its own frame.
    for block in &blocks {
        let mut keep = vec![false; all.len()];
        for &member in &block.members {
            keep[member] = true;
        }
        for letter in orient::letters_taken_for_dots(block, all, &lattice, glyph_height) {
            keep[letter] = true;
        }
        let pad = glyph_height * 2.0;
        let mut frame = Frame::around(all, &block.members, block.angle, pad);
        let mut found = trace_frame(&frame, &labeled.labels, width, height, &keep, glyph_height);
        let flat: Vec<PixelLine> = found.1.iter().flatten().cloned().collect();
        // Upside down when the letters say so; when they hardly say (a single short line, a title
        // up a chart's axis), the way type is set: a slanted line reads to the right, and one that
        // stands nearly upright reads upward.
        let looks = orient::upside_down(&found.0, &flat);
        let rightward = frame.angle.cos();
        let set_upside_down = if rightward.abs() > UPRIGHT_COS {
            rightward < 0.0
        } else {
            frame.angle.sin() > 0.0
        };
        let flip = if looks.abs() >= UPSIDE_DOWN_EVIDENCE {
            looks > 0.0
        } else {
            set_upside_down
        };
        if std::env::var_os("MUSAEUM_VISION_DEBUG").is_some() {
            let c = crate::near::center(&all[block.members[0]]);
            let (mut l, mut t, mut r, mut b) = (u32::MAX, u32::MAX, 0, 0);
            for &m in &block.members {
                l = l.min(all[m].left);
                t = t.min(all[m].top);
                r = r.max(all[m].right);
                b = b.max(all[m].bottom);
            }
            eprintln!(
                "block at {:.0}° from ({:.0}, {:.0}), box {l},{t}-{r},{b}, {} members, {} lines; upside down {looks:.2}, flip {flip}",
                block.angle.to_degrees(),
                c[0],
                c[1],
                block.members.len(),
                flat.len(),
            );
        }
        if flip {
            frame = frame.turned_around();
            found = trace_frame(&frame, &labeled.labels, width, height, &keep, glyph_height);
        }
        let (frame_mask, areas, frame_glyphs) = found;
        drop(frame_mask);
        if std::env::var_os("MUSAEUM_VISION_DEBUG").is_some() {
            eprintln!(
                "  traced as {} areas of {:?} lines",
                areas.len(),
                areas.iter().map(Vec::len).collect::<Vec<_>>()
            );
        }
        let degrees = frame.angle.to_degrees().rem_euclid(360.0);
        stats.turned_blocks.push(degrees);
        // Read down the page at 90°, with y growing downward; up it at 270°.
        if (degrees - 90.0).abs() <= 18.0 {
            upright_letters[0] += block.members.len();
        } else if (degrees - 270.0).abs() <= 18.0 {
            upright_letters[1] += block.members.len();
        }
        let in_frame_picture = |u: f32, v: f32| {
            let [x, y] = frame.to_page(u, v);
            pictures.at(x, y)
        };
        for found in areas {
            let judge =
                |line: &PixelLine| match verify::judge(line, &frame_glyphs, &in_frame_picture) {
                    Verdict::Text if at_a_screen(&to_page_line(&frame, line.clone())) => {
                        Verdict::InPicture
                    }
                    verdict => verdict,
                };
            let kept = keep_lines(found, &judge, &mut stats)
                .into_iter()
                .map(|line| to_page_line(&frame, line))
                .collect();
            traced.push(Traced {
                lines: kept,
                turned: true,
            });
        }
    }

    let explained = if on_its_side || !ask_side {
        let letters: Vec<Component> = (0..all.len())
            .filter(|&index| {
                shapes[index] == Shape::Glyph
                    && crate::near::size(&all[index]) >= glyph_height * 0.5
            })
            .map(|index| all[index])
            .collect();
        taken_in(
            traced.iter().flat_map(|area| area.lines.iter()),
            &letters,
            glyph_height,
        )
    } else {
        0.0
    };
    let fewer = upright_letters[0].min(upright_letters[1]) as f32;
    let one_way = fewer <= (upright_letters[0] + upright_letters[1]) as f32 * ONE_WAY;
    stats.ms[3] = started.elapsed().as_secs_f32() * 1000.0;
    Analysis {
        traced,
        regions,
        pictures,
        stats,
        skew,
        upside_down,
        on_its_side,
        explained,
        one_way,
    }
}

/// The share of `letters` whose centres lie on one of `lines`: across it, between its ascent
/// above the baseline and its descent below, and along it, within half a letter of its ends.
fn taken_in<'a>(
    lines: impl Iterator<Item = &'a PixelLine>,
    letters: &[Component],
    glyph_height: f32,
) -> f32 {
    if letters.is_empty() {
        return 0.0;
    }
    let pad = glyph_height.max(4.0) * 0.5;
    let centers: Vec<[f32; 2]> = letters.iter().map(crate::near::center).collect();
    let mut taken = vec![false; letters.len()];
    for line in lines {
        let (mut left, mut top, mut right, mut bottom) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for point in &line.baseline {
            left = left.min(point[0]);
            right = right.max(point[0]);
            top = top.min(point[1]);
            bottom = bottom.max(point[1]);
        }
        let reach = line.ascent.max(line.descent) + pad;
        for (index, &[x, y]) in centers.iter().enumerate() {
            if taken[index]
                || x < left - reach
                || x > right + reach
                || y < top - reach
                || y > bottom + reach
            {
                continue;
            }
            taken[index] = line.baseline.windows(2).any(|pair| {
                let ([ax, ay], [bx, by]) = (pair[0], pair[1]);
                let length = (bx - ax).hypot(by - ay);
                if length <= 0.0 {
                    return false;
                }
                let (dx, dy) = ((bx - ax) / length, (by - ay) / length);
                let along = (x - ax) * dx + (y - ay) * dy;
                // Upward across a line going right, with y growing downward.
                let up = (x - ax) * dy - (y - ay) * dx;
                along >= -pad
                    && along <= length + pad
                    && up <= line.ascent
                    && up >= -line.descent - pad * 0.5
            });
        }
    }
    taken.iter().filter(|&&taken| taken).count() as f32 / letters.len() as f32
}

/// Whether a line, by its ends, runs more than a fifth of a right angle off level and to the right.
fn off_level(line: &PixelLine) -> bool {
    let (first, last) = (line.baseline[0], line.baseline[line.baseline.len() - 1]);
    let (dx, dy) = (last[0] - first[0], last[1] - first[1]);
    !(dx > 0.0 && dy.abs() <= dx * TURNED_TAN)
}

/// The lines of an analysis, in a model of the picture given: a picture turned `quarters` for the
/// analysis has its lines and areas turned back first, and every line that does not run level on
/// the page as shown is a turned one.
fn into_detection(
    analysis: Analysis,
    quarters: u8,
    width: u32,
    height: u32,
    rotation: u8,
) -> Detection {
    let Analysis {
        mut traced,
        mut regions,
        pictures,
        mut stats,
        skew,
        ..
    } = analysis;
    let started = Instant::now();
    stats.page_quarters = quarters;
    if quarters != 0 {
        let (turned_width, turned_height) = if quarters % 2 == 1 {
            (height, width)
        } else {
            (width, height)
        };
        let back = (4 - quarters) % 4;
        let unturn =
            |point: [f32; 2]| Gray::quarter_point(point, back, turned_width, turned_height);
        for area in &mut traced {
            for line in &mut area.lines {
                // The gaps as fractions of the arc, which the turn does not change.
                line.gaps = arc_gaps(line);
                line.gaps_are_arc = true;
                for point in &mut line.baseline {
                    *point = unturn(*point);
                }
                line.left = line.baseline.iter().map(|p| p[0]).fold(f32::MAX, f32::min);
                line.right = line.baseline.iter().map(|p| p[0]).fold(f32::MIN, f32::max);
            }
            let off = area.lines.iter().filter(|line| off_level(line)).count();
            area.turned = off * 2 > area.lines.len();
        }
        for band in &mut regions {
            for area in band {
                let a = unturn([area.left as f32, area.top as f32]);
                let b = unturn([area.right as f32, area.bottom as f32]);
                *area = lines::Area {
                    left: a[0].min(b[0]).round() as u32,
                    right: a[0].max(b[0]).round() as u32,
                    top: a[1].min(b[1]).round() as u32,
                    bottom: a[1].max(b[1]).round() as u32,
                };
            }
        }
    }

    // The model: an area with lines is a column, its lines in order.
    let mut model = LineModel {
        version: LINES_VERSION,
        rotation: rotation % 4,
        skew,
        columns: Vec::new(),
        lines: Vec::new(),
    };
    let (w, h) = (width as f32, height as f32);
    let mut pixel_lines = Vec::new();
    for area in traced.into_iter().filter(|area| !area.lines.is_empty()) {
        if model.columns.len() >= crate::model::MAX_COLUMNS {
            break;
        }
        let column = model.columns.len() as u16;
        let (mut left, mut top, mut right, mut bottom) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for line in &area.lines {
            for point in &line.baseline {
                left = left.min(point[0] - line.ascent);
                right = right.max(point[0] + line.ascent);
                top = top.min(point[1] - line.ascent);
                bottom = bottom.max(point[1] + line.descent);
            }
        }
        if !area.turned {
            // Level text: the column is as wide as its lines' ink.
            left = area.lines.iter().map(|l| l.left).fold(f32::MAX, f32::min);
            right = area.lines.iter().map(|l| l.right).fold(f32::MIN, f32::max);
        }
        let corners = [
            to_stored(
                (left / w).clamp(0.0, 1.0),
                (top / h).clamp(0.0, 1.0),
                rotation,
            ),
            to_stored(
                (right / w).clamp(0.0, 1.0),
                (bottom / h).clamp(0.0, 1.0),
                rotation,
            ),
        ];
        model.columns.push(Column {
            left: corners[0][0].min(corners[1][0]),
            top: corners[0][1].min(corners[1][1]),
            right: corners[0][0].max(corners[1][0]),
            bottom: corners[0][1].max(corners[1][1]),
        });
        for mut line in area.lines {
            line.column = column;
            model.lines.push(VisualLine {
                column,
                flags: if area.turned { LINE_TURNED } else { 0 },
                baseline: line
                    .baseline
                    .iter()
                    .map(|point| to_stored(point[0] / w, point[1] / h, rotation))
                    .collect(),
                x_height: line.x_height / h,
                ascent: line.ascent / h,
                descent: line.descent / h,
                gaps: arc_gaps(&line),
            });
            pixel_lines.push(line);
        }
    }
    stats.lines = model.lines.len();
    stats.ms[3] += started.elapsed().as_secs_f32() * 1000.0;
    Detection {
        model,
        pixel_lines,
        regions,
        pictures,
        stats,
    }
}

/// How far a line left out may be from the kept lines above and below it, in the area's usual
/// distance between lines, to be judged again with their heights.
const COLUMN_PITCHES: f32 = 1.6;

/// The lines of one area that `judge` says are text, in order, with the reasons for the others
/// counted in `stats`. A line left out for its letters' height or for how they sit on its
/// baseline, that stands between two kept lines of its area, each about a line's distance from it,
/// is judged again with their x-height, ascent and descent, and kept with them if it passes: a
/// short line of a list or a table of contents (an entry in italics between roman ones, a name and
/// its page) spans a strip or two, too few for the heights it measured to be right, and the
/// column it stands in says what they are. A row of a screen's dots or of specks is no taller
/// against the column's heights than against its own.
fn keep_lines(
    lines: Vec<PixelLine>,
    judge: &dyn Fn(&PixelLine) -> Verdict,
    stats: &mut Stats,
) -> Vec<PixelLine> {
    let verdicts: Vec<Verdict> = lines.iter().map(judge).collect();
    let kept: Vec<bool> = verdicts
        .iter()
        .map(|&verdict| verdict == Verdict::Text)
        .collect();
    let middle = |line: &PixelLine| {
        let x = (line.left + line.right) / 2.0;
        (x, crate::orient::baseline_at(&line.baseline, x))
    };
    // The area's usual distance between lines, from its kept lines one under another; lines side
    // by side on one row (a title and its page) are no step.
    let mut heights: Vec<f32> = lines.iter().map(|line| line.x_height).collect();
    heights.sort_by(f32::total_cmp);
    let same_row = heights.get(heights.len() / 2).copied().unwrap_or(0.0) * 0.8;
    let mut rows: Vec<f32> = lines
        .iter()
        .zip(&kept)
        .filter(|(_, kept)| **kept)
        .map(|(line, _)| middle(line).1)
        .collect();
    rows.sort_by(f32::total_cmp);
    let mut steps: Vec<f32> = rows
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .filter(|&step| step > same_row)
        .collect();
    steps.sort_by(f32::total_cmp);
    let pitch = steps.get(steps.len() / 2).copied();
    // The heights of the kept lines just above and below a line left out, when both are near.
    let column_heights = |index: usize| -> Option<(f32, f32, f32)> {
        let pitch = pitch?;
        let line = &lines[index];
        let (x, y) = middle(line);
        // The nearest kept line above and below: how far, and which.
        let mut above: Option<(f32, usize)> = None;
        let mut below: Option<(f32, usize)> = None;
        for (other, candidate) in lines.iter().enumerate() {
            if !kept[other] || candidate.right <= line.left || candidate.left >= line.right {
                continue;
            }
            let d = crate::orient::baseline_at(
                &candidate.baseline,
                x.clamp(candidate.left, candidate.right),
            ) - y;
            if d < 0.0 && above.is_none_or(|(nearest, _)| -d < nearest) {
                above = Some((-d, other));
            } else if d > 0.0 && below.is_none_or(|(nearest, _)| d < nearest) {
                below = Some((d, other));
            }
        }
        let ((up, a), (down, b)) = (above?, below?);
        let near = |d: f32| d >= pitch * 0.5 && d <= pitch * COLUMN_PITCHES;
        if !near(up) || !near(down) {
            return None;
        }
        let (a, b) = (&lines[a], &lines[b]);
        Some((
            (a.x_height + b.x_height) / 2.0,
            (a.ascent + b.ascent) / 2.0,
            (a.descent + b.descent) / 2.0,
        ))
    };
    let column: Vec<Option<(f32, f32, f32)>> = (0..lines.len())
        .map(|index| match verdicts[index] {
            Verdict::WrongHeight | Verdict::NotAligned => column_heights(index),
            _ => None,
        })
        .collect();
    let mut out = Vec::with_capacity(lines.len());
    for ((mut line, verdict), column) in lines.into_iter().zip(verdicts).zip(column) {
        let verdict = match column {
            Some((x_height, ascent, descent)) => {
                let own = (line.x_height, line.ascent, line.descent);
                (line.x_height, line.ascent, line.descent) = (x_height, ascent, descent);
                match judge(&line) {
                    Verdict::Text => {
                        if std::env::var_os("MUSAEUM_VISION_DEBUG").is_some() {
                            let (a, b) = (line.baseline[0], line.baseline[line.baseline.len() - 1]);
                            eprintln!(
                                "kept as its column's ({verdict:?} at its own x height {:.1}): ({:.0}, {:.0}) -> ({:.0}, {:.0})",
                                own.0, a[0], a[1], b[0], b[1]
                            );
                        }
                        Verdict::Text
                    }
                    _ => {
                        (line.x_height, line.ascent, line.descent) = own;
                        verdict
                    }
                }
            }
            None => verdict,
        };
        match verdict_slot(verdict) {
            None => out.push(line),
            Some(slot) => {
                if std::env::var_os("MUSAEUM_VISION_DEBUG").is_some() {
                    let (a, b) = (line.baseline[0], line.baseline[line.baseline.len() - 1]);
                    eprintln!(
                        "dropped {verdict:?}: ({:.0}, {:.0}) -> ({:.0}, {:.0}), x height {:.1}, ascent {:.1}, descent {:.1}",
                        a[0], a[1], b[0], b[1], line.x_height, line.ascent, line.descent
                    );
                }
                stats.dropped[slot] += 1;
            }
        }
    }
    out
}

/// The fewest long lines a column has for its edges to be measured, and how many of them, at
/// least, end within half a letter of the same place for the column to be justified.
const EDGE_LINES: usize = 4;
const JUSTIFIED: f32 = 0.6;

/// The level lines of one area cut back to the edges of their column, when it is justified: most
/// of its long lines start at one place and end at another, within half a letter. Ink a line
/// reaches past either edge, beyond a gap wider than the space between words, is not the line's:
/// a scribble or a mark in the margin, a speck of the scan. A column whose lines end raggedly (a
/// table of contents, verse) is left as it is.
fn to_column_edges(lines: &mut [PixelLine], glyph_height: f32) {
    let g = glyph_height.max(4.0);
    let (Some(left), Some(right)) = (
        lines.iter().map(|line| line.left).reduce(f32::min),
        lines.iter().map(|line| line.right).reduce(f32::max),
    ) else {
        return;
    };
    let long: Vec<&PixelLine> = lines
        .iter()
        .filter(|line| line.right - line.left >= (right - left) * 0.6)
        .collect();
    if long.len() < EDGE_LINES {
        return;
    }
    let edge = |of: &dyn Fn(&PixelLine) -> f32| -> Option<f32> {
        let mut at: Vec<f32> = long.iter().map(|line| of(line)).collect();
        at.sort_by(f32::total_cmp);
        let median = at[at.len() / 2];
        let near = at
            .iter()
            .filter(|&&x| (x - median).abs() <= g * 0.5)
            .count();
        (near as f32 >= long.len() as f32 * JUSTIFIED).then_some(median)
    };
    let (start, end) = (edge(&|line| line.left), edge(&|line| line.right));
    let wide = g * 0.6;
    for line in lines.iter_mut() {
        if line.gaps_are_arc {
            continue;
        }
        let extent = (line.right - line.left).max(1.0);
        let at = |fraction: f32| line.left + fraction * extent;
        // The first wide gap that opens at the column's right edge or past it, and the last that
        // closes at its left edge or before it.
        let cut_right = end
            .filter(|&end| line.right > end + g * 0.5)
            .and_then(|end| {
                line.gaps
                    .iter()
                    .find(|gap| at(gap[0]) >= end - g * 0.5 && at(gap[1]) - at(gap[0]) >= wide)
                    .map(|gap| at(gap[0]))
            });
        let cut_left = start
            .filter(|&start| line.left < start - g * 0.5)
            .and_then(|start| {
                line.gaps
                    .iter()
                    .rev()
                    .find(|gap| at(gap[1]) <= start + g * 0.5 && at(gap[1]) - at(gap[0]) >= wide)
                    .map(|gap| at(gap[1]))
            });
        if cut_right.is_none() && cut_left.is_none() {
            continue;
        }
        let from = cut_left.unwrap_or(line.left);
        let to = cut_right.unwrap_or(line.right);
        if to - from < g {
            continue;
        }
        let gaps: Vec<[f32; 2]> = line
            .gaps
            .iter()
            .map(|gap| [at(gap[0]), at(gap[1])])
            .filter(|gap| gap[0] > from && gap[1] < to)
            .map(|gap| [(gap[0] - from) / (to - from), (gap[1] - from) / (to - from)])
            .collect();
        let mut baseline = vec![[from, crate::orient::baseline_at(&line.baseline, from)]];
        baseline.extend(
            line.baseline
                .iter()
                .copied()
                .filter(|point| point[0] > from + 0.5 && point[0] < to - 0.5),
        );
        baseline.push([to, crate::orient::baseline_at(&line.baseline, to)]);
        line.baseline = baseline;
        line.gaps = gaps;
        line.left = from;
        line.right = to;
    }
}

/// The letters' mask without its underlines, for a page whose letters are `glyph_height` high.
fn erase_underlines(mask: &mut Bitmap, glyph_height: f32) {
    let g = glyph_height.max(4.0);
    mask.erase_underlines(
        ((g * UNDERLINE_THICKNESS).round() as usize).max(2),
        (g * UNDERLINE_LENGTH).round() as usize,
    );
}

/// The frame's mask, its areas' lines and its letters, in the frame's pixels.
fn trace_frame(
    frame: &Frame,
    labels: &[u32],
    width: u32,
    height: u32,
    keep: &[bool],
    page_glyph_height: f32,
) -> (Bitmap, Vec<Vec<PixelLine>>, Vec<Component>) {
    let mut mask = frame.mask(labels, width, height, keep);
    erase_underlines(&mut mask, page_glyph_height);
    let labeled = components::label(&mask);
    let glyphs: Vec<Component> = labeled
        .components
        .into_iter()
        .filter(|c| c.area >= 4 && c.height() >= 2)
        .collect();
    // The block's own letters may be smaller or larger than the page's.
    let own = components::typical_glyph_height(&glyphs, frame.width, frame.height);
    let glyph_height = if own > 0.0 { own } else { page_glyph_height };
    let skew = skew::estimate(
        &glyphs,
        frame.width as f32 / 2.0,
        (glyph_height / 6.0).max(1.0),
    );
    let (_, areas) = trace(&mask, &glyphs, glyph_height, skew, &[], &[]);
    (mask, areas, glyphs)
}

/// A line found in a frame, in pixels of the page.
fn to_page_line(frame: &Frame, mut line: PixelLine) -> PixelLine {
    // The gaps are fractions along the frame's x, which is along the line: keep them as fractions
    // of the arc before the points move.
    line.gaps = arc_gaps(&line);
    line.gaps_are_arc = true;
    line.baseline = line
        .baseline
        .iter()
        .map(|&[u, v]| frame.to_page(u, v))
        .collect();
    line.left = line.baseline.iter().map(|p| p[0]).fold(f32::MAX, f32::min);
    line.right = line.baseline.iter().map(|p| p[0]).fold(f32::MIN, f32::max);
    line
}

/// A line's gaps as fractions of its arc length, from fractions of its x-span.
fn arc_gaps(line: &PixelLine) -> Vec<[f32; 2]> {
    if line.gaps_are_arc {
        return line.gaps.clone();
    }
    let points = &line.baseline;
    let (x0, x1) = (points[0][0], points[points.len() - 1][0]);
    let span = x1 - x0;
    let lengths: Vec<f32> = points
        .windows(2)
        .map(|pair| (pair[1][0] - pair[0][0]).hypot(pair[1][1] - pair[0][1]))
        .collect();
    let total: f32 = lengths.iter().sum::<f32>().max(1e-3);
    let arc_at = |fraction: f32| -> f32 {
        let x = x0 + fraction * span;
        let mut walked = 0.0;
        for (index, pair) in points.windows(2).enumerate() {
            let (ax, bx) = (pair[0][0], pair[1][0]);
            if x <= bx || index + 2 == points.len() {
                let t = if bx > ax {
                    ((x - ax) / (bx - ax)).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                return (walked + lengths[index] * t) / total;
            }
            walked += lengths[index];
        }
        1.0
    };
    line.gaps
        .iter()
        .map(|gap| [arc_at(gap[0]), arc_at(gap[1])])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_turn_both_ways() {
        for rotation in 0..4 {
            let [u, v] = to_stored(0.2, 0.7, rotation);
            let [x, y] = to_shown(u, v, rotation);
            assert!((x - 0.2).abs() < 1e-6 && (y - 0.7).abs() < 1e-6);
        }
        assert_eq!(to_shown(0.0, 0.0, 1), [1.0, 0.0]);
    }

    #[test]
    fn a_blank_page_has_no_lines() {
        let gray = Gray::filled(300, 400, 235);
        let detection = detect(&gray, &DetectOptions::default());
        assert!(detection.model.lines.is_empty());
        assert_eq!(detection.stats.components, 0);
    }

    #[test]
    fn gaps_become_fractions_of_the_arc() {
        // Level for half its width, then down at 45°.
        let line = PixelLine {
            column: 0,
            baseline: vec![[0.0, 0.0], [100.0, 0.0], [200.0, 100.0]],
            x_height: 10.0,
            ascent: 14.0,
            descent: 4.0,
            gaps: vec![[0.5, 0.75]],
            left: 0.0,
            right: 200.0,
            gaps_are_arc: false,
        };
        let total = 100.0 + 100.0 * 2f32.sqrt();
        let arc = arc_gaps(&line);
        assert!((arc[0][0] - 100.0 / total).abs() < 1e-4);
        assert!((arc[0][1] - (100.0 + 50.0 * 2f32.sqrt()) / total).abs() < 1e-4);
    }

    fn level_line(y: f32, from: f32, to: f32, x_height: f32) -> PixelLine {
        PixelLine {
            column: 0,
            baseline: vec![[from, y], [to, y]],
            x_height,
            ascent: x_height * 1.4,
            descent: x_height * 0.3,
            gaps: Vec::new(),
            left: from,
            right: to,
            gaps_are_arc: false,
        }
    }

    #[test]
    fn a_short_line_between_lines_of_its_column_is_judged_by_their_height() {
        // As a line of italics measured too tall is: it fails at its own x-height and passes at
        // its column's.
        let judge = |line: &PixelLine| {
            if line.x_height > 13.0 {
                Verdict::NotAligned
            } else {
                Verdict::Text
            }
        };
        let lines = vec![
            level_line(100.0, 100.0, 500.0, 12.0),
            level_line(130.0, 120.0, 260.0, 15.0),
            level_line(160.0, 100.0, 480.0, 12.0),
            level_line(190.0, 100.0, 450.0, 12.0),
            // The same, alone below the column, with nothing under it.
            level_line(290.0, 120.0, 260.0, 15.0),
        ];
        let mut stats = Stats::default();
        let kept = keep_lines(lines, &judge, &mut stats);
        assert_eq!(kept.len(), 4, "{kept:#?}");
        assert_eq!(kept[1].x_height, 12.0);
        assert_eq!(stats.dropped[2], 1);
    }

    #[test]
    fn lines_of_a_justified_column_end_at_its_edges() {
        let g = 10.0;
        let mut lines: Vec<PixelLine> = (0..6)
            .map(|row| level_line(100.0 + row as f32 * 30.0, 100.0, 500.0, 8.0))
            .collect();
        // A scribble in the margin, past a gap of 25 pixels, taken into the second line.
        lines[1].right = 560.0;
        lines[1].baseline = vec![[100.0, 130.0], [560.0, 130.0]];
        lines[1].gaps = vec![
            [300.0 / 460.0, 306.0 / 460.0],
            [400.0 / 460.0, 425.0 / 460.0],
        ];
        to_column_edges(&mut lines, g);
        assert_eq!(lines[1].right, 500.0);
        assert_eq!(lines[1].baseline.last(), Some(&[500.0, 130.0]));
        assert_eq!(lines[1].gaps.len(), 1);
        assert!((lines[1].gaps[0][0] - 300.0 / 400.0).abs() < 1e-4);
        // A column whose lines end raggedly, as a table of contents does, has no edge to cut at.
        let mut ragged: Vec<PixelLine> = [500.0, 380.0, 460.0, 330.0, 420.0, 470.0]
            .into_iter()
            .enumerate()
            .map(|(row, right)| level_line(100.0 + row as f32 * 30.0, 100.0, right, 8.0))
            .collect();
        ragged[0].gaps = vec![[300.0 / 400.0, 330.0 / 400.0]];
        let before = ragged.clone();
        to_column_edges(&mut ragged, g);
        assert_eq!(ragged, before);
    }

    /// A page of the labeled corpus turned `quarters` clockwise, and its lines turned with it.
    fn turned_page(quarters: u8) -> (Gray, Vec<crate::synth::TruthLine>) {
        use crate::synth::{Layout, TruthLine, page};
        let upright = page(Layout::Straight, 3);
        let (w, h) = (upright.gray.width, upright.gray.height);
        let truth = upright
            .lines
            .iter()
            .map(|line| TruthLine {
                baseline: line
                    .baseline
                    .iter()
                    .map(|&point| Gray::quarter_point(point, quarters, w, h))
                    .collect(),
                ..line.clone()
            })
            .collect();
        (upright.gray.quarter_turned(quarters), truth)
    }

    /// The lines are the page's, all turned, and read up the page (turned 3) or down it (1).
    fn read_the_way_they_run(found: &Detection, truth: &[crate::synth::TruthLine], quarters: u8) {
        let result = crate::score::score(&found.pixel_lines, truth);
        assert!(
            result.f1() >= 0.95,
            "turned {quarters}: {result:?}, F1 {:.3}",
            result.f1()
        );
        assert!(
            found
                .model
                .lines
                .iter()
                .all(|line| line.flags & crate::model::LINE_TURNED != 0)
        );
        for line in &found.pixel_lines {
            let (first, last) = (line.baseline[0], line.baseline[line.baseline.len() - 1]);
            if quarters == 3 {
                assert!(first[1] > last[1], "{:?}", line.baseline);
            } else {
                assert!(first[1] < last[1], "{:?}", line.baseline);
            }
        }
    }

    #[test]
    fn a_page_on_its_side_is_read_as_turned_lines_the_right_way() {
        // Turned counter-clockwise its text runs up the page; clockwise, down it. Whether the
        // detector reads it in blocks or turns it, the lines are the same.
        for quarters in [3u8, 1] {
            let (gray, truth) = turned_page(quarters);
            let found = detect(&gray, &DetectOptions::default());
            read_the_way_they_run(&found, &truth, quarters);
        }
    }

    #[test]
    fn lines_found_on_a_page_turned_for_them_come_back_onto_the_page_as_shown() {
        let options = DetectOptions::default();
        for quarters in [3u8, 1] {
            let (gray, truth) = turned_page(quarters);
            // Turned back level, as the detector turns a page on its side, and analyzed there.
            let back = (4 - quarters) % 4;
            let analysis = analyze(&gray.quarter_turned(back), &options, false);
            let found = into_detection(analysis, back, gray.width, gray.height, 0);
            assert_eq!(found.stats.page_quarters, back);
            read_the_way_they_run(&found, &truth, quarters);
        }
    }

    #[test]
    fn letters_upside_down_say_so() {
        use crate::synth::{Layout, page};
        let options = DetectOptions::default();
        let upright = page(Layout::Straight, 3).gray;
        assert!(analyze(&upright, &options, false).upside_down <= -UPSIDE_DOWN_EVIDENCE);
        let upside_down = upright.quarter_turned(2);
        assert!(analyze(&upside_down, &options, false).upside_down >= UPSIDE_DOWN_EVIDENCE);
    }
}

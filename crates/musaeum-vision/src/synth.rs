//! Pages of a book drawn with their answers: a labeled corpus for measuring the line detector.
//!
//! Each page is a gray picture about the size the render workers draw for the detector (1500
//! pixels tall), with lines of letter-like shapes (stems, bowls, ascenders and descenders, word
//! spaces, paragraphs with an indented first line and a short last one, a running head and a
//! folio), on paper with grain and specks; and, for every line, where its baseline really is.
//! Eight layouts: straight, skewed, two columns, two columns above a photograph across the page with
//! one column below it (columns that are columns only for part of the page, and a picture that is
//! not text), curved toward the spine as a book pressed on a flatbed scanner is, its lines
//! bending and squeezed near the gutter, the paper darker there; crooked, the paper waved and
//! twisted so lines tilt and wave differently down the page, each a little on its own besides; turned, with blocks of text at 90°, 270°, 45° and −30° under a column; and
//! graphics, a column above a halftone plate ruled at its sides, a bar chart with its axes, grid,
//! ticks, labels at 45° and a title at 90°, and a ruled table read row by row across its cells,
//! whose lines and dots are no text.
//!
//! The shapes are drawn on the flat page and then carried onto the picture by the same warp that
//! gives the true baselines, so the answers are exact whatever the layout does to the page.

use crate::gray::Gray;

/// How a page is laid out and how the scan treated it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Layout {
    /// Square on the glass.
    Straight,
    /// Turned by up to three degrees.
    Skewed,
    /// Two columns with a gutter.
    TwoColumns,
    /// Two columns, a photograph across the page with its caption, one column below.
    Mixed,
    /// Bending toward the spine.
    Curved,
    /// Lines tilted and waving differently down the page, each a little on its own besides.
    Crooked,
    /// Blocks of text at 90°, 270°, 45° and −30° under a column.
    Turned,
    /// A halftone plate, a bar chart and a ruled table under a column.
    Graphics,
}

impl Layout {
    pub const ALL: [Layout; 8] = [
        Layout::Straight,
        Layout::Skewed,
        Layout::TwoColumns,
        Layout::Mixed,
        Layout::Curved,
        Layout::Crooked,
        Layout::Turned,
        Layout::Graphics,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Layout::Straight => "straight",
            Layout::Skewed => "skewed",
            Layout::TwoColumns => "two-columns",
            Layout::Mixed => "mixed",
            Layout::Curved => "curved",
            Layout::Crooked => "crooked",
            Layout::Turned => "turned",
            Layout::Graphics => "graphics",
        }
    }
}

/// A line as it really is on the picture.
#[derive(Clone, Debug, PartialEq)]
pub struct TruthLine {
    pub column: u16,
    /// The baseline, from the line's start to its end as it reads, in pixels of the picture.
    pub baseline: Vec<[f32; 2]>,
    pub x_height: f32,
    /// Whether it runs at another angle than the page's text.
    pub turned: bool,
}

/// A page and its answers.
#[derive(Clone, Debug)]
pub struct SynthPage {
    pub layout: Layout,
    pub seed: u64,
    pub gray: Gray,
    pub lines: Vec<TruthLine>,
    /// How far the page is turned, degrees; how deep the curve near the spine goes, pixels.
    pub skew_degrees: f32,
    pub curve_depth: f32,
    /// Where the page has pictures, rules and charts, in pixels: left, top, right, bottom. A found
    /// line there that is no true line is a graphic taken for text.
    pub graphics: Vec<[f32; 4]>,
}

/// xorshift64*, enough for drawing.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in `0..1`.
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }

    fn range(&mut self, low: f32, high: f32) -> f32 {
        low + (high - low) * self.unit()
    }

    fn below(&mut self, n: u32) -> u32 {
        (self.next() % u64::from(n.max(1))) as u32
    }
}

/// Carries a point of the flat page onto the picture.
#[derive(Clone, Copy, Debug)]
struct Warp {
    width: f32,
    height: f32,
    /// Radians, about the centre.
    skew: f32,
    /// Where the curve starts, as x on the flat page, and toward which edge the spine is.
    curve_from: f32,
    spine_right: bool,
    /// How far a line at the top or bottom edge bends by the spine, and how much the last part
    /// before the spine is squeezed, as a fraction of its width.
    depth: f32,
    squeeze: f32,
}

impl Warp {
    /// How far into the curve `x` is, 0 before it and 1 at the spine.
    fn curve_at(&self, x: f32) -> f32 {
        if self.depth == 0.0 {
            return 0.0;
        }
        let t = if self.spine_right {
            (x - self.curve_from) / (self.width - self.curve_from)
        } else {
            (self.curve_from - x) / self.curve_from
        };
        t.clamp(0.0, 1.0)
    }

    fn apply(&self, x: f32, y: f32) -> [f32; 2] {
        let mut x = x;
        let mut y = y;
        let t = self.curve_at(x);
        if t > 0.0 {
            // The paper sinks into the gutter: lines bend away from the middle of the page, more
            // the farther from it they are, and the last stretch is foreshortened.
            let middle = (y - self.height / 2.0) / (self.height / 2.0);
            y += self.depth * t * t * (0.35 + 0.65 * middle);
            let span = if self.spine_right {
                self.width - self.curve_from
            } else {
                self.curve_from
            };
            let squeezed = self.squeeze * span * t * t * t / 3.0;
            x += if self.spine_right {
                -squeezed
            } else {
                squeezed
            };
        }
        if self.skew != 0.0 {
            let (sin, cos) = self.skew.sin_cos();
            let (cx, cy) = (self.width / 2.0, self.height / 2.0);
            let (dx, dy) = (x - cx, y - cy);
            x = cx + dx * cos - dy * sin;
            y = cy + dx * sin + dy * cos;
        }
        [x, y]
    }
}

/// Coverage of ink on the picture, splatted from the flat page.
struct Canvas {
    width: usize,
    height: usize,
    ink: Vec<f32>,
}

impl Canvas {
    fn splat(&mut self, [x, y]: [f32; 2], amount: f32) {
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        for (dx, dy, weight) in [
            (0, 0, (1.0 - fx) * (1.0 - fy)),
            (1, 0, fx * (1.0 - fy)),
            (0, 1, (1.0 - fx) * fy),
            (1, 1, fx * fy),
        ] {
            let px = x0 as i64 + dx;
            let py = y0 as i64 + dy;
            if px >= 0 && py >= 0 && (px as usize) < self.width && (py as usize) < self.height {
                let at = py as usize * self.width + px as usize;
                self.ink[at] = (self.ink[at] + amount * weight).min(1.0);
            }
        }
    }

    /// Fills a rectangle of the flat page, sampled every half pixel and carried by `place`.
    fn fill(&mut self, place: Place<'_>, left: f32, top: f32, right: f32, bottom: f32) {
        const STEP: f32 = 0.5;
        let mut y = top;
        while y < bottom {
            let mut x = left;
            while x < right {
                self.splat(place(x, y), STEP * STEP * 1.6);
                x += STEP;
            }
            y += STEP;
        }
    }
}

impl Canvas {
    /// Like `fill`, with a fraction of the ink: a light gray line.
    fn fill_light(
        &mut self,
        place: Place<'_>,
        left: f32,
        top: f32,
        right: f32,
        bottom: f32,
        strength: f32,
    ) {
        const STEP: f32 = 0.5;
        let mut y = top;
        while y < bottom {
            let mut x = left;
            while x < right {
                self.splat(place(x, y), STEP * STEP * 1.6 * strength);
                x += STEP;
            }
            y += STEP;
        }
    }
}

/// Where a point of the flat page, or of a block's own frame, lands on the picture.
type Place<'a> = &'a dyn Fn(f32, f32) -> [f32; 2];

/// One letter: stems, bars, a bowl, an ascender or a descender, on a baseline at `y`.
fn letter(
    canvas: &mut Canvas,
    warp: Place<'_>,
    rng: &mut Rng,
    x: f32,
    y: f32,
    xh: f32,
    width: f32,
) {
    let stroke = (xh * 0.15).max(1.4);
    let top = y - xh;
    match rng.below(10) {
        // A bowl: o, e, c, a.
        0..=2 => {
            canvas.fill(warp, x, top, x + stroke, y);
            canvas.fill(warp, x + width - stroke, top, x + width, y);
            canvas.fill(warp, x, top, x + width, top + stroke);
            canvas.fill(warp, x, y - stroke, x + width, y);
        }
        // An arch: n, m, u, r.
        3..=5 => {
            canvas.fill(warp, x, top, x + stroke, y);
            canvas.fill(warp, x + width - stroke, top + stroke, x + width, y);
            canvas.fill(warp, x, top, x + width, top + stroke);
        }
        // An ascender: l, d, b, h, t, k.
        6..=7 => {
            let ascender = y - xh * 1.5;
            canvas.fill(warp, x, ascender, x + stroke, y);
            canvas.fill(warp, x, top, x + width, top + stroke);
            canvas.fill(warp, x + width - stroke, top, x + width, y);
        }
        // A descender: p, q, g, y.
        8 => {
            canvas.fill(warp, x, top, x + stroke, y + xh * 0.45);
            canvas.fill(warp, x, top, x + width, top + stroke);
            canvas.fill(warp, x + width - stroke, top, x + width, y);
            canvas.fill(warp, x, y - stroke, x + width, y);
        }
        // A thin letter: i, j, f; the dot apart from it.
        _ => {
            canvas.fill(warp, x, top, x + stroke, y);
            canvas.fill(warp, x, top - xh * 0.45, x + stroke, top - xh * 0.25);
        }
    }
}

/// Words along a line from `left` to at most `right`, returning where the ink ends.
#[allow(clippy::too_many_arguments)]
fn words(
    canvas: &mut Canvas,
    warp: Place<'_>,
    rng: &mut Rng,
    left: f32,
    right: f32,
    baseline: f32,
    xh: f32,
) -> f32 {
    let mut x = left;
    let mut end = left;
    loop {
        let letters = 1 + rng.below(9);
        let widths: Vec<f32> = (0..letters).map(|_| xh * rng.range(0.45, 0.8)).collect();
        let spacing = xh * 0.13;
        let word: f32 = widths.iter().sum::<f32>() + spacing * (letters - 1) as f32;
        if x + word > right {
            break;
        }
        for width in widths {
            letter(canvas, warp, rng, x, baseline, xh, width);
            x += width + spacing;
        }
        end = x - spacing;
        x = end + xh * rng.range(0.6, 0.85);
    }
    end
}

/// A block of text: from, to, and its columns, each from and to across the page.
type Block = (f32, f32, Vec<(f32, f32)>);

/// A page of `layout`, the same for the same seed.
pub fn page(layout: Layout, seed: u64) -> SynthPage {
    let mut rng = Rng::new(seed ^ (layout as u64) << 32);
    let (width, height) = (1060usize, 1500usize);
    let (w, h) = (width as f32, height as f32);
    let skew_degrees = match layout {
        Layout::Skewed => {
            let sign = if rng.below(2) == 0 { -1.0 } else { 1.0 };
            sign * rng.range(0.8, 3.0)
        }
        _ => rng.range(-0.15, 0.15),
    };
    let spine_right = rng.below(2) == 0;
    let curve_depth = if layout == Layout::Curved {
        rng.range(20.0, 70.0)
    } else {
        0.0
    };
    let warp = Warp {
        width: w,
        height: h,
        skew: skew_degrees.to_radians(),
        curve_from: if spine_right { w * 0.68 } else { w * 0.32 },
        spine_right,
        depth: curve_depth,
        squeeze: if layout == Layout::Curved { 0.5 } else { 0.0 },
    };
    let flat = |x: f32, y: f32| warp.apply(x, y);
    let mut canvas = Canvas {
        width,
        height,
        ink: vec![0.0; width * height],
    };

    let xh = rng.range(8.5, 12.5);
    let pitch = xh * rng.range(2.5, 3.0);
    let (margin_left, margin_right) = (w * rng.range(0.09, 0.13), w * rng.range(0.09, 0.13));
    let top = h * rng.range(0.08, 0.1);
    let bottom = h * 0.9;
    let gutter = w * rng.range(0.04, 0.06);
    let middle = (margin_left + w - margin_right) / 2.0;
    let two = vec![
        (margin_left, middle - gutter / 2.0),
        (middle + gutter / 2.0, w - margin_right),
    ];
    let one = vec![(margin_left, w - margin_right)];
    // Blocks of text, top to bottom: from, to, and their columns. A mixed page has a photograph
    // between its two blocks, and a caption under it; a turned or graphics page has its column at
    // the top and the rest below it.
    let mut photo: Option<[f32; 4]> = None;
    let mut caption: Option<f32> = None;
    let blocks: Vec<Block> = match layout {
        Layout::TwoColumns => vec![(top, bottom, two)],
        Layout::Mixed => {
            let split = top + (bottom - top) * rng.range(0.3, 0.45);
            let photo_top = split + pitch * 0.5;
            let photo_bottom = photo_top + (bottom - top) * rng.range(0.18, 0.26);
            photo = Some([margin_left, photo_top, w - margin_right, photo_bottom]);
            let caption_line = photo_bottom + pitch * 1.3;
            caption = Some(caption_line);
            vec![(top, split, two), (caption_line + pitch * 1.6, bottom, one)]
        }
        Layout::Turned => vec![(top, top + (bottom - top) * 0.4, one)],
        Layout::Graphics => vec![(top, top + (bottom - top) * 0.28, one)],
        _ => vec![(top, bottom, one)],
    };

    let mut lines: Vec<TruthLine> = Vec::new();
    let mut graphics: Vec<[f32; 4]> = Vec::new();

    // The running head, across the top, and the folio at the foot: short lines of their own.
    let head = top - pitch * 1.6;
    let head_left = w / 2.0 - w * rng.range(0.08, 0.15);
    let end = words(
        &mut canvas,
        &flat,
        &mut rng,
        head_left,
        w / 2.0 + w * 0.15,
        head,
        xh,
    );
    record(&mut lines, 0, false, &flat, (head_left, end), head, xh);
    let folio = bottom + pitch * 1.4;
    let end = words(
        &mut canvas,
        &flat,
        &mut rng,
        w / 2.0 - xh,
        w / 2.0 + xh * 2.0,
        folio,
        xh,
    );
    record(&mut lines, 0, false, &flat, (w / 2.0 - xh, end), folio, xh);

    if let (Some(line), Some([left, _, right, _])) = (caption, photo) {
        let stop = left + (right - left) * rng.range(0.4, 0.8);
        let end = words(&mut canvas, &flat, &mut rng, left, stop, line, xh);
        record(&mut lines, 0, false, &flat, (left, end), line, xh);
    }
    // A crooked page: the paper itself waved and twisted, so lines tilt and wave a little
    // differently down the page but neighbours stay nearly parallel, as a page photographed or
    // scanned unflattened does; and each line a little on its own besides.
    let field = (layout == Layout::Crooked).then(|| {
        let tau = std::f32::consts::TAU;
        let tilt = rng.range(0.6, 1.4).to_radians();
        let tilt_length = rng.range(500.0, 1200.0);
        let tilt_phase = rng.range(0.0, tau);
        // Waves up to half a letter high, long enough that a line never slopes more than about 3°
        // along them: what an unflattened page does away from the spine.
        let wave = xh * rng.range(0.3, 0.5);
        let wave_x = rng.range(650.0, 1100.0);
        let wave_y = rng.range(700.0, 1500.0);
        let wave_phase = rng.range(0.0, tau);
        move |x: f32, y: f32| -> f32 {
            (tilt * (y / tilt_length * tau + tilt_phase).sin()).tan() * (x - w / 2.0)
                + wave * (x / wave_x * tau + y / wave_y * tau + wave_phase).sin()
        }
    });
    let mut first_column = 0u16;
    for (from, to, columns) in &blocks {
        for (column, &(left, right)) in columns.iter().enumerate() {
            let column = first_column + column as u16;
            let mut baseline = from + xh;
            let mut paragraph_left = 0;
            while baseline < *to {
                // A paragraph's first line is indented, its last one ends short.
                let first = paragraph_left == 0;
                if first {
                    paragraph_left = 3 + rng.below(9);
                }
                paragraph_left -= 1;
                let last = paragraph_left == 0;
                let start = if first { left + xh * 2.5 } else { left };
                let stop = if last {
                    left + (right - left) * rng.range(0.25, 0.85)
                } else {
                    right
                };
                if let Some(field) = field {
                    let tilt = rng.range(-0.2, 0.2).to_radians().tan();
                    let amplitude = xh * rng.range(0.0, 0.12);
                    let wavelength = rng.range(200.0, 500.0);
                    let phase = rng.range(0.0, std::f32::consts::TAU);
                    let line_y = baseline;
                    let crooked = move |x: f32, y: f32| {
                        let along = (x - start) / wavelength * std::f32::consts::TAU + phase;
                        let own = (x - start) * tilt + amplitude * along.sin();
                        warp.apply(x, y + field(x, line_y) + own)
                    };
                    let end = words(&mut canvas, &crooked, &mut rng, start, stop, baseline, xh);
                    record(
                        &mut lines,
                        column,
                        false,
                        &crooked,
                        (start, end),
                        baseline,
                        xh,
                    );
                } else {
                    let end = words(&mut canvas, &flat, &mut rng, start, stop, baseline, xh);
                    record(&mut lines, column, false, &flat, (start, end), baseline, xh);
                }
                baseline += pitch;
            }
        }
        first_column += columns.len() as u16;
    }

    let below = top + (bottom - top) * 0.45;
    if layout == Layout::Turned {
        let size = xh * rng.range(0.8, 1.0);
        let block = |origin: [f32; 2], angle: f32, length: f32| Turned {
            origin,
            angle,
            length,
            count: 3,
            xh: size,
        };
        // Up the left margin, reading upward; down the right, reading downward; at 45°, rising,
        // in the middle; at −30°, falling, above it.
        for (column, turned) in [
            block([margin_left, bottom], 90.0, (bottom - below) * 0.95),
            block([w - margin_right, below], -90.0, (bottom - below) * 0.95),
            block([w * 0.3, bottom - h * 0.05], 45.0, w * 0.32),
            block([w * 0.32, below + h * 0.02], -30.0, w * 0.3),
        ]
        .into_iter()
        .enumerate()
        {
            turned.draw(&mut canvas, &mut rng, &mut lines, 1 + column as u16, &warp);
        }
    }
    // A halftone plate ruled at its sides, a bar chart and a ruled table.
    let mut plate: Option<[f32; 4]> = None;
    if layout == Layout::Graphics {
        let plate_rect = [
            margin_left + xh * 2.0,
            below - h * 0.12,
            w - margin_right - xh * 2.0,
            below + h * 0.06,
        ];
        plate = Some(plate_rect);
        graphics.push([
            plate_rect[0] - xh * 1.5,
            plate_rect[1],
            plate_rect[2] + xh * 1.5,
            plate_rect[3],
        ]);
        for x in [plate_rect[0] - xh * 1.2, plate_rect[2] + xh * 1.0] {
            canvas.fill(&flat, x, plate_rect[1], x + 2.2, plate_rect[3]);
        }
        let caption_line = plate_rect[3] + pitch * 1.2;
        let stop = plate_rect[0] + (plate_rect[2] - plate_rect[0]) * 0.6;
        let end = words(
            &mut canvas,
            &flat,
            &mut rng,
            plate_rect[0],
            stop,
            caption_line,
            xh * 0.85,
        );
        record(
            &mut lines,
            1,
            false,
            &flat,
            (plate_rect[0], end),
            caption_line,
            xh * 0.85,
        );
        // The chart: plot, grid, axes, bars; tick labels, labels at 45° under the bars, a title at
        // 90° beside them.
        let plot = [
            margin_left + xh * 6.0,
            caption_line + pitch * 2.0,
            w * 0.55,
            bottom - h * 0.07,
        ];
        graphics.push(plot);
        let label_xh = xh * 0.75;
        for step in 1..=4 {
            let y = plot[3] - (plot[3] - plot[1]) * step as f32 / 4.0;
            canvas.fill_light(&flat, plot[0], y, plot[2], y + 1.2, 0.45);
        }
        for step in 0..=4 {
            let y = plot[3] - (plot[3] - plot[1]) * step as f32 / 4.0;
            canvas.fill(&flat, plot[0] - xh * 0.6, y - 1.0, plot[0], y + 1.0);
            let label_right = plot[0] - xh * 1.0;
            let start = label_right - label_xh * 2.0;
            let baseline = y + label_xh * 0.5;
            let end = words(
                &mut canvas,
                &flat,
                &mut rng,
                start,
                label_right,
                baseline,
                label_xh,
            );
            record(
                &mut lines,
                5,
                false,
                &flat,
                (start, end),
                baseline,
                label_xh,
            );
        }
        canvas.fill(&flat, plot[0] - 1.0, plot[1], plot[0] + 1.0, plot[3]);
        canvas.fill(&flat, plot[0], plot[3] - 1.0, plot[2], plot[3] + 1.0);
        let bars = 5;
        let slot = (plot[2] - plot[0]) / bars as f32;
        for bar in 0..bars {
            let x = plot[0] + slot * bar as f32 + slot * 0.25;
            let tall = (plot[3] - plot[1]) * rng.range(0.3, 0.95);
            canvas.fill(&flat, x, plot[3] - tall, x + slot * 0.5, plot[3]);
            // Its label at 45°, rising to end under it.
            let length = label_xh * rng.range(3.0, 5.0);
            let (sin, cos) = 45f32.to_radians().sin_cos();
            let label = Turned {
                origin: [
                    x + slot * 0.25 - length * cos - label_xh,
                    plot[3] + label_xh * 1.5 + length * sin,
                ],
                angle: 45.0,
                length,
                count: 1,
                xh: label_xh,
            };
            label.draw(&mut canvas, &mut rng, &mut lines, 6 + bar as u16, &warp);
        }
        let title = Turned {
            origin: [margin_left, plot[3] - (plot[3] - plot[1]) * 0.1],
            angle: 90.0,
            length: (plot[3] - plot[1]) * 0.8,
            count: 1,
            xh: label_xh,
        };
        title.draw(&mut canvas, &mut rng, &mut lines, 12, &warp);
        // The table: rules around and between its cells, a word or two in each.
        let table = [
            w * 0.62,
            plot[1],
            w - margin_right,
            plot[1] + (plot[3] - plot[1]) * 0.7,
        ];
        graphics.push(table);
        let rows = 4;
        let columns = [
            table[0],
            table[0] + (table[2] - table[0]) * 0.4,
            table[0] + (table[2] - table[0]) * 0.7,
            table[2],
        ];
        let row_height = (table[3] - table[1]) / rows as f32;
        for row in 0..=rows {
            let y = table[1] + row_height * row as f32;
            canvas.fill(&flat, table[0], y - 1.0, table[2], y + 1.0);
        }
        for x in columns {
            canvas.fill(&flat, x - 1.0, table[1], x + 1.0, table[3]);
        }
        // A row is read across its cells, as a text layer reads it: one line from its first cell's
        // first word to its last cell's last.
        for row in 0..rows {
            let baseline = table[1] + row_height * (row as f32 + 0.5) + xh * 0.5;
            let mut last_end = 0.0;
            for column in 0..3 {
                let left = columns[column] + xh * 0.8;
                let right = columns[column + 1] - xh * 0.8;
                last_end = words(
                    &mut canvas,
                    &flat,
                    &mut rng,
                    left,
                    right,
                    baseline,
                    xh * 0.9,
                );
            }
            let first = columns[0] + xh * 0.8;
            record(
                &mut lines,
                13,
                false,
                &flat,
                (first, last_end),
                baseline,
                xh * 0.9,
            );
        }
    }

    // Paper with grain, darker toward the spine on a curved page; a few specks; the ink a little
    // soft, as a scan's is.
    let mut soft = vec![0.0f32; width * height];
    for y in 0..height {
        for x in 0..width {
            let mut sum = 0.0;
            let mut count = 0.0;
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let (px, py) = (x as i64 + dx, y as i64 + dy);
                    if px >= 0 && py >= 0 && (px as usize) < width && (py as usize) < height {
                        let weight = if dx == 0 && dy == 0 { 4.0 } else { 1.0 };
                        sum += canvas.ink[py as usize * width + px as usize] * weight;
                        count += weight;
                    }
                }
            }
            soft[y * width + x] = sum / count;
        }
    }
    let mut pixels = vec![0u8; width * height];
    let (screen_sin, screen_cos) = 45f32.to_radians().sin_cos();
    for y in 0..height {
        for x in 0..width {
            let shade = 1.0 - 0.45 * warp.curve_at(x as f32).powf(1.6);
            let paper = (rng.range(222.0, 238.0)) * shade;
            let ink = rng.range(25.0, 60.0);
            let coverage = (soft[y * width + x] * 1.25).min(1.0);
            pixels[y * width + x] =
                (paper * (1.0 - coverage) + ink * coverage).clamp(0.0, 255.0) as u8;
            if let Some([left, top, right, bottom]) = photo
                && (left..right).contains(&(x as f32))
                && (top..bottom).contains(&(y as f32))
            {
                // A photograph: soft light and shade, a few dark shapes, grain.
                let (u, v) = (
                    (x as f32 - left) / (right - left),
                    (y as f32 - top) / (bottom - top),
                );
                let light = 0.5
                    + 0.25 * (u * 5.0).sin() * (v * 3.0).cos()
                    + 0.15 * (u * 13.0 + v * 7.0).sin();
                let shape = ((u - 0.3).powi(2) + (v - 0.5).powi(2) < 0.04)
                    || ((u - 0.72).abs() < 0.06 && v > 0.2);
                let value = if shape { 45.0 } else { 70.0 + 140.0 * light };
                pixels[y * width + x] = (value + rng.range(-12.0, 12.0)).clamp(0.0, 255.0) as u8;
            }
            if let Some([left, top, right, bottom]) = plate
                && (left..right).contains(&(x as f32))
                && (top..bottom).contains(&(y as f32))
            {
                // A halftone plate: the shade printed as dots on a screen turned 45°, a dot to a
                // cell, as big as the shade is dark.
                let (u, v) = (
                    (x as f32 - left) / (right - left),
                    (y as f32 - top) / (bottom - top),
                );
                let dark = if (u - 0.6).abs() < 0.08 { 0.25 } else { 0.0 };
                let tone = (0.5 + 0.3 * (u * 7.0).sin() * (v * 4.0).cos() - dark).clamp(0.05, 0.95);
                let cell = 5.0;
                let rx = x as f32 * screen_cos - y as f32 * screen_sin;
                let ry = x as f32 * screen_sin + y as f32 * screen_cos;
                let fx = (rx / cell).rem_euclid(1.0) - 0.5;
                let fy = (ry / cell).rem_euclid(1.0) - 0.5;
                let dot = (fx * fx + fy * fy).sqrt() < ((1.0 - tone) / std::f32::consts::PI).sqrt();
                let value = if dot { 50.0 } else { 225.0 };
                pixels[y * width + x] = (value + rng.range(-10.0, 10.0)).clamp(0.0, 255.0) as u8;
            }
        }
    }
    for _ in 0..rng.below(120) + 40 {
        let (x, y) = (
            rng.below(width as u32) as usize,
            rng.below(height as u32) as usize,
        );
        pixels[y * width + x] = rng.range(30.0, 120.0) as u8;
    }

    SynthPage {
        layout,
        seed,
        gray: Gray {
            width: width as u32,
            height: height as u32,
            pixels,
        },
        lines,
        skew_degrees,
        curve_depth,
        graphics,
    }
}

/// A line from `span.0` to `span.1` along x of whatever frame `place` carries onto the picture,
/// its baseline at `baseline` in that frame.
#[allow(clippy::too_many_arguments)]
fn record(
    lines: &mut Vec<TruthLine>,
    column: u16,
    turned: bool,
    place: Place<'_>,
    span: (f32, f32),
    baseline: f32,
    xh: f32,
) {
    let (start, end) = span;
    if end <= start {
        return;
    }
    let steps = ((end - start) / 8.0).ceil().max(1.0) as usize;
    let points = (0..=steps)
        .map(|step| place(start + (end - start) * step as f32 / steps as f32, baseline))
        .collect();
    lines.push(TruthLine {
        column,
        baseline: points,
        x_height: xh,
        turned,
    });
}

/// A block of lines turned away from the page, in a frame of its own: x along its lines, y across
/// them downward, carried to the page from `origin` along `angle`, degrees as seen, 90 reading
/// upward.
struct Turned {
    origin: [f32; 2],
    angle: f32,
    length: f32,
    count: usize,
    xh: f32,
}

impl Turned {
    fn draw(
        &self,
        canvas: &mut Canvas,
        rng: &mut Rng,
        lines: &mut Vec<TruthLine>,
        column: u16,
        warp: &Warp,
    ) {
        let (sin, cos) = self.angle.to_radians().sin_cos();
        let origin = self.origin;
        // As seen, counterclockwise: with y down, the line's direction is (cos, −sin), and across
        // it downward, to the right of the direction, is (sin, cos).
        let place = move |x: f32, y: f32| {
            warp.apply(origin[0] + x * cos + y * sin, origin[1] - x * sin + y * cos)
        };
        let pitch = self.xh * 2.6;
        for line in 0..self.count {
            let baseline = self.xh + line as f32 * pitch;
            let stop = if line + 1 == self.count && self.count > 1 {
                self.length * rng.range(0.4, 0.9)
            } else {
                self.length
            };
            let end = words(canvas, &place, rng, 0.0, stop, baseline, self.xh);
            record(lines, column, true, &place, (0.0, end), baseline, self.xh);
        }
    }
}

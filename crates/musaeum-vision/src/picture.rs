//! Where a page has pictures rather than text: photographs, halftone plates, the texture of a
//! drawing. A grid of cells about a letter and a half wide; in each, the ink of components that sit
//! as letters do (`components::text_like`) against the ink of texture: figures with some body to
//! them, and blobs smaller than letters that do not sit as letters do. A cell is picture when most of its ink is not text and there is enough of it; the
//! cells are then smoothed so a stray letter-like dot in a photograph or a lone glyph-shaped smudge
//! does not decide anything, and areas too small to be a picture are dropped.

use crate::components::{Component, Shape};

/// The cells of a page that are picture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PictureMask {
    /// The side of a cell, in pixels.
    pub cell: u32,
    pub columns: u32,
    pub rows: u32,
    pub cells: Vec<bool>,
}

impl PictureMask {
    /// A mask with no picture.
    pub fn empty(width: u32, height: u32, cell: u32) -> Self {
        let cell = cell.max(1);
        let columns = width.div_ceil(cell).max(1);
        let rows = height.div_ceil(cell).max(1);
        Self {
            cell,
            columns,
            rows,
            cells: vec![false; (columns * rows) as usize],
        }
    }

    /// Whether the pixel (x, y) is in a picture.
    #[inline]
    pub fn at(&self, x: f32, y: f32) -> bool {
        if x < 0.0 || y < 0.0 {
            return false;
        }
        let column = (x as u32) / self.cell;
        let row = (y as u32) / self.cell;
        column < self.columns
            && row < self.rows
            && self.cells[(row * self.columns + column) as usize]
    }

    /// How many cells are picture.
    pub fn count(&self) -> usize {
        self.cells.iter().filter(|&&cell| cell).count()
    }
}

/// What is known of each component of a page, by its index.
#[derive(Clone, Copy)]
pub struct Parts<'a> {
    pub components: &'a [Component],
    /// What each component is.
    pub shapes: &'a [Shape],
    /// Whether it sits as a letter does.
    pub text: &'a [bool],
    /// Whether it is a dot of a halftone screen.
    pub lattice: &'a [bool],
}

/// The picture cells of a page. `labels` are the components' labels of every pixel;
/// `glyph_height` the typical letter.
pub fn find(
    width: u32,
    height: u32,
    labels: &[u32],
    parts: Parts<'_>,
    glyph_height: f32,
) -> PictureMask {
    let Parts {
        components,
        shapes,
        text,
        lattice,
    } = parts;
    let cell = (glyph_height * 1.5).round().max(6.0) as u32;
    let mut mask = PictureMask::empty(width, height, cell);
    let (columns, rows) = (mask.columns as usize, mask.rows as usize);
    let mut text_ink = vec![0u32; columns * rows];
    let mut other_ink = vec![0u32; columns * rows];
    // A component counts as texture when it is a figure with some body to it, or a blob that does
    // not sit as a letter does; rules, edges, specks and the thin lines of a frame or a table's grid
    // count for nothing, since pages of text have them too.
    let kind: Vec<u8> = components
        .iter()
        .enumerate()
        .map(|(index, c)| match shapes[index] {
            Shape::Figure if c.area as f32 >= c.width() as f32 * c.height() as f32 * 0.2 => 2,
            Shape::Figure => 0,
            Shape::Glyph if lattice[index] => 2,
            Shape::Glyph if text[index] => 1,
            // Dots of a screen and the grain of a photograph are smaller than letters; a letter
            // that merely has odd neighbours (lines that run close together) is no texture.
            Shape::Glyph if crate::near::size(c) < glyph_height * 0.8 => 2,
            _ => 0,
        })
        .collect();
    let w = width as usize;
    for y in 0..height as usize {
        let row = (y / cell as usize) * columns;
        for x in 0..w {
            let label = labels[y * w + x];
            if label == 0 {
                continue;
            }
            let at = row + x / cell as usize;
            match kind[label as usize - 1] {
                1 => text_ink[at] += 1,
                2 => other_ink[at] += 1,
                _ => {}
            }
        }
    }
    let area = (cell * cell) as f32;
    let raw: Vec<bool> = (0..columns * rows)
        .map(|at| {
            let other = other_ink[at] as f32;
            other > area * 0.12 && other > text_ink[at] as f32 * 2.0
        })
        .collect();
    // A majority of the 3 × 3 cells around, twice: fills the holes of a picture, removes lone cells.
    let mut smooth = raw;
    for _ in 0..2 {
        let mut next = vec![false; columns * rows];
        for row in 0..rows {
            for column in 0..columns {
                let mut count = 0;
                let mut total = 0;
                for dy in -1i64..=1 {
                    for dx in -1i64..=1 {
                        let (r, c) = (row as i64 + dy, column as i64 + dx);
                        if r >= 0 && c >= 0 && (r as usize) < rows && (c as usize) < columns {
                            total += 1;
                            count += usize::from(smooth[r as usize * columns + c as usize]);
                        }
                    }
                }
                next[row * columns + column] = count * 2 > total;
            }
        }
        smooth = next;
    }
    // Areas of fewer than a 3 × 3 block of cells are not pictures.
    let mut seen = vec![false; columns * rows];
    for start in 0..columns * rows {
        if !smooth[start] || seen[start] {
            continue;
        }
        let mut stack = vec![start];
        let mut members = Vec::new();
        seen[start] = true;
        while let Some(at) = stack.pop() {
            members.push(at);
            let (row, column) = (at / columns, at % columns);
            let mut push = |r: usize, c: usize| {
                let next = r * columns + c;
                if smooth[next] && !seen[next] {
                    seen[next] = true;
                    stack.push(next);
                }
            };
            if row > 0 {
                push(row - 1, column);
            }
            if row + 1 < rows {
                push(row + 1, column);
            }
            if column > 0 {
                push(row, column - 1);
            }
            if column + 1 < columns {
                push(row, column + 1);
            }
        }
        if members.len() < 9 {
            continue;
        }
        // A picture in a book is a rectangle: one that fills most of its box gets the whole box,
        // light corners and pale stretches of a plate included; no more, since its caption is
        // often a line below it.
        let (mut r0, mut c0, mut r1, mut c1) = (rows, columns, 0, 0);
        for &at in &members {
            let (row, column) = (at / columns, at % columns);
            r0 = r0.min(row);
            c0 = c0.min(column);
            r1 = r1.max(row);
            c1 = c1.max(column);
        }
        let boxed = (r1 - r0 + 1) * (c1 - c0 + 1);
        if members.len() * 10 >= boxed * 6 {
            for row in r0..=r1 {
                for column in c0..=c1 {
                    mask.cells[row * columns + column] = true;
                }
            }
        } else {
            for at in members {
                mask.cells[at] = true;
            }
        }
    }
    mask
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_of_texture_is_picture_and_a_letter_is_not() {
        let (width, height) = (120u32, 120u32);
        let mut labels = vec![0u32; (width * height) as usize];
        let mut components = Vec::new();
        let mut shapes = Vec::new();
        let mut text = Vec::new();
        // Texture: a figure with body to it over the left half.
        for y in 0..120usize {
            for x in 0..60usize {
                if (x / 2 + y / 2) % 2 == 0 {
                    labels[y * 120 + x] = 1;
                }
            }
        }
        components.push(Component {
            left: 0,
            top: 0,
            right: 60,
            bottom: 120,
            area: 3600,
        });
        shapes.push(Shape::Figure);
        text.push(false);
        // A letter on the right.
        for y in 50..60usize {
            for x in 90..96usize {
                labels[y * 120 + x] = 2;
            }
        }
        components.push(Component {
            left: 90,
            top: 50,
            right: 96,
            bottom: 60,
            area: 60,
        });
        shapes.push(Shape::Glyph);
        text.push(true);
        let parts = Parts {
            components: &components,
            shapes: &shapes,
            text: &text,
            lattice: &[false, false],
        };
        let mask = find(width, height, &labels, parts, 8.0);
        assert!(mask.at(30.0, 60.0));
        assert!(!mask.at(92.0, 55.0));
        assert!(mask.count() > 0);
    }
}

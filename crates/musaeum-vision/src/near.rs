//! Components near each other: a grid of buckets over the page, by the centres of the components
//! put in it, so "what is within this distance" looks at a few buckets and not at every blob.

use crate::components::Component;

/// The centre of a component's box.
pub fn center(component: &Component) -> [f32; 2] {
    [
        (component.left + component.right) as f32 / 2.0,
        (component.top + component.bottom) as f32 / 2.0,
    ]
}

/// The larger side of a component's box: about a letter's height whatever way the letter is turned.
pub fn size(component: &Component) -> f32 {
    component.width().max(component.height()) as f32
}

/// The gap between two boxes along x and along y: zero where they overlap on that axis.
pub fn gaps(a: &Component, b: &Component) -> (f32, f32) {
    let gx = (a.left.max(b.left) as f32 - a.right.min(b.right) as f32).max(0.0);
    let gy = (a.top.max(b.top) as f32 - a.bottom.min(b.bottom) as f32).max(0.0);
    (gx, gy)
}

/// Buckets of component indices by where their centres are.
pub struct Grid {
    cell: f32,
    columns: usize,
    rows: usize,
    buckets: Vec<Vec<u32>>,
}

impl Grid {
    /// A grid with square cells of `cell` pixels over a page of `width` × `height`, holding the
    /// components whose indices `members` gives.
    pub fn new(
        components: &[Component],
        members: impl IntoIterator<Item = usize>,
        cell: f32,
        width: u32,
        height: u32,
    ) -> Self {
        let cell = cell.max(2.0);
        let columns = ((width as f32 / cell).ceil() as usize).max(1);
        let rows = ((height as f32 / cell).ceil() as usize).max(1);
        let mut buckets = vec![Vec::new(); columns * rows];
        for index in members {
            let [x, y] = center(&components[index]);
            let column = ((x / cell) as usize).min(columns - 1);
            let row = ((y / cell) as usize).min(rows - 1);
            buckets[row * columns + column].push(index as u32);
        }
        Self {
            cell,
            columns,
            rows,
            buckets,
        }
    }

    /// Every component in a bucket that a circle of `radius` around (x, y) touches: a superset of
    /// those within `radius`, which the caller narrows.
    pub fn around(&self, x: f32, y: f32, radius: f32, mut visit: impl FnMut(usize)) {
        let reach = |value: f32, count: usize| -> (usize, usize) {
            let low = ((value - radius) / self.cell).floor().max(0.0) as usize;
            let high = (((value + radius) / self.cell).floor().max(0.0) as usize).min(count - 1);
            (low.min(count - 1), high)
        };
        let (c0, c1) = reach(x, self.columns);
        let (r0, r1) = reach(y, self.rows);
        for row in r0..=r1 {
            for column in c0..=c1 {
                for &index in &self.buckets[row * self.columns + column] {
                    visit(index as usize);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neighbours_are_found_through_the_buckets() {
        let at = |x: u32, y: u32| Component {
            left: x,
            top: y,
            right: x + 4,
            bottom: y + 4,
            area: 16,
        };
        let components = [at(10, 10), at(20, 10), at(200, 200), at(14, 30)];
        let grid = Grid::new(&components, 0..components.len(), 16.0, 300, 300);
        let mut found = Vec::new();
        grid.around(12.0, 12.0, 25.0, |index| found.push(index));
        found.sort_unstable();
        assert!(found.contains(&1) && found.contains(&3) && !found.contains(&2));
        assert_eq!(gaps(&components[0], &components[1]), (6.0, 0.0));
        assert_eq!(size(&components[0]), 4.0);
    }
}

//! How much the page is turned: the angle at which the bottoms of the letters line up best.
//!
//! The bottoms of the glyphs are projected onto a vertical axis after turning them by each
//! candidate angle; where the lines of text are level, the projection is a comb of sharp peaks,
//! and the sum of its squares is largest. A coarse sweep over the whole range, then a fine one
//! around its best.

use crate::components::Component;

/// The most a page is expected to be turned, in degrees, either way.
pub const MAX_SKEW_DEGREES: f32 = 5.0;

/// The skew of a page, in radians, from the bottoms of its glyphs: positive when lines go down
/// to the right. `center_x` is where the turn is measured from, usually the middle of the page.
pub fn estimate(glyphs: &[Component], center_x: f32, bin: f32) -> f32 {
    if glyphs.len() < 8 {
        return 0.0;
    }
    let points: Vec<(f32, f32)> = glyphs
        .iter()
        .map(|c| ((c.left + c.right) as f32 / 2.0 - center_x, c.bottom as f32))
        .collect();
    let bin = bin.max(1.0);
    let sharpness = |angle: f32| -> f64 {
        let slope = angle.tan();
        let mut counts: std::collections::HashMap<i32, u32> = std::collections::HashMap::new();
        for &(x, y) in &points {
            let level = y - x * slope;
            *counts.entry((level / bin).floor() as i32).or_insert(0) += 1;
        }
        counts.values().map(|&n| f64::from(n) * f64::from(n)).sum()
    };
    let limit = MAX_SKEW_DEGREES.to_radians();
    let mut best = (sharpness(0.0), 0.0f32);
    let coarse = 0.25f32.to_radians();
    let steps = (limit / coarse).round() as i32;
    for step in -steps..=steps {
        let angle = step as f32 * coarse;
        let score = sharpness(angle);
        if score > best.0 {
            best = (score, angle);
        }
    }
    let fine = 0.05f32.to_radians();
    let around = best.1;
    for step in -5..=5 {
        let angle = around + step as f32 * fine;
        let score = sharpness(angle);
        if score > best.0 {
            best = (score, angle);
        }
    }
    best.1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_angle_that_levels_the_lines_is_found() {
        // Letters on 20 lines of a page turned by 1.5 degrees.
        let angle = 1.5f32.to_radians();
        let mut glyphs = Vec::new();
        for line in 0..20 {
            for word in 0..40 {
                let x = 100.0 + word as f32 * 20.0;
                let bottom = 100.0 + line as f32 * 30.0 + (x - 500.0) * angle.tan();
                glyphs.push(Component {
                    left: x as u32,
                    top: bottom as u32 - 10,
                    right: x as u32 + 8,
                    bottom: bottom.round() as u32,
                    area: 40,
                });
            }
        }
        let found = estimate(&glyphs, 500.0, 2.0);
        assert!(
            (found - angle).abs() < 0.1f32.to_radians(),
            "{}",
            found.to_degrees()
        );
        assert_eq!(estimate(&glyphs[..3], 500.0, 2.0), 0.0, "too few to say");
    }
}

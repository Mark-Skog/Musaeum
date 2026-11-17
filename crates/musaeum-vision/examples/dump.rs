//! Draws a page of the labeled corpus with what the detector made of it, for looking at:
//!
//! ```text
//! cargo run --release -p musaeum-vision --example dump -- LAYOUT SEED OUT.ppm
//! ```
//!
//! The page in gray, the picture mask tinted blue, the true baselines green and the found ones
//! red, each found line with a dot where it starts.

use musaeum_vision::synth::{Layout, page};
use musaeum_vision::{DetectOptions, detect};

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [layout, seed, out] = args.as_slice() else {
        return Err("LAYOUT SEED OUT.ppm".to_owned());
    };
    // A PGM picture instead of a layout: no answers, only what was found.
    let synthetic = if layout.ends_with(".pgm") {
        let bytes = std::fs::read(layout).map_err(|error| error.to_string())?;
        let text = String::from_utf8_lossy(&bytes[..64.min(bytes.len())]).into_owned();
        let fields: Vec<&str> = text.split_whitespace().take(4).collect();
        let (w, h): (u32, u32) = (
            fields[1].parse().map_err(|_| "w")?,
            fields[2].parse().map_err(|_| "h")?,
        );
        let header = bytes.len() - (w * h) as usize;
        let mut synthetic = page(Layout::Straight, 0);
        synthetic.gray = musaeum_vision::Gray {
            width: w,
            height: h,
            pixels: bytes[header..].to_vec(),
        };
        synthetic.lines.clear();
        synthetic
    } else {
        let layout = Layout::ALL
            .into_iter()
            .find(|l| l.name() == layout)
            .ok_or("no such layout")?;
        let seed: u64 = seed.parse().map_err(|_| "SEED")?;
        page(layout, seed)
    };
    let found = detect(&synthetic.gray, &DetectOptions::default());
    let (w, h) = (
        synthetic.gray.width as usize,
        synthetic.gray.height as usize,
    );
    let mut rgb: Vec<[u8; 3]> = synthetic.gray.pixels.iter().map(|&v| [v, v, v]).collect();
    let quarters = found.stats.page_quarters;
    for y in 0..h {
        for x in 0..w {
            let [px, py] = musaeum_vision::Gray::quarter_point(
                [x as f32 + 0.5, y as f32 + 0.5],
                quarters,
                w as u32,
                h as u32,
            );
            if found.pictures.at(px, py) {
                let p = &mut rgb[y * w + x];
                p[2] = p[2].saturating_add(60);
                p[0] = p[0].saturating_sub(30);
            }
        }
    }
    let mut plot = |x: f32, y: f32, color: [u8; 3]| {
        if x >= 0.0 && y >= 0.0 && (x as usize) < w && (y as usize) < h {
            rgb[y as usize * w + x as usize] = color;
        }
    };
    // The bands and their columns, in orange.
    for band in &found.regions {
        for area in band {
            for x in area.left..area.right.min(w as u32) {
                for y in [area.top, area.bottom.saturating_sub(1).min(h as u32 - 1)] {
                    plot(x as f32, y as f32, [255, 140, 0]);
                }
            }
            for y in area.top..area.bottom.min(h as u32) {
                for x in [area.left, area.right.saturating_sub(1).min(w as u32 - 1)] {
                    plot(x as f32, y as f32, [255, 140, 0]);
                }
            }
        }
    }
    let mut line = |points: &[[f32; 2]], color: [u8; 3], offset: f32| {
        for pair in points.windows(2) {
            let ([ax, ay], [bx, by]) = (pair[0], pair[1]);
            let steps = ((bx - ax).hypot(by - ay) * 2.0).ceil().max(1.0) as usize;
            for step in 0..=steps {
                let t = step as f32 / steps as f32;
                plot(ax + (bx - ax) * t, ay + (by - ay) * t + offset, color);
            }
        }
        if let Some(&[x, y]) = points.first() {
            for dy in -2..=2 {
                for dx in -2..=2 {
                    plot(x + dx as f32, y + dy as f32 + offset, color);
                }
            }
        }
    };
    for truth in &synthetic.lines {
        line(&truth.baseline, [0, 170, 0], 1.0);
    }
    for found_line in &found.pixel_lines {
        line(&found_line.baseline, [220, 0, 0], 0.0);
    }
    // With MUSAEUM_VISION_LINES set, every line found, by its points.
    if std::env::var_os("MUSAEUM_VISION_LINES").is_some() {
        for (index, found_line) in found.pixel_lines.iter().enumerate() {
            let points: Vec<String> = found_line
                .baseline
                .iter()
                .map(|p| format!("({:.0}, {:.0})", p[0], p[1]))
                .collect();
            eprintln!(
                "line {index}: x height {:.1}: {}",
                found_line.x_height,
                points.join(" ")
            );
        }
    }
    let mut file = format!("P6\n{w} {h}\n255\n").into_bytes();
    for p in rgb {
        file.extend_from_slice(&p);
    }
    std::fs::write(out, file).map_err(|error| error.to_string())?;
    eprintln!(
        "{} true, {} found; blocks {:?}; dropped {:?}; glyph {:.1}",
        synthetic.lines.len(),
        found.pixel_lines.len(),
        found.stats.turned_blocks,
        found.stats.dropped,
        found.stats.glyph_height
    );
    Ok(())
}

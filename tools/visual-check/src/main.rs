//! Holds what the reader's window shows against a reference drawn without tiles.
//!
//! ```text
//! visual-check compare --document PATH --page N --scale S --shot PNG --x X --y Y [--diff PNG]
//! visual-check tiles --document PATH --pages N,N... [--width PX] [--diff DIR]
//! visual-check colors --document colors.pdf [--scale S] [--shot PNG --x X --y Y]
//! ```
//!
//! `compare` draws page `N` whole at scale `S` (in units of 1/4096 pixel per point) with the
//! format's own library, in one call: no tiles, no progressive renderer, no compositor. It then
//! compares it with the page as a screenshot of the real window shows it, its top-left corner at
//! device pixel `(X, Y)`, and prints how far apart they are as JSON.
//!
//! `tiles` does the same without a window: it asks the real pool, with its sandboxed workers, for
//! every tile of each page at the scale that fits `PX` pixels across (1600 by default), puts the
//! tiles together, and compares the result with the page drawn whole. That covers tiling, the
//! progressive renderer and the shared memory; the window adds the compositor and the WebView.
//!
//! `colors` checks the patches of `gen-fixtures`' color page: RGB has to come out exactly, the
//! process primaries close to what the SWOP profile gives, and the rest is reported as measured.
//! With a screenshot, it also checks that the window shows the colors PDFium drew.
//!
//! This is a developer's tool. It runs the format libraries in its own process, without the
//! sandbox the reader puts them in, so it is only ever pointed at fixtures and at books the user
//! already reads.

use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use std::collections::HashMap;
use std::time::Duration;

use gen_fixtures::color::{COLOR_PATCHES, Expected, OVERPRINT_OVERLAP, delta_e_2000};
use musaeum_core::{FixedFormat, encode_demand};
use musaeum_render_djvu::book::Book;
use musaeum_render_pdf::engine::{Document, Pdfium};
use musaeum_render_raster::comic::Comic;
use musaeum_shm::tile::{SCALE_ONE, page_pixels, tile_grid};
use musaeum_shm::{TILE_SIZE, TileKey};
use musaeum_worker::harness;
use serde_json::{Value, json};

/// An RGBA image, rows packed.
struct Picture {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Picture {
    fn rgb(&self, x: u32, y: u32) -> Option<[u8; 3]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let at = ((y * self.width + x) * 4) as usize;
        Some([self.pixels[at], self.pixels[at + 1], self.pixels[at + 2]])
    }

    /// The mean color of the `size` × `size` block centered on `(x, y)`.
    fn mean(&self, x: u32, y: u32, size: u32) -> Option<[u8; 3]> {
        let half = size / 2;
        let mut sum = [0u64; 3];
        let mut count = 0u64;
        for dy in 0..size {
            for dx in 0..size {
                let pixel = self.rgb((x + dx).checked_sub(half)?, (y + dy).checked_sub(half)?)?;
                for channel in 0..3 {
                    sum[channel] += u64::from(pixel[channel]);
                }
                count += 1;
            }
        }
        Some(sum.map(|total| ((total + count / 2) / count) as u8))
    }
}

fn arguments() -> Result<(String, Vec<(String, String)>), String> {
    let mut args = std::env::args().skip(1);
    let command = args.next().ok_or("say compare or colors")?;
    let mut pairs = Vec::new();
    while let Some(flag) = args.next() {
        let value = args.next().ok_or_else(|| format!("{flag} needs a value"))?;
        pairs.push((flag, value));
    }
    Ok((command, pairs))
}

fn option<'a>(pairs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(flag, _)| flag == name)
        .map(|(_, value)| value.as_str())
}

fn number<T: std::str::FromStr>(pairs: &[(String, String)], name: &str) -> Result<T, String> {
    option(pairs, name)
        .ok_or_else(|| format!("{name} is required"))?
        .parse()
        .map_err(|_| format!("{name} is not a number"))
}

/// Draws page `page` of `path` whole, with the library its format needs.
fn reference(path: &Path, page: u32, scale: u32) -> Result<Picture, String> {
    let format = FixedFormat::of_file(path)
        .map_err(|error| format!("{}: {error}", path.display()))?
        .ok_or("not a PDF, CBZ, CBR or DjVu file")?;
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let drawn = match format {
        FixedFormat::Pdf => {
            let dll = std::env::current_exe()
                .map_err(|error| error.to_string())?
                .with_file_name("pdfium.dll");
            let pdfium =
                Pdfium::load(&dll).map_err(|error| format!("{}: {error}", dll.display()))?;
            let mut document = Document::open(&pdfium, &bytes)
                .map_err(|error| format!("PDFium error {}", error.pdfium_error))?;
            document.render_page(page, scale)
        }
        FixedFormat::Comic => Comic::open(&bytes)
            .map_err(|error| format!("{error:?}"))?
            .render_page(page, scale),
        FixedFormat::Djvu => Book::open(&bytes)
            .map_err(|error| format!("{error:?}"))?
            .render_page(page, scale),
    };
    let (width, height, pixels) = drawn.map_err(|failure| failure.to_string())?;
    Ok(Picture {
        width,
        height,
        pixels,
    })
}

fn read_png(path: &Path) -> Result<Picture, String> {
    let file = File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut decoder = png::Decoder::new(BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().map_err(|error| error.to_string())?;
    let size = reader.output_buffer_size().ok_or("the PNG is too large")?;
    let mut buffer = vec![0u8; size];
    let info = reader
        .next_frame(&mut buffer)
        .map_err(|error| error.to_string())?;
    buffer.truncate(info.buffer_size());
    let pixels = match info.color_type {
        png::ColorType::Rgba => buffer,
        png::ColorType::Rgb => buffer
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|pixel| [pixel[0], pixel[1], pixel[2], 255])
            .collect(),
        other => return Err(format!("unexpected PNG color type {other:?}")),
    };
    Ok(Picture {
        width: info.width,
        height: info.height,
        pixels,
    })
}

fn write_gray_png(path: &Path, width: u32, height: u32, gray: &[u8]) -> Result<(), String> {
    let file = File::create(path).map_err(|error| error.to_string())?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .and_then(|mut writer| writer.write_image_data(gray))
        .map_err(|error| error.to_string())
}

/// Writes `[x, y, width, height]` of both pictures side by side, each pixel `zoom` times larger,
/// the tiles on the left and the reference on the right: for looking at a difference.
fn write_crop(
    path: &Path,
    left: &Picture,
    right: &Picture,
    [x, y, width, height]: [u32; 4],
    zoom: u32,
) -> Result<(), String> {
    let (out_width, out_height) = (width * zoom * 2 + zoom, height * zoom);
    let mut rgb = vec![255u8; (out_width * out_height * 3) as usize];
    for (side, picture) in [left, right].into_iter().enumerate() {
        for dy in 0..height {
            for dx in 0..width {
                let color = picture.rgb(x + dx, y + dy).unwrap_or([255, 0, 255]);
                for zy in 0..zoom {
                    for zx in 0..zoom {
                        let ox = side as u32 * (width * zoom + zoom) + dx * zoom + zx;
                        let at = (((dy * zoom + zy) * out_width + ox) * 3) as usize;
                        rgb[at..at + 3].copy_from_slice(&color);
                    }
                }
            }
        }
    }
    let file = File::create(path).map_err(|error| error.to_string())?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), out_width, out_height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .and_then(|mut writer| writer.write_image_data(&rgb))
        .map_err(|error| error.to_string())
}

fn compare(pairs: &[(String, String)]) -> Result<Value, String> {
    let document = PathBuf::from(option(pairs, "--document").ok_or("--document is required")?);
    let page: u32 = number(pairs, "--page")?;
    let scale: u32 = number(pairs, "--scale")?;
    let shot = read_png(Path::new(
        option(pairs, "--shot").ok_or("--shot is required")?,
    ))?;
    let (left, top): (i64, i64) = (number(pairs, "--x")?, number(pairs, "--y")?);
    let reference = reference(&document, page, scale)?;
    let mut report = measure(&reference, &shot, left, top, option(pairs, "--diff"))?;
    report["document"] = json!(document.display().to_string());
    report["page"] = json!(page);
    report["scale"] = json!(scale);
    Ok(report)
}

/// How far `shown`, with the page's corner at `(left, top)`, is from `reference`.
fn measure(
    reference: &Picture,
    shot: &Picture,
    left: i64,
    top: i64,
    diff: Option<&str>,
) -> Result<Value, String> {
    // The last column and row of a page may be only partly covered by it, so they blend with
    // the background and are left out.
    let (width, height) = (
        reference.width.saturating_sub(1),
        reference.height.saturating_sub(1),
    );
    let mut differences = vec![0u8; (width * height) as usize];
    let (mut compared, mut outside, mut sum, mut worst) = (0u64, 0u64, 0u64, 0u8);
    let (mut over_2, mut over_8, mut squared) = (0u64, 0u64, 0f64);
    for y in 0..height {
        for x in 0..width {
            let (sx, sy) = (left + i64::from(x), top + i64::from(y));
            let shown = u32::try_from(sx)
                .ok()
                .zip(u32::try_from(sy).ok())
                .and_then(|(sx, sy)| shot.rgb(sx, sy));
            let Some(shown) = shown else {
                outside += 1;
                continue;
            };
            let expected = reference.rgb(x, y).expect("inside the reference");
            let difference = (0..3)
                .map(|channel| shown[channel].abs_diff(expected[channel]))
                .max()
                .unwrap_or(0);
            for channel in 0..3 {
                let delta = f64::from(shown[channel]) - f64::from(expected[channel]);
                squared += delta * delta;
            }
            differences[(y * width + x) as usize] = difference.saturating_mul(8);
            compared += 1;
            sum += u64::from(difference);
            worst = worst.max(difference);
            over_2 += u64::from(difference > 2);
            over_8 += u64::from(difference > 8);
        }
    }
    if let Some(diff) = diff {
        write_gray_png(Path::new(diff), width, height, &differences)?;
    }
    let mse = squared / (compared.max(1) as f64 * 3.0);
    Ok(json!({
        "width": reference.width,
        "height": reference.height,
        "compared": compared,
        "outside_the_shot": outside,
        "max_difference": worst,
        "mean_difference": sum as f64 / compared.max(1) as f64,
        "share_over_2": over_2 as f64 / compared.max(1) as f64,
        "share_over_8": over_8 as f64 / compared.max(1) as f64,
        "identical": compared > 0 && worst == 0,
        "psnr_db": if mse == 0.0 { None } else { Some(10.0 * (255.0f64 * 255.0 / mse).log10()) },
    }))
}

/// The scale at which a page `width_pt` wide is `pixels` across, as a reader would see it.
fn fitting_scale(width_pt: f32, pixels: u32) -> u32 {
    let scale = (f64::from(pixels) / f64::from(width_pt.max(1.0)) * f64::from(SCALE_ONE)).round();
    (scale as u32).clamp(SCALE_ONE / 16, SCALE_ONE * 8)
}

fn tiles(pairs: &[(String, String)]) -> Result<Value, String> {
    let document = PathBuf::from(option(pairs, "--document").ok_or("--document is required")?);
    let pages: Vec<u32> = option(pairs, "--pages")
        .ok_or("--pages is required")?
        .split(',')
        .map(|page| page.trim().parse().map_err(|_| format!("bad page {page}")))
        .collect::<Result<_, _>>()?;
    let width: u32 = option(pairs, "--width").map_or(Ok(1600), |_| number(pairs, "--width"))?;
    let format = FixedFormat::of_file(&document)
        .map_err(|error| error.to_string())?
        .ok_or("not a PDF, CBZ, CBR or DjVu file")?;
    let worker = std::env::current_exe()
        .map_err(|error| error.to_string())?
        .with_file_name(format.worker_exe());
    let (pool, arrived) = harness::pull_pool(&worker, &document, format.policy(), 2, 32);
    let sizes = pool.pages().to_vec();

    let mut results = Vec::new();
    for (epoch, &page) in (1u32..).zip(&pages) {
        let size = *sizes
            .get(page as usize)
            .ok_or_else(|| format!("no page {page}"))?;
        let scale = fitting_scale(size.width_pt, width);
        let (columns, rows) = tile_grid(size, scale);
        let wanted: Vec<TileKey> = (0..rows)
            .flat_map(|y| {
                (0..columns).map(move |x| TileKey {
                    page,
                    scale,
                    x: x as u16,
                    y: y as u16,
                })
            })
            .collect();
        pool.submit_demand(encode_demand(epoch, &wanted));
        let mut received: HashMap<(u16, u16), Vec<u8>> = HashMap::new();
        while received.len() < wanted.len() {
            let tile = arrived
                .recv_timeout(Duration::from_secs(120))
                .map_err(|_| format!("page {page}: the tiles did not all arrive"))?;
            let key = tile.tile;
            if key.page == page && key.scale == scale {
                received.insert((key.x as u16, key.y as u16), tile.pixels);
            }
        }

        // The tiles, put back together into the page.
        let (page_width, page_height) = (
            page_pixels(size.width_pt, scale) as u32,
            page_pixels(size.height_pt, scale) as u32,
        );
        let mut composed = vec![0u8; page_width as usize * page_height as usize * 4];
        for ((x, y), pixels) in &received {
            let (left, top) = (u32::from(*x) * TILE_SIZE, u32::from(*y) * TILE_SIZE);
            let columns = (page_width - left).min(TILE_SIZE) as usize;
            for row in 0..(page_height - top).min(TILE_SIZE) as usize {
                let from = row * TILE_SIZE as usize * 4;
                let to = ((top as usize + row) * page_width as usize + left as usize) * 4;
                composed[to..to + columns * 4].copy_from_slice(&pixels[from..from + columns * 4]);
            }
        }
        let shown = Picture {
            width: page_width,
            height: page_height,
            pixels: composed,
        };
        let reference = reference(&document, page, scale)?;
        let diff = option(pairs, "--diff").map(|dir| {
            Path::new(dir)
                .join(format!("page-{page}.png"))
                .display()
                .to_string()
        });
        let mut report = measure(&reference, &shown, 0, 0, diff.as_deref())?;
        if let (Some(dir), Some(crop)) = (option(pairs, "--diff"), option(pairs, "--crop")) {
            let numbers: Vec<u32> = crop.split(',').filter_map(|n| n.parse().ok()).collect();
            if let [x, y, w, h] = numbers[..] {
                let path = Path::new(dir).join(format!("page-{page}-crop.png"));
                write_crop(&path, &shown, &reference, [x, y, w, h], 12)?;
            }
        }
        report["page"] = json!(page);
        report["scale"] = json!(scale);
        report["tiles"] = json!(wanted.len());
        results.push(report);
    }
    let stats = pool.stats().unwrap_or_default();
    pool.shutdown();
    Ok(json!({
        "document": document.display().to_string(),
        "format": format,
        "pages": results,
        "render_us": stats.render_us,
        "worker_failures": stats.tiles_failed,
        "worker_restarts": stats.worker_restarts,
    }))
}

fn colors(pairs: &[(String, String)]) -> Result<Value, String> {
    let document = PathBuf::from(option(pairs, "--document").ok_or("--document is required")?);
    let scale: u32 = option(pairs, "--scale").map_or(Ok(4096), |_| number(pairs, "--scale"))?;
    let drawn = reference(&document, 0, scale)?;
    let shot = match option(pairs, "--shot") {
        Some(path) => Some((
            read_png(Path::new(path))?,
            number::<i64>(pairs, "--x")?,
            number::<i64>(pairs, "--y")?,
        )),
        None => None,
    };
    let per_point = f64::from(scale) / 4096.0;
    let center = |rect: [f32; 4]| {
        (
            ((f64::from(rect[0]) + f64::from(rect[2]) / 2.0) * per_point) as u32,
            ((f64::from(rect[1]) + f64::from(rect[3]) / 2.0) * per_point) as u32,
        )
    };
    let block = (16.0 * per_point).max(4.0) as u32;
    let in_shot = |x: u32, y: u32| -> Option<[u8; 3]> {
        let (picture, left, top) = shot.as_ref()?;
        let x = u32::try_from(*left + i64::from(x)).ok()?;
        let y = u32::try_from(*top + i64::from(y)).ok()?;
        picture.mean(x, y, block)
    };

    let mut rows = Vec::new();
    let mut passed = true;
    for patch in &COLOR_PATCHES {
        let (x, y) = center(patch.rect());
        let rendered = drawn.mean(x, y, block).ok_or("a patch is off the page")?;
        let (verdict, detail) = match patch.expected {
            Expected::Exact(expected, tolerance) => {
                let off = (0..3)
                    .map(|c| rendered[c].abs_diff(expected[c]))
                    .max()
                    .unwrap_or(0);
                (
                    Some(off <= tolerance),
                    json!({ "expected": expected, "max_channel_difference": off }),
                )
            }
            Expected::Profile(expected, tolerance) => {
                let delta = delta_e_2000(rendered, expected);
                (
                    Some(delta <= tolerance),
                    json!({ "expected": expected, "delta_e_2000": delta, "tolerance": tolerance }),
                )
            }
            Expected::Measured => (None, json!({})),
        };
        passed &= verdict.unwrap_or(true);
        let shown = in_shot(x, y);
        let window = shown.map(|shown| {
            json!({
                "shown": shown,
                "delta_e_2000_from_pdfium": delta_e_2000(shown, rendered),
                "max_channel_difference": (0..3).map(|c| shown[c].abs_diff(rendered[c])).max().unwrap_or(0),
            })
        });
        rows.push(json!({
            "patch": patch.name,
            "pdfium": rendered,
            "passed": verdict,
            "check": detail,
            "window": window,
        }));
    }
    let [ox, oy, ow, oh] = OVERPRINT_OVERLAP;
    let overlap = drawn
        .mean(
            ((f64::from(ox) + f64::from(ow) / 2.0) * per_point) as u32,
            ((f64::from(oy) + f64::from(oh) / 2.0) * per_point) as u32,
            block,
        )
        .ok_or("the overprint overlap is off the page")?;
    // Yellow over cyan: knocked out it stays yellow, overprinted it turns green.
    let knocked_out = delta_e_2000(overlap, [255, 242, 0]) < delta_e_2000(overlap, [0, 166, 81]);
    Ok(json!({
        "document": document.display().to_string(),
        "scale": scale,
        "passed": passed,
        "patches": rows,
        "overprint": { "overlap": overlap, "simulated": !knocked_out },
    }))
}

fn main() -> ExitCode {
    let result = arguments().and_then(|(command, pairs)| match command.as_str() {
        "compare" => compare(&pairs),
        "tiles" => tiles(&pairs),
        "colors" => colors(&pairs),
        other => Err(format!("unknown command {other}")),
    });
    match result {
        Ok(report) => {
            println!("{report}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("visual-check: {message}");
            ExitCode::FAILURE
        }
    }
}

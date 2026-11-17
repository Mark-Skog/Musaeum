//! Reads a PGM picture with the engine, outside any sandbox, for trying the OCR by hand:
//!
//! ```text
//! cargo run --release -p musaeum-tesseract --example recognize -- MODELS LANGUAGES PGM [PSM] [PPI]
//! ```
//!
//! `MODELS` is a folder of `.traineddata` files; `LANGUAGES` as Tesseract takes them (`por+eng`);
//! `PSM` is auto, block, line or sparse.

use std::time::Instant;

use musaeum_tesseract::{Engine, GrayImage, Segmentation, add_model};

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [models, languages, pgm, rest @ ..] = args.as_slice() else {
        return Err("MODELS LANGUAGES PGM [PSM] [PPI]".to_owned());
    };
    let mode = match rest.first().map(String::as_str) {
        None | Some("auto") => Segmentation::Auto,
        Some("block") => Segmentation::Block,
        Some("line") => Segmentation::Line,
        Some("sparse") => Segmentation::Sparse,
        Some(other) => return Err(format!("unknown mode {other}")),
    };
    let ppi: u32 = rest
        .get(1)
        .map_or(Ok(300), |value| value.parse().map_err(|_| "PPI"))?;
    for language in languages.split('+') {
        let path = std::path::Path::new(models).join(format!("{language}.traineddata"));
        let data = std::fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        add_model(language, &data)?;
    }
    let started = Instant::now();
    let mut engine = Engine::new(languages)?;
    eprintln!(
        "loaded in {:.0} ms",
        started.elapsed().as_secs_f64() * 1000.0
    );
    let bytes = std::fs::read(pgm).map_err(|error| format!("{pgm}: {error}"))?;
    // P5, width, height, 255, then the pixels.
    let mut fields = Vec::new();
    let mut at = 0;
    while fields.len() < 4 {
        while bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        let start = at;
        while !bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        fields.push(String::from_utf8_lossy(&bytes[start..at]).into_owned());
    }
    at += 1;
    let width: u32 = fields[1].parse().map_err(|_| "width")?;
    let height: u32 = fields[2].parse().map_err(|_| "height")?;
    let image = GrayImage {
        width,
        height,
        pixels: &bytes[at..],
    };
    let started = Instant::now();
    let words = engine.recognize(image, mode, ppi)?;
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    let mut line = u32::MAX;
    let mut text = String::new();
    for word in &words {
        if word.line != line {
            if !text.is_empty() {
                println!("{text}");
            }
            text.clear();
            line = word.line;
        } else {
            text.push(' ');
        }
        text.push_str(&word.text);
    }
    if !text.is_empty() {
        println!("{text}");
    }
    let mean = words.iter().map(|word| word.confidence).sum::<f32>() / words.len().max(1) as f32;
    eprintln!(
        "{} words in {ms:.0} ms, mean confidence {mean:.1}",
        words.len()
    );
    Ok(())
}

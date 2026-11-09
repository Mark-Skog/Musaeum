//! Writes, for a normalized book on disk, positions in each of its parts as the reader's CFI with
//! the text found there, so that a browser can resolve the same CFIs with foliate-js and compare.
//! The browser side is `tools/cfi-browser-check.mjs`.
//!
//! ```text
//! cargo run --release -p musaeum-flow --example cfi-samples -- BOOK_DIR OUT.json [PER_PART]
//! ```

use std::path::Path;

use musaeum_flow::cfi::{self, Cfi, Point, Step};
use musaeum_flow::locator;
use musaeum_flow::package::Manifest;
use musaeum_flow::xml;
use serde_json::json;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(dir), Some(out)) = (args.first(), args.get(1)) else {
        eprintln!("usage: cfi-samples BOOK_DIR OUT.json [PER_PART]");
        std::process::exit(2);
    };
    let per_part: usize = args.get(2).and_then(|n| n.parse().ok()).unwrap_or(40);
    let dir = Path::new(dir);
    let manifest: Manifest =
        serde_json::from_slice(&std::fs::read(dir.join("musaeum.json")).expect("musaeum.json"))
            .expect("a manifest");
    let mut samples = Vec::new();
    let mut global = 0usize;
    for document in &manifest.documents {
        for part in &document.parts {
            let index = global;
            global += 1;
            let text = std::fs::read_to_string(dir.join(&part.href)).expect("a part");
            let dom = xml::parse(&text, &xml::Limits::default()).expect("well-formed");
            let Some(body) = dom.body() else { continue };
            let texts: Vec<_> = dom
                .descendants(body)
                .filter(|&n| dom.text(n).is_some_and(|t| !t.trim().is_empty()))
                .collect();
            if texts.is_empty() {
                continue;
            }
            let step = (texts.len() / per_part).max(1);
            for &node in texts.iter().step_by(step) {
                let value = dom.text(node).unwrap();
                let length = cfi::utf16_len(value);
                for offset in [0, length / 3, length] {
                    let Some(at) = cfi::utf16_to_byte(value, offset) else {
                        continue;
                    };
                    let (path, offset) = cfi::point_to_path(&dom, Point::Text { node, offset });
                    let reader = Cfi {
                        package: vec![Step::new(6), Step::new(2 * (index as u32 + 1))],
                        path,
                        offset,
                    }
                    .to_string();
                    // The canonical CFI of the same place, for the record: the engine's own conversion.
                    let canonical =
                        locator::reader_to_canonical(&manifest, &reader).unwrap_or_default();
                    samples.push(json!({
                        "href": part.href,
                        "cfi": reader,
                        "canonical": canonical,
                        "after": value[at..].chars().take(16).collect::<String>(),
                        "before": value[..at].chars().rev().take(16).collect::<Vec<_>>().into_iter().rev().collect::<String>(),
                    }));
                }
            }
        }
    }
    eprintln!("{} samples in {global} parts", samples.len());
    std::fs::write(out, serde_json::to_vec(&samples).unwrap()).expect("writing the samples");
}

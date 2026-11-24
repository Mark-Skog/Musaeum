//! The PDF reports embed the fonts tools/fetch-fonts.ps1 downloads into vendor/fonts/. Without them
//! the crate cannot be built, and this says how to get them instead of a bare missing-file error.

use std::path::Path;

const FONTS: [&str; 9] = [
    "NotoSans-Regular.ttf",
    "NotoSans-Italic.ttf",
    "NotoSans-Bold.ttf",
    "NotoSans-BoldItalic.ttf",
    "NotoSerif-Regular.ttf",
    "NotoSerif-Bold.ttf",
    "NotoSansMono-Regular.ttf",
    "NotoSansMath-Regular.ttf",
    "NotoSansSymbols2-Regular.ttf",
];

fn main() {
    let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/fonts");
    for name in FONTS {
        let path = folder.join(name);
        println!("cargo:rerun-if-changed={}", path.display());
        assert!(
            path.is_file(),
            "{} is missing: run tools/fetch-fonts.ps1 to download the report fonts",
            path.display()
        );
    }
}

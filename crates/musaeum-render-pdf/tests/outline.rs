//! A PDF's outline, read by the real worker in its sandbox and brought back through a slot, as the
//! app's table of contents gets it.

use std::fs;
use std::path::{Path, PathBuf};

use musaeum_sandbox::Policy;
use musaeum_shm::outline::{self, MAX_OUTLINE_ENTRIES, OUTLINE_PAGE, OutlineEntry};
use musaeum_worker::harness::Worker;

fn exe() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_musaeum-render-pdf"))
}

/// A PDF from its objects, numbered from 1, with a cross-reference table that points at each.
fn pdf(name: &str, objects: &[&str]) -> PathBuf {
    let mut out = String::from("%PDF-1.7\n");
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.push_str(&format!("{} 0 obj\n{object}\nendobj\n", index + 1));
    }
    let xref = out.len();
    out.push_str(&format!(
        "xref\n0 {}\n0000000000 65535 f \n",
        objects.len() + 1
    ));
    for offset in offsets {
        out.push_str(&format!("{offset:010} 00000 n \n"));
    }
    out.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objects.len() + 1
    ));
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("outline-{name}"));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("outline.pdf");
    fs::write(&path, out).unwrap();
    path
}

const PAGES: [&str; 4] = [
    "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >>",
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >>",
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >>",
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >>",
];

fn read_outline(path: &Path) -> Vec<OutlineEntry> {
    let mut worker = Worker::started(exe(), path, Policy::Lockdown);
    let layer = worker
        .text(1, 0, OUTLINE_PAGE)
        .unwrap_or_else(|message| panic!("no outline: {message:?}"));
    outline::from_text_layer(&layer, 3)
}

#[test]
fn bookmarks_come_back_in_order_with_their_pages() {
    let mut objects = vec!["<< /Type /Catalog /Pages 2 0 R /Outlines 6 0 R >>"];
    objects.extend(PAGES);
    objects.extend([
        "<< /Type /Outlines /First 7 0 R /Last 9 0 R /Count 3 >>",
        // A destination of its own, and a child below it.
        "<< /Title (Capa) /Parent 6 0 R /Next 9 0 R /First 8 0 R /Last 8 0 R /Count 1 /Dest [3 0 R /Fit] >>",
        // A go-to action, and a title in UTF-16 with an accent: Capítulo.
        "<< /Title <FEFF00430061007000ED00740075006C006F> /Parent 7 0 R /A << /S /GoTo /D [4 0 R /XYZ 0 200 0] >> >>",
        "<< /Title (Sem destino) /Parent 6 0 R /Prev 7 0 R >>",
    ]);
    let outline = read_outline(&pdf("ordered", &objects));
    let entry = |level, page, title: &str| OutlineEntry {
        level,
        page,
        title: title.to_owned(),
    };
    assert_eq!(
        outline,
        [
            entry(0, Some(0), "Capa"),
            entry(1, Some(1), "Capítulo"),
            entry(0, None, "Sem destino"),
        ]
    );
}

#[test]
fn an_outline_that_points_back_at_itself_still_ends() {
    let mut objects = vec!["<< /Type /Catalog /Pages 2 0 R /Outlines 6 0 R >>"];
    objects.extend(PAGES);
    objects.extend([
        "<< /Type /Outlines /First 7 0 R /Last 7 0 R /Count 1 >>",
        "<< /Title (Volta) /Parent 6 0 R /Next 7 0 R /First 7 0 R /Dest [5 0 R /Fit] >>",
    ]);
    let outline = read_outline(&pdf("loop", &objects));
    assert!(!outline.is_empty());
    assert!(outline.len() <= MAX_OUTLINE_ENTRIES);
    assert!(
        outline
            .iter()
            .all(|item| item.title == "Volta" && item.page == Some(2))
    );
}

#[test]
fn a_document_without_bookmarks_has_an_empty_outline() {
    let mut objects = vec!["<< /Type /Catalog /Pages 2 0 R >>"];
    objects.extend(PAGES);
    assert!(read_outline(&pdf("none", &objects)).is_empty());
}

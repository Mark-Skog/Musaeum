use std::io::Write;

use flate2::Compression;
use flate2::write::ZlibEncoder;

use super::*;

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

/// Writes PDFs the way producers do, keeping track of where every object went.
struct Builder {
    bytes: Vec<u8>,
    offsets: Vec<(u32, u64)>,
    listed: usize,
    last_section: Option<u64>,
}

impl Builder {
    fn new() -> Self {
        Self {
            bytes: b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec(),
            offsets: Vec::new(),
            listed: 0,
            last_section: None,
        }
    }

    fn object(&mut self, number: u32, body: &str) -> &mut Self {
        self.offsets.push((number, self.bytes.len() as u64));
        self.bytes
            .extend_from_slice(format!("{number} 0 obj\n{body}\nendobj\n").as_bytes());
        self
    }

    fn stream(&mut self, number: u32, entries: &str, data: &[u8]) -> &mut Self {
        self.offsets.push((number, self.bytes.len() as u64));
        self.bytes.extend_from_slice(
            format!(
                "{number} 0 obj\n<< {entries} /Length {} >>\nstream\n",
                data.len()
            )
            .as_bytes(),
        );
        self.bytes.extend_from_slice(data);
        self.bytes.extend_from_slice(b"\nendstream\nendobj\n");
        self
    }

    /// A classic section listing the objects written since the last one.
    fn table(&mut self, trailer: &str) -> &mut Self {
        let offset = self.bytes.len() as u64;
        let mut text = String::from("xref\n0 1\n0000000000 65535 f \n");
        for &(number, at) in &self.offsets[self.listed..] {
            text.push_str(&format!("{number} 1\n{at:010} 00000 n \n"));
        }
        self.listed = self.offsets.len();
        let previous = self
            .last_section
            .map(|previous| format!(" /Prev {previous}"))
            .unwrap_or_default();
        text.push_str(&format!(
            "trailer\n<< {trailer}{previous} >>\nstartxref\n{offset}\n%%EOF\n"
        ));
        self.bytes.extend_from_slice(text.as_bytes());
        self.last_section = Some(offset);
        self
    }

    /// A cross-reference stream, Flate and the PNG Up predictor as producers write them, listing
    /// what was written since the last section plus objects held in object streams.
    fn xref_stream(
        &mut self,
        number: u32,
        compressed: &[(u32, u32, u16)],
        trailer: &str,
    ) -> &mut Self {
        let offset = self.bytes.len() as u64;
        let mut rows: Vec<(u32, [u8; 7])> = Vec::new();
        for &(object, at) in &self.offsets[self.listed..] {
            let mut row = [1, 0, 0, 0, 0, 0, 0];
            row[1..5].copy_from_slice(&(at as u32).to_be_bytes());
            rows.push((object, row));
        }
        let mut own = [1, 0, 0, 0, 0, 0, 0];
        own[1..5].copy_from_slice(&(offset as u32).to_be_bytes());
        rows.push((number, own));
        for &(object, stream, index) in compressed {
            let mut row = [2, 0, 0, 0, 0, 0, 0];
            row[1..5].copy_from_slice(&stream.to_be_bytes());
            row[5..7].copy_from_slice(&index.to_be_bytes());
            rows.push((object, row));
        }
        rows.sort_by_key(|(object, _)| *object);

        let mut index = String::new();
        let mut predicted = Vec::new();
        let mut previous = [0u8; 7];
        for (object, row) in &rows {
            index.push_str(&format!("{object} 1 "));
            predicted.push(2);
            predicted.extend(
                row.iter()
                    .zip(previous)
                    .map(|(byte, up)| byte.wrapping_sub(up)),
            );
            previous = *row;
        }
        let size = rows.iter().map(|(object, _)| object + 1).max().unwrap();
        let previous = self
            .last_section
            .map(|previous| format!(" /Prev {previous}"))
            .unwrap_or_default();
        self.stream(
            number,
            &format!(
                "/Type /XRef /Size {size} /Index [{index}] /W [1 4 2] {trailer}{previous} \
                 /Filter /FlateDecode /DecodeParms << /Predictor 12 /Columns 7 >>"
            ),
            &zlib(&predicted),
        );
        self.listed = self.offsets.len();
        self.bytes
            .extend_from_slice(format!("startxref\n{offset}\n%%EOF\n").as_bytes());
        self.last_section = Some(offset);
        self
    }

    fn hints(&self) -> CatalogHints {
        catalog_hints(self.bytes.as_slice())
    }
}

fn hints(page_layout: Option<PageLayout>, direction: Option<Direction>) -> CatalogHints {
    CatalogHints {
        page_layout,
        direction,
    }
}

#[test]
fn a_classic_file_with_both_hints_in_the_catalog() {
    let mut pdf = Builder::new();
    pdf.object(
        1,
        "<< /Type /Catalog /Pages 2 0 R /PageLayout /TwoPageRight \
         /ViewerPreferences << /Direction /R2L /FitWindow true >> >>",
    )
    .object(2, "<< /Type /Pages /Kids [] /Count 0 >>")
    .table("/Size 3 /Root 1 0 R");
    assert_eq!(
        pdf.hints(),
        hints(Some(PageLayout::TwoPageRight), Some(Direction::R2L))
    );
}

#[test]
fn preferences_in_an_object_of_their_own() {
    let mut pdf = Builder::new();
    pdf.object(
        1,
        "<< /Type /Catalog /Pages 2 0 R /PageLayout /TwoColumnLeft /ViewerPreferences 3 0 R >>",
    )
    .object(2, "<< /Type /Pages /Kids [] /Count 0 >>")
    .object(3, "<< /HideToolbar false /Direction /L2R >>")
    .table("/Size 4 /Root 1 0 R");
    assert_eq!(
        pdf.hints(),
        hints(Some(PageLayout::TwoColumnLeft), Some(Direction::L2R))
    );
}

#[test]
fn a_catalog_inside_a_compressed_object_stream() {
    let catalog = "<< /Type /Catalog /Pages 2 0 R /PageLayout /SinglePage \
                   /ViewerPreferences << /Direction /R2L >> >>";
    let pages = "<< /Type /Pages /Kids [] /Count 0 >>";
    let header = format!("1 0 2 {} ", catalog.len() + 1);
    let contents = format!("{header}{catalog}\n{pages}");
    let mut pdf = Builder::new();
    pdf.stream(
        5,
        &format!(
            "/Type /ObjStm /N 2 /First {} /Filter /FlateDecode",
            header.len()
        ),
        &zlib(contents.as_bytes()),
    )
    .xref_stream(6, &[(1, 5, 0), (2, 5, 1)], "/Root 1 0 R");
    assert_eq!(
        pdf.hints(),
        hints(Some(PageLayout::SinglePage), Some(Direction::R2L))
    );
}

#[test]
fn the_catalog_of_the_last_update_is_the_one_in_force() {
    let mut pdf = Builder::new();
    pdf.object(
        1,
        "<< /Type /Catalog /Pages 2 0 R /PageLayout /OneColumn >>",
    )
    .object(2, "<< /Type /Pages /Kids [] /Count 0 >>")
    .table("/Size 3 /Root 1 0 R");
    assert_eq!(pdf.hints().page_layout, Some(PageLayout::OneColumn));

    pdf.object(
        1,
        "<< /Type /Catalog /Pages 2 0 R /PageLayout /TwoPageLeft >>",
    )
    .table("/Size 3 /Root 1 0 R");
    assert_eq!(pdf.hints().page_layout, Some(PageLayout::TwoPageLeft));

    // An update that only touches another object still leaves the newest catalog in force.
    pdf.object(2, "<< /Type /Pages /Kids [] /Count 0 /Changed true >>")
        .table("/Size 3 /Root 1 0 R");
    assert_eq!(pdf.hints().page_layout, Some(PageLayout::TwoPageLeft));
}

#[test]
fn a_hybrid_file_finds_compressed_objects_through_its_stream() {
    let catalog = "<< /Type /Catalog /Pages 2 0 R /PageLayout /TwoColumnRight >>";
    let header = "1 0 ";
    let mut pdf = Builder::new();
    pdf.object(2, "<< /Type /Pages /Kids [] /Count 0 >>")
        .stream(
            5,
            &format!("/Type /ObjStm /N 1 /First {}", header.len()),
            format!("{header}{catalog}").as_bytes(),
        );
    // The stream is for readers that understand it; the table for those that do not.
    let stream_at = pdf.bytes.len() as u64;
    pdf.xref_stream(6, &[(1, 5, 0)], "");
    let mut bytes = pdf.bytes.clone();
    let cut = bytes.windows(9).rposition(|b| b == b"startxref").unwrap();
    bytes.truncate(cut);
    let mut hybrid = Builder {
        bytes,
        offsets: pdf.offsets.clone(),
        listed: 0,
        last_section: None,
    };
    hybrid.offsets.retain(|(number, _)| *number == 2);
    hybrid.table(&format!("/Size 7 /Root 1 0 R /XRefStm {stream_at}"));
    assert_eq!(hybrid.hints().page_layout, Some(PageLayout::TwoColumnRight));
}

#[test]
fn escaped_names_and_comments_are_read_as_written() {
    let mut pdf = Builder::new();
    pdf.object(
        1,
        "<< /Type /Catalog % a comment\n /Pages 2 0 R /Page#4Cayout /Two#50ageRight >>",
    )
    .object(2, "<< /Type /Pages /Kids [] /Count 0 >>")
    .table("/Size 3 /Root 1 0 R");
    assert_eq!(pdf.hints().page_layout, Some(PageLayout::TwoPageRight));
}

#[test]
fn values_that_are_not_layouts_are_no_answer() {
    let mut pdf = Builder::new();
    pdf.object(
        1,
        "<< /Type /Catalog /Pages 2 0 R /PageLayout /Sideways \
         /ViewerPreferences << /Direction (R2L) >> >>",
    )
    .object(2, "<< /Type /Pages /Kids [] /Count 0 >>")
    .table("/Size 3 /Root 1 0 R");
    assert_eq!(pdf.hints(), CatalogHints::default());
}

#[test]
fn damage_anywhere_is_no_answer_and_never_a_panic() {
    let mut pdf = Builder::new();
    pdf.object(
        1,
        "<< /Type /Catalog /Pages 2 0 R /PageLayout /TwoPageRight >>",
    )
    .object(2, "<< /Type /Pages /Kids [] /Count 0 >>")
    .table("/Size 3 /Root 1 0 R");
    let good = pdf.bytes.clone();
    assert_eq!(pdf.hints().page_layout, Some(PageLayout::TwoPageRight));

    // Every truncation, and a byte flipped at every position, in turn.
    for cut in 0..good.len() {
        let _ = catalog_hints(&good[..cut]);
    }
    for at in 0..good.len() {
        let mut broken = good.clone();
        broken[at] ^= 0x55;
        let _ = catalog_hints(broken.as_slice());
    }
    assert_eq!(catalog_hints(b"".as_slice()), CatalogHints::default());
    assert_eq!(
        catalog_hints(b"%PDF-1.4\nstartxref\n999999999\n%%EOF".as_slice()),
        CatalogHints::default()
    );
}

#[test]
fn a_chain_of_updates_that_loops_ends() {
    let mut pdf = Builder::new();
    pdf.object(2, "<< /Type /Pages /Kids [] /Count 0 >>");
    let offset = pdf.bytes.len();
    // A section whose /Prev is itself, and that never lists the catalog.
    let text = format!(
        "xref\n0 1\n0000000000 65535 f \ntrailer\n<< /Size 3 /Root 1 0 R /Prev {offset} >>\n\
         startxref\n{offset}\n%%EOF\n"
    );
    pdf.bytes.extend_from_slice(text.as_bytes());
    assert_eq!(pdf.hints(), CatalogHints::default());
}

#[test]
fn deep_nesting_is_refused_before_it_costs_anything() {
    let nested = format!("{}{}", "[".repeat(10_000), "]".repeat(10_000));
    let mut pdf = Builder::new();
    pdf.object(
        1,
        &format!("<< /Type /Catalog /Deep {nested} /PageLayout /OneColumn >>"),
    )
    .table("/Size 2 /Root 1 0 R");
    assert_eq!(pdf.hints(), CatalogHints::default());
}

#[test]
fn the_png_predictors_undo_what_encoders_do() {
    // Two rows of three columns: None, then Sub, then Up, Average and Paeth on a third.
    let rows = [[10u8, 20, 30], [11, 22, 33], [5, 250, 7]];
    let mut encoded = Vec::new();
    encoded.push(0);
    encoded.extend_from_slice(&rows[0]);
    encoded.push(1);
    encoded.extend([
        rows[1][0],
        rows[1][1].wrapping_sub(rows[1][0]),
        rows[1][2].wrapping_sub(rows[1][1]),
    ]);
    encoded.push(2);
    encoded.extend(
        rows[2]
            .iter()
            .zip(rows[1])
            .map(|(byte, up)| byte.wrapping_sub(up)),
    );
    let decoded = unpredict_png(&encoded, 3).unwrap();
    assert_eq!(decoded, rows.concat());

    let average = [4u8, 8, 12];
    let mut encoded = vec![0, 2, 4, 6, 3];
    encoded.extend([
        average[0].wrapping_sub(2 / 2),
        average[1].wrapping_sub(((u16::from(average[0]) + 4) / 2) as u8),
        average[2].wrapping_sub(((u16::from(average[1]) + 6) / 2) as u8),
    ]);
    assert_eq!(unpredict_png(&encoded, 3).unwrap(), [2, 4, 6, 4, 8, 12]);

    let mut encoded = vec![0, 9, 9, 9, 4];
    let paeth_row = [1u8, 2, 3];
    let mut left = 0;
    let mut up_left = 0;
    for (at, &byte) in paeth_row.iter().enumerate() {
        encoded.push(byte.wrapping_sub(paeth(left, 9, up_left)));
        left = paeth_row[at];
        up_left = 9;
    }
    assert_eq!(unpredict_png(&encoded, 3).unwrap(), [9, 9, 9, 1, 2, 3]);
}

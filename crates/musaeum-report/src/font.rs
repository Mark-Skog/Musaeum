//! TrueType fonts, as far as a PDF that embeds them needs: which glyph draws a character, how wide
//! it is, the measures a font descriptor gives, and a subset of the font with only the glyphs used,
//! numbered again from 0, which goes into the PDF as its `FontFile2`.
//!
//! Only fonts built into the app are read here; still, every read is checked, and a font that does
//! not hold together is refused rather than trusted.

use std::collections::BTreeMap;

/// Why a font could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontError(pub String);

impl std::fmt::Display for FontError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for FontError {}

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn i16_at(data: &[u8], at: usize) -> Option<i16> {
    u16_at(data, at).map(|value| value as i16)
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

/// The cmap subtable a character is looked up in.
#[derive(Clone, Copy, Debug)]
enum Cmap {
    /// Format 4, at this offset in the font: the Basic Multilingual Plane.
    Segments(usize),
    /// Format 12: every plane.
    Groups(usize),
}

/// A TrueType font, read.
#[derive(Clone, Debug)]
pub struct Font {
    data: &'static [u8],
    tables: BTreeMap<[u8; 4], (usize, usize)>,
    /// None in a subset, which is drawn by glyph number only.
    cmap: Option<Cmap>,
    pub units_per_em: u16,
    pub glyph_count: u16,
    metrics_count: u16,
    long_offsets: bool,
    pub ascent: i16,
    pub descent: i16,
    pub cap_height: i16,
    pub bbox: [i16; 4],
    pub italic_angle: f32,
    pub fixed_pitch: bool,
    pub weight: u16,
    /// Its PostScript name, as `NotoSans-Regular`.
    pub name: String,
}

impl Font {
    /// Reads a font from its bytes.
    pub fn parse(data: &'static [u8]) -> Result<Self, FontError> {
        let bad = |what: &str| FontError(format!("not a usable TrueType font: {what}"));
        let version = u32_at(data, 0).ok_or_else(|| bad("too short"))?;
        if version != 0x0001_0000 && version != u32::from_be_bytes(*b"true") {
            return Err(bad("not TrueType outlines"));
        }
        let count = u16_at(data, 4).ok_or_else(|| bad("no table count"))? as usize;
        let mut tables = BTreeMap::new();
        for index in 0..count {
            let record = 12 + index * 16;
            let tag: [u8; 4] = data
                .get(record..record + 4)
                .and_then(|tag| tag.try_into().ok())
                .ok_or_else(|| bad("a table record"))?;
            let offset = u32_at(data, record + 8).ok_or_else(|| bad("a table offset"))? as usize;
            let length = u32_at(data, record + 12).ok_or_else(|| bad("a table length"))? as usize;
            if offset
                .checked_add(length)
                .is_none_or(|end| end > data.len())
            {
                return Err(bad("a table out of the file"));
            }
            tables.insert(tag, (offset, length));
        }
        let table = |tag: &[u8; 4]| -> Result<&'static [u8], FontError> {
            let (offset, length) = tables
                .get(tag)
                .copied()
                .ok_or_else(|| bad(&format!("no {} table", String::from_utf8_lossy(tag))))?;
            Ok(&data[offset..offset + length])
        };
        for needed in [b"head", b"hhea", b"maxp", b"hmtx", b"loca", b"glyf"] {
            table(needed)?;
        }
        let head = table(b"head")?;
        let hhea = table(b"hhea")?;
        let maxp = table(b"maxp")?;
        let units_per_em = u16_at(head, 18)
            .filter(|units| *units > 0)
            .ok_or_else(|| bad("head"))?;
        let bbox = [
            i16_at(head, 36).ok_or_else(|| bad("head"))?,
            i16_at(head, 38).ok_or_else(|| bad("head"))?,
            i16_at(head, 40).ok_or_else(|| bad("head"))?,
            i16_at(head, 42).ok_or_else(|| bad("head"))?,
        ];
        let long_offsets = i16_at(head, 50).ok_or_else(|| bad("head"))? == 1;
        let ascent = i16_at(hhea, 4).ok_or_else(|| bad("hhea"))?;
        let descent = i16_at(hhea, 6).ok_or_else(|| bad("hhea"))?;
        let metrics_count = u16_at(hhea, 34)
            .filter(|n| *n > 0)
            .ok_or_else(|| bad("hhea"))?;
        let glyph_count = u16_at(maxp, 4)
            .filter(|n| *n > 0)
            .ok_or_else(|| bad("maxp"))?;
        if metrics_count > glyph_count {
            return Err(bad("more metrics than glyphs"));
        }
        let hmtx = table(b"hmtx")?;
        let metrics_bytes =
            usize::from(metrics_count) * 4 + usize::from(glyph_count - metrics_count) * 2;
        if hmtx.len() < metrics_bytes {
            return Err(bad("hmtx too short"));
        }
        let loca = table(b"loca")?;
        let loca_bytes = (usize::from(glyph_count) + 1) * if long_offsets { 4 } else { 2 };
        if loca.len() < loca_bytes {
            return Err(bad("loca too short"));
        }
        let (cap_height, weight) = match table(b"OS/2") {
            Ok(os2) => {
                let version = u16_at(os2, 0).unwrap_or(0);
                let cap = if version >= 2 {
                    i16_at(os2, 88).unwrap_or(0)
                } else {
                    0
                };
                (cap, u16_at(os2, 4).unwrap_or(400))
            }
            Err(_) => (0, 400),
        };
        let cap_height = if cap_height > 0 {
            cap_height
        } else {
            (f32::from(ascent) * 0.7) as i16
        };
        let (italic_angle, fixed_pitch) = match table(b"post") {
            Ok(post) => (
                u32_at(post, 4).map_or(0.0, |fixed| fixed as i32 as f32 / 65536.0),
                u32_at(post, 12).unwrap_or(0) != 0,
            ),
            Err(_) => (0.0, false),
        };
        let cmap = tables.get(b"cmap").and_then(|&(at, _)| find_cmap(data, at));
        let name = table(b"name")
            .ok()
            .and_then(postscript_name)
            .unwrap_or_else(|| "Font".to_owned());
        let font = Font {
            data,
            tables,
            cmap,
            units_per_em,
            glyph_count,
            metrics_count,
            long_offsets,
            ascent,
            descent,
            cap_height,
            bbox,
            italic_angle,
            fixed_pitch,
            weight,
            name,
        };
        // Every glyph's outline is inside the glyph table.
        let glyf_length = font.tables[b"glyf"].1;
        for glyph in 0..glyph_count {
            let (start, end) = font.glyph_range(glyph).ok_or_else(|| bad("loca"))?;
            if start > end || end > glyf_length {
                return Err(bad("a glyph out of its table"));
            }
        }
        Ok(font)
    }

    fn table(&self, tag: &[u8; 4]) -> &'static [u8] {
        let (offset, length) = self.tables.get(tag).copied().unwrap_or((0, 0));
        &self.data[offset..offset + length]
    }

    /// The glyph that draws `c`, if the font has one.
    pub fn glyph(&self, c: char) -> Option<u16> {
        let code = u32::from(c);
        let data = self.data;
        let found = match self.cmap? {
            Cmap::Groups(at) => {
                let count = u32_at(data, at + 12)? as usize;
                let (mut low, mut high) = (0usize, count);
                let mut found = None;
                while low < high {
                    let middle = (low + high) / 2;
                    let group = at + 16 + middle * 12;
                    let start = u32_at(data, group)?;
                    let end = u32_at(data, group + 4)?;
                    if code < start {
                        high = middle;
                    } else if code > end {
                        low = middle + 1;
                    } else {
                        found = Some(u32_at(data, group + 8)? + (code - start));
                        break;
                    }
                }
                found.and_then(|glyph| u16::try_from(glyph).ok())
            }
            Cmap::Segments(at) => {
                if code > 0xFFFF {
                    return None;
                }
                let code = code as u16;
                let segments = usize::from(u16_at(data, at + 6)? / 2);
                let ends = at + 14;
                let starts = ends + segments * 2 + 2;
                let deltas = starts + segments * 2;
                let ranges = deltas + segments * 2;
                let (mut low, mut high) = (0usize, segments);
                while low < high {
                    let middle = (low + high) / 2;
                    if u16_at(data, ends + middle * 2)? < code {
                        low = middle + 1;
                    } else {
                        high = middle;
                    }
                }
                if low >= segments {
                    return None;
                }
                let start = u16_at(data, starts + low * 2)?;
                if code < start {
                    return None;
                }
                let delta = u16_at(data, deltas + low * 2)?;
                let range = u16_at(data, ranges + low * 2)?;
                if range == 0 {
                    Some(code.wrapping_add(delta))
                } else {
                    let at = ranges + low * 2 + usize::from(range) + usize::from(code - start) * 2;
                    let glyph = u16_at(data, at)?;
                    (glyph != 0).then(|| glyph.wrapping_add(delta))
                }
            }
        };
        found.filter(|glyph| *glyph != 0 && *glyph < self.glyph_count)
    }

    /// How far a glyph moves the pen, in font units.
    pub fn advance(&self, glyph: u16) -> u16 {
        let hmtx = self.table(b"hmtx");
        let index = glyph.min(self.metrics_count - 1);
        u16_at(hmtx, usize::from(index) * 4).unwrap_or(0)
    }

    /// How far a glyph moves the pen, in thousandths of the font size: what the PDF's widths say,
    /// and so what lines are measured with.
    pub fn width(&self, glyph: u16) -> u32 {
        let units = u32::from(self.advance(glyph));
        (units * 1000 + u32::from(self.units_per_em) / 2) / u32::from(self.units_per_em)
    }

    fn left_bearing(&self, glyph: u16) -> i16 {
        let hmtx = self.table(b"hmtx");
        if glyph < self.metrics_count {
            i16_at(hmtx, usize::from(glyph) * 4 + 2).unwrap_or(0)
        } else {
            let at =
                usize::from(self.metrics_count) * 4 + usize::from(glyph - self.metrics_count) * 2;
            i16_at(hmtx, at).unwrap_or(0)
        }
    }

    fn glyph_range(&self, glyph: u16) -> Option<(usize, usize)> {
        let loca = self.table(b"loca");
        let at = usize::from(glyph);
        if self.long_offsets {
            Some((
                u32_at(loca, at * 4)? as usize,
                u32_at(loca, at * 4 + 4)? as usize,
            ))
        } else {
            Some((
                usize::from(u16_at(loca, at * 2)?) * 2,
                usize::from(u16_at(loca, at * 2 + 2)?) * 2,
            ))
        }
    }

    fn outline(&self, glyph: u16) -> &'static [u8] {
        let glyf = self.table(b"glyf");
        match self.glyph_range(glyph) {
            Some((start, end)) if start <= end && end <= glyf.len() => &glyf[start..end],
            _ => &[],
        }
    }

    /// The glyphs a composite glyph is made of, with where each one's number is in its outline.
    fn components(outline: &[u8]) -> Vec<(usize, u16)> {
        let mut found = Vec::new();
        if i16_at(outline, 0).is_none_or(|contours| contours >= 0) {
            return found;
        }
        const WORDS: u16 = 0x0001;
        const SCALE: u16 = 0x0008;
        const MORE: u16 = 0x0020;
        const XY_SCALE: u16 = 0x0040;
        const TWO_BY_TWO: u16 = 0x0080;
        let mut at = 10;
        while let (Some(flags), Some(glyph)) = (u16_at(outline, at), u16_at(outline, at + 2)) {
            found.push((at + 2, glyph));
            at += 4 + if flags & WORDS != 0 { 4 } else { 2 };
            at += if flags & SCALE != 0 {
                2
            } else if flags & XY_SCALE != 0 {
                4
            } else if flags & TWO_BY_TWO != 0 {
                8
            } else {
                0
            };
            if flags & MORE == 0 {
                break;
            }
        }
        found
    }

    /// A font with only `used` glyphs (and `.notdef`), numbered in the order given after it, and
    /// `blanks` more copies of `.notdef` after them, one for each character no font could draw,
    /// so each still has a number of its own to be found by. Returns the font and the new number
    /// of each glyph kept, the components of composite glyphs among them.
    pub fn subset(&self, used: &[u16], blanks: usize) -> (Vec<u8>, BTreeMap<u16, u16>) {
        // The glyphs, with what composite glyphs are made of, in a fixed order.
        let mut order: Vec<u16> = vec![0];
        let mut renumbered: BTreeMap<u16, u16> = BTreeMap::from([(0, 0)]);
        let mut pending: Vec<u16> = used.to_vec();
        pending.reverse();
        while let Some(glyph) = pending.pop() {
            if glyph >= self.glyph_count || renumbered.contains_key(&glyph) {
                continue;
            }
            renumbered.insert(glyph, order.len() as u16);
            order.push(glyph);
            let mut parts: Vec<u16> = Self::components(self.outline(glyph))
                .into_iter()
                .map(|(_, part)| part)
                .collect();
            parts.reverse();
            pending.extend(parts);
        }
        let count = order.len() + blanks;

        let mut glyf = Vec::new();
        let mut loca: Vec<u32> = Vec::with_capacity(count + 1);
        let mut hmtx = Vec::with_capacity(count * 4);
        for index in 0..count {
            let old = order.get(index).copied().unwrap_or(0);
            loca.push(glyf.len() as u32);
            let mut outline = self.outline(old).to_vec();
            for (at, part) in Self::components(self.outline(old)) {
                let new = renumbered.get(&part).copied().unwrap_or(0);
                outline[at..at + 2].copy_from_slice(&new.to_be_bytes());
            }
            glyf.extend_from_slice(&outline);
            while glyf.len() % 4 != 0 {
                glyf.push(0);
            }
            hmtx.extend_from_slice(&self.advance(old).to_be_bytes());
            hmtx.extend_from_slice(&self.left_bearing(old).to_be_bytes());
        }
        loca.push(glyf.len() as u32);
        let loca: Vec<u8> = loca
            .iter()
            .flat_map(|offset| offset.to_be_bytes())
            .collect();

        let mut head = self.table(b"head").to_vec();
        head[8..12].copy_from_slice(&[0; 4]);
        head[50..52].copy_from_slice(&1i16.to_be_bytes());
        let mut hhea = self.table(b"hhea").to_vec();
        hhea[34..36].copy_from_slice(&(count as u16).to_be_bytes());
        let mut maxp = self.table(b"maxp").to_vec();
        maxp[4..6].copy_from_slice(&(count as u16).to_be_bytes());
        // `post` without glyph names: version 3.
        let mut post = self
            .table(b"post")
            .get(..32)
            .map(<[u8]>::to_vec)
            .unwrap_or_else(|| vec![0; 32]);
        post[0..4].copy_from_slice(&0x0003_0000u32.to_be_bytes());

        let mut tables: Vec<([u8; 4], Vec<u8>)> = vec![
            (*b"glyf", glyf),
            (*b"head", head),
            (*b"hhea", hhea),
            (*b"hmtx", hmtx),
            (*b"loca", loca),
            (*b"maxp", maxp),
            (*b"post", post),
        ];
        for kept in [b"OS/2", b"cvt ", b"fpgm", b"prep"] {
            if self.tables.contains_key(kept) {
                tables.push((*kept, self.table(kept).to_vec()));
            }
        }
        tables.sort_by_key(|table| table.0);
        (assemble(tables), renumbered)
    }
}

fn find_cmap(data: &[u8], at: usize) -> Option<Cmap> {
    let count = usize::from(u16_at(data, at + 2)?);
    let mut segments = None;
    for index in 0..count {
        let record = at + 4 + index * 8;
        let platform = u16_at(data, record)?;
        let encoding = u16_at(data, record + 2)?;
        let offset = at + u32_at(data, record + 4)? as usize;
        let format = u16_at(data, offset)?;
        match (platform, encoding, format) {
            (3, 10, 12) | (0, 4 | 6, 12) => return Some(Cmap::Groups(offset)),
            (3, 1, 4) | (0, 3, 4) => segments = Some(Cmap::Segments(offset)),
            _ => {}
        }
    }
    segments
}

fn postscript_name(name: &[u8]) -> Option<String> {
    let count = usize::from(u16_at(name, 2)?);
    let strings = usize::from(u16_at(name, 4)?);
    for index in 0..count {
        let record = 6 + index * 12;
        let platform = u16_at(name, record)?;
        let id = u16_at(name, record + 6)?;
        let length = usize::from(u16_at(name, record + 8)?);
        let offset = strings + usize::from(u16_at(name, record + 10)?);
        if id != 6 {
            continue;
        }
        let bytes = name.get(offset..offset + length)?;
        let text = if platform == 3 || platform == 0 {
            let units: Vec<u16> = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_be_bytes(*pair))
                .collect();
            String::from_utf16_lossy(&units)
        } else {
            bytes.iter().map(|&b| char::from(b)).collect()
        };
        let clean: String = text
            .chars()
            .filter(|c| c.is_ascii_graphic() && !"[](){}<>/%".contains(*c))
            .collect();
        if !clean.is_empty() {
            return Some(clean);
        }
    }
    None
}

fn checksum(bytes: &[u8]) -> u32 {
    bytes.chunks(4).fold(0u32, |sum, chunk| {
        let mut word = [0u8; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        sum.wrapping_add(u32::from_be_bytes(word))
    })
}

/// A font file from its tables, in the order given: directory, checksums, padding, and the
/// adjustment in `head` that makes the whole file sum to the magic number.
fn assemble(tables: Vec<([u8; 4], Vec<u8>)>) -> Vec<u8> {
    let count = tables.len() as u16;
    let mut power = 1u16;
    let mut log = 0u16;
    while power * 2 <= count {
        power *= 2;
        log += 1;
    }
    let mut out = Vec::new();
    out.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    out.extend_from_slice(&count.to_be_bytes());
    out.extend_from_slice(&(power * 16).to_be_bytes());
    out.extend_from_slice(&log.to_be_bytes());
    out.extend_from_slice(&(count * 16 - power * 16).to_be_bytes());
    let mut offset = 12 + tables.len() * 16;
    let mut head_at = None;
    for (tag, bytes) in &tables {
        out.extend_from_slice(tag);
        out.extend_from_slice(&checksum(bytes).to_be_bytes());
        out.extend_from_slice(&(offset as u32).to_be_bytes());
        out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        if tag == b"head" {
            head_at = Some(offset);
        }
        offset += bytes.len().div_ceil(4) * 4;
    }
    for (_, bytes) in &tables {
        out.extend_from_slice(bytes);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }
    if let Some(at) = head_at {
        let adjustment = 0xB1B0_AFBAu32.wrapping_sub(checksum(&out));
        out[at + 8..at + 12].copy_from_slice(&adjustment.to_be_bytes());
    }
    out
}

/// The faces a report is set in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Face {
    Sans,
    SansItalic,
    SansBold,
    SansBoldItalic,
    Serif,
    SerifBold,
    Mono,
    Math,
    Symbols,
}

impl Face {
    pub const ALL: [Face; 9] = [
        Face::Sans,
        Face::SansItalic,
        Face::SansBold,
        Face::SansBoldItalic,
        Face::Serif,
        Face::SerifBold,
        Face::Mono,
        Face::Math,
        Face::Symbols,
    ];

    fn bytes(self) -> &'static [u8] {
        macro_rules! font {
            ($name:literal) => {
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../vendor/fonts/",
                    $name
                ))
            };
        }
        match self {
            Face::Sans => font!("NotoSans-Regular.ttf"),
            Face::SansItalic => font!("NotoSans-Italic.ttf"),
            Face::SansBold => font!("NotoSans-Bold.ttf"),
            Face::SansBoldItalic => font!("NotoSans-BoldItalic.ttf"),
            Face::Serif => font!("NotoSerif-Regular.ttf"),
            Face::SerifBold => font!("NotoSerif-Bold.ttf"),
            Face::Mono => font!("NotoSansMono-Regular.ttf"),
            Face::Math => font!("NotoSansMath-Regular.ttf"),
            Face::Symbols => font!("NotoSansSymbols2-Regular.ttf"),
        }
    }

    /// The face's font, read once.
    pub fn font(self) -> &'static Font {
        static FONTS: std::sync::OnceLock<Vec<Font>> = std::sync::OnceLock::new();
        let fonts = FONTS.get_or_init(|| {
            Face::ALL
                .iter()
                .map(|face| {
                    let font = Font::parse(face.bytes())
                        .unwrap_or_else(|error| panic!("the font built in for {face:?}: {error}"));
                    assert!(
                        font.cmap.is_some(),
                        "the font built in for {face:?} maps no character"
                    );
                    font
                })
                .collect()
        });
        &fonts[self as usize]
    }

    pub fn serif(self) -> bool {
        matches!(self, Face::Serif | Face::SerifBold)
    }
}

/// Where a character is drawn from: its own face if that has it, else the fonts of mathematics and
/// symbols, else nowhere (`None`), and a blank of its face stands in for it.
pub fn glyph_for(face: Face, c: char) -> (Face, Option<u16>) {
    for candidate in [face, Face::Math, Face::Symbols] {
        if let Some(glyph) = candidate.font().glyph(c) {
            return (candidate, Some(glyph));
        }
    }
    (face, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_fonts_draw_portuguese_and_what_books_quote() {
        let sans = Face::Sans.font();
        assert_eq!(sans.units_per_em, 1000);
        assert_eq!(sans.name, "NotoSans-Regular");
        for c in "Ação, coração, pão, avó, pêssego — “aspas” ‘x’ … ºª €".chars()
        {
            assert!(sans.glyph(c).is_some(), "{c:?}");
        }
        assert!(Face::Serif.font().glyph('ç').is_some());
        let mono = Face::Mono.font();
        assert_eq!(
            mono.width(mono.glyph('i').unwrap()),
            mono.width(mono.glyph('m').unwrap())
        );
        assert!(Face::SansItalic.font().italic_angle < 0.0);
        assert_eq!(glyph_for(Face::Sans, '∫').0, Face::Math);
        for c in ['✓', '✗', '☐', '♞', '⇒', '≤'] {
            assert!(glyph_for(Face::Sans, c).1.is_some(), "{c:?}");
        }
        assert_eq!(glyph_for(Face::Sans, '♞').0, Face::Symbols);
        assert!(
            glyph_for(Face::Sans, '→').1.is_some(),
            "an arrow, from the mathematics font"
        );
        assert_eq!(glyph_for(Face::Serif, '中').1, None);
        let a = sans.glyph('a').unwrap();
        assert!((400..700).contains(&sans.width(a)), "{}", sans.width(a));
    }

    #[test]
    fn a_subset_keeps_the_glyphs_used_and_holds_together() {
        let sans = Face::Sans.font();
        let used: Vec<u16> = "Ação é".chars().filter_map(|c| sans.glyph(c)).collect();
        let (bytes, renumbered) = sans.subset(&used, 2);
        let leaked: &'static [u8] = Box::leak(bytes.clone().into_boxed_slice());
        let subset = Font::parse(leaked).expect("the subset is a font");
        assert!(subset.glyph_count as usize >= used.len() + 3);
        assert!(bytes.len() < 40_000, "{} bytes", bytes.len());
        assert_eq!(
            checksum(&bytes),
            0xB1B0_AFBA,
            "the whole file sums to the magic number"
        );
        for old in &used {
            let new = renumbered[old];
            assert_eq!(subset.advance(new), sans.advance(*old));
            assert_eq!(
                subset.outline(new).len().div_ceil(4),
                sans.outline(*old).len().div_ceil(4)
            );
        }
        // Composite glyphs point at their parts' new numbers.
        for (&old, &new) in &renumbered {
            for ((_, part_old), (_, part_new)) in Font::components(sans.outline(old))
                .into_iter()
                .zip(Font::components(subset.outline(new)))
            {
                assert_eq!(renumbered[&part_old], part_new);
            }
        }
        assert_eq!(sans.subset(&used, 2).0, bytes, "the same subset every time");
    }
}

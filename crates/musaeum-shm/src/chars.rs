//! A page's characters and where each one is, on their way from a worker to the host: what
//! selecting text and anchoring annotations on a fixed layout work with.
//!
//! Positions are quads in the page's own frame: fractions of the crop box before the page's
//! rotation is applied, from its top-left corner. That frame does not change with zoom, with the
//! window, or with the rotation a new revision may set, so a quad kept today still covers the same
//! glyphs then. The rotation the page is shown with comes along, for drawing.
//!
//! ```text
//! 0   "MCHR"
//! 4   version
//! 8   character count
//! 12  rotation: quarter turns clockwise the page is shown with (0 to 3)
//! 16  characters, 24 bytes each: the code point (u32); the quad's four corners, top left, top
//!     right, bottom right, bottom left, each x then y as u16 in units of 1/65535 of the crop box;
//!     flags (u16); and two bytes that are zero
//! ```
//!
//! The host decodes this from memory a worker controls, so every code point must be a character
//! and the count has to fit the bytes given.

use thiserror::Error;

/// `"MCHR"`, the first word of an encoded character layer.
pub const CHARS_MAGIC: u32 = u32::from_le_bytes(*b"MCHR");
/// Version of this layout, and of how workers decide what a character and its quad are. Annotations
/// anchored on one version are checked again on another. Version 2 turns the quads of characters
/// drawn turned the right way round: version 1 took PDFium's clockwise angle for a counterclockwise
/// one, so such a quad's top was its bottom.
pub const CHARS_VERSION: u32 = 2;
/// A line ends after this character.
pub const LINE_END: u16 = 1;
/// The text layer made this character up: a space between words or a line break, with no glyph.
pub const GENERATED: u16 = 2;

const HEADER: usize = 16;
const GLYPH: usize = 24;
const UNIT: f32 = 65535.0;

/// One character and where it is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glyph {
    pub ch: char,
    /// `[x0, y0, x1, y1, x2, y2, x3, y3]`: top left, top right, bottom right, bottom left, as
    /// fractions of the unrotated crop box.
    pub quad: [f32; 8],
    pub flags: u16,
}

impl Glyph {
    /// Whether a line ends after it.
    pub fn ends_line(&self) -> bool {
        self.flags & LINE_END != 0
    }

    /// Whether it was made up by the text layer, with no glyph of its own.
    pub fn is_generated(&self) -> bool {
        self.flags & GENERATED != 0
    }

    /// The middle of its quad.
    pub fn center(&self) -> (f32, f32) {
        let q = &self.quad;
        (
            (q[0] + q[2] + q[4] + q[6]) / 4.0,
            (q[1] + q[3] + q[5] + q[7]) / 4.0,
        )
    }

    /// Whether its quad is an upright rectangle, as it is for text that is not turned on its page.
    pub fn is_upright(&self) -> bool {
        let q = &self.quad;
        let near = |a: f32, b: f32| (a - b).abs() <= 1.5 / UNIT;
        near(q[1], q[3]) && near(q[5], q[7]) && near(q[0], q[6]) && near(q[2], q[4])
    }
}

/// A page's characters, in reading order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CharLayer {
    /// Quarter turns clockwise the page is shown with.
    pub rotation: u8,
    pub chars: Vec<Glyph>,
}

/// Why an encoded character layer was refused.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CharLayerError {
    #[error("the character layer is truncated")]
    Truncated,
    #[error("not a character layer of a known version")]
    BadHeader,
    #[error("character {0} is not a character")]
    BadChar(usize),
}

fn unit(value: f32) -> u16 {
    (value.clamp(0.0, 1.0) * UNIT).round() as u16
}

impl CharLayer {
    /// Bytes the encoded layer takes.
    pub fn encoded_len(&self) -> usize {
        HEADER + self.chars.len() * GLYPH
    }

    /// The text, one `char` per glyph, so a character's index is its index here.
    pub fn text(&self) -> String {
        self.chars.iter().map(|glyph| glyph.ch).collect()
    }

    /// Writes the layer at the start of `out` and returns its length, or `None` if it does not fit.
    pub fn encode_into(&self, out: &mut [u8]) -> Option<usize> {
        let len = self.encoded_len();
        if len > out.len() || u32::try_from(self.chars.len()).is_err() {
            return None;
        }
        put(out, 0, CHARS_MAGIC);
        put(out, 4, CHARS_VERSION);
        put(out, 8, self.chars.len() as u32);
        put(out, 12, u32::from(self.rotation % 4));
        for (index, glyph) in self.chars.iter().enumerate() {
            let at = HEADER + index * GLYPH;
            put(out, at, u32::from(glyph.ch));
            for (corner, value) in glyph.quad.iter().enumerate() {
                let value = if value.is_finite() { *value } else { 0.0 };
                out[at + 4 + corner * 2..at + 6 + corner * 2]
                    .copy_from_slice(&unit(value).to_le_bytes());
            }
            out[at + 20..at + 22].copy_from_slice(&glyph.flags.to_le_bytes());
            out[at + 22..at + 24].copy_from_slice(&[0, 0]);
        }
        Some(len)
    }

    /// Reads a layer a worker wrote.
    pub fn decode(bytes: &[u8]) -> Result<Self, CharLayerError> {
        if bytes.len() < HEADER {
            return Err(CharLayerError::Truncated);
        }
        if get(bytes, 0) != CHARS_MAGIC || get(bytes, 4) != CHARS_VERSION {
            return Err(CharLayerError::BadHeader);
        }
        let count = get(bytes, 8) as usize;
        let rotation = get(bytes, 12);
        if rotation > 3 {
            return Err(CharLayerError::BadHeader);
        }
        let end = count
            .checked_mul(GLYPH)
            .and_then(|glyphs| glyphs.checked_add(HEADER))
            .ok_or(CharLayerError::Truncated)?;
        if end > bytes.len() {
            return Err(CharLayerError::Truncated);
        }
        let mut chars = Vec::with_capacity(count);
        for index in 0..count {
            let at = HEADER + index * GLYPH;
            let ch = char::from_u32(get(bytes, at)).ok_or(CharLayerError::BadChar(index))?;
            let mut quad = [0f32; 8];
            for (corner, value) in quad.iter_mut().enumerate() {
                let raw =
                    u16::from_le_bytes([bytes[at + 4 + corner * 2], bytes[at + 5 + corner * 2]]);
                *value = f32::from(raw) / UNIT;
            }
            let flags = u16::from_le_bytes([bytes[at + 20], bytes[at + 21]]);
            chars.push(Glyph { ch, quad, flags });
        }
        Ok(Self {
            rotation: rotation as u8,
            chars,
        })
    }
}

/// The upright quad of a box `[left, top, right, bottom]`.
pub fn box_quad([left, top, right, bottom]: [f32; 4]) -> [f32; 8] {
    [left, top, right, top, right, bottom, left, bottom]
}

/// Quads that cover characters `start..end` of a page, one per stretch of a line: what a mark is
/// drawn with and what its `page_quads` selector keeps. Upright characters of one line merge into one
/// box; a character turned on its page keeps its own quad. Made-up characters add nothing.
pub fn line_quads(chars: &[Glyph], start: usize, end: usize) -> Vec<[f32; 8]> {
    let mut quads = Vec::new();
    let mut open: Option<[f32; 4]> = None;
    let close = |open: &mut Option<[f32; 4]>, quads: &mut Vec<[f32; 8]>| {
        if let Some(rect) = open.take() {
            quads.push(box_quad(rect));
        }
    };
    for glyph in chars.get(start..end.min(chars.len())).unwrap_or(&[]) {
        if !glyph.is_generated() && !glyph.ch.is_whitespace() {
            if glyph.is_upright() {
                let q = &glyph.quad;
                let rect = [
                    q[0].min(q[6]),
                    q[1].min(q[3]),
                    q[2].max(q[4]),
                    q[5].max(q[7]),
                ];
                open = Some(match open {
                    // The same line: it overlaps the open box vertically by most of its height.
                    Some(current) if overlaps_line(current, rect) => [
                        current[0].min(rect[0]),
                        current[1].min(rect[1]),
                        current[2].max(rect[2]),
                        current[3].max(rect[3]),
                    ],
                    Some(_) => {
                        close(&mut open, &mut quads);
                        rect
                    }
                    None => rect,
                });
            } else {
                close(&mut open, &mut quads);
                quads.push(glyph.quad);
            }
        }
        if glyph.ends_line() {
            close(&mut open, &mut quads);
        }
    }
    close(&mut open, &mut quads);
    quads
}

fn overlaps_line(a: [f32; 4], b: [f32; 4]) -> bool {
    let overlap = a[3].min(b[3]) - a[1].max(b[1]);
    let smaller = (a[3] - a[1]).min(b[3] - b[1]).max(f32::EPSILON);
    // A line goes left to right; a box far back to the left starts another stretch of it.
    overlap >= smaller * 0.5 && b[0] >= a[0] - smaller
}

/// The characters whose middles fall inside any of `quads`, as the range from the first to the last.
pub fn chars_in_quads(chars: &[Glyph], quads: &[[f32; 8]]) -> Option<(usize, usize)> {
    let inside = |x: f32, y: f32, q: &[f32; 8]| {
        // Inside a convex quad: on the same side of all four edges.
        let mut sign = 0.0f32;
        for corner in 0..4 {
            let (ax, ay) = (q[corner * 2], q[corner * 2 + 1]);
            let (bx, by) = (q[(corner * 2 + 2) % 8], q[(corner * 2 + 3) % 8]);
            let cross = (bx - ax) * (y - ay) - (by - ay) * (x - ax);
            if cross.abs() < 1e-9 {
                continue;
            }
            if sign == 0.0 {
                sign = cross.signum();
            } else if cross.signum() != sign {
                return false;
            }
        }
        true
    };
    let mut first = None;
    let mut last = None;
    for (index, glyph) in chars.iter().enumerate() {
        if glyph.is_generated() {
            continue;
        }
        let (x, y) = glyph.center();
        if quads.iter().any(|quad| inside(x, y, quad)) {
            first.get_or_insert(index);
            last = Some(index);
        }
    }
    Some((first?, last? + 1))
}

fn put(out: &mut [u8], at: usize, value: u32) {
    out[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn get(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glyph(ch: char, rect: [f32; 4], flags: u16) -> Glyph {
        Glyph {
            ch,
            quad: box_quad(rect),
            flags,
        }
    }

    /// "Olá mundo" on one line and "日本" on the next.
    fn layer() -> CharLayer {
        let mut chars = Vec::new();
        for (index, ch) in "Olá".chars().enumerate() {
            let left = 0.1 + index as f32 * 0.02;
            chars.push(glyph(ch, [left, 0.1, left + 0.02, 0.12], 0));
        }
        chars.push(glyph(' ', [0.0; 4], GENERATED));
        for (index, ch) in "mundo".chars().enumerate() {
            let left = 0.2 + index as f32 * 0.02;
            chars.push(glyph(
                ch,
                [left, 0.1, left + 0.02, 0.12],
                if index == 4 { LINE_END } else { 0 },
            ));
        }
        chars.push(glyph('日', [0.1, 0.2, 0.13, 0.23], 0));
        chars.push(glyph('本', [0.13, 0.2, 0.16, 0.23], LINE_END));
        CharLayer { rotation: 1, chars }
    }

    #[test]
    fn a_layer_comes_back_as_it_went_within_a_unit() {
        let layer = layer();
        let mut slot = vec![0u8; layer.encoded_len()];
        let len = layer.encode_into(&mut slot).unwrap();
        let back = CharLayer::decode(&slot[..len]).unwrap();
        assert_eq!(back.rotation, 1);
        assert_eq!(back.text(), "Olá mundo日本");
        assert_eq!(back.chars.len(), layer.chars.len());
        for (a, b) in back.chars.iter().zip(&layer.chars) {
            assert_eq!(a.ch, b.ch);
            assert_eq!(a.flags, b.flags);
            for (x, y) in a.quad.iter().zip(&b.quad) {
                assert!((x - y).abs() <= 0.5 / UNIT, "{x} {y}");
            }
        }
        assert!(
            layer
                .encode_into(&mut vec![0u8; layer.encoded_len() - 1])
                .is_none()
        );
    }

    #[test]
    fn what_a_worker_writes_wrong_is_refused() {
        let layer = layer();
        let mut slot = vec![0u8; layer.encoded_len()];
        layer.encode_into(&mut slot).unwrap();
        assert_eq!(
            CharLayer::decode(&slot[..slot.len() - 1]),
            Err(CharLayerError::Truncated)
        );
        let mut huge = slot.clone();
        huge[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(CharLayer::decode(&huge), Err(CharLayerError::Truncated));
        let mut surrogate = slot.clone();
        surrogate[16..20].copy_from_slice(&0xD800u32.to_le_bytes());
        assert_eq!(
            CharLayer::decode(&surrogate),
            Err(CharLayerError::BadChar(0))
        );
        let mut turned = slot.clone();
        turned[12..16].copy_from_slice(&4u32.to_le_bytes());
        assert_eq!(CharLayer::decode(&turned), Err(CharLayerError::BadHeader));
        let mut other = slot;
        other[4..8].copy_from_slice(&99u32.to_le_bytes());
        assert_eq!(CharLayer::decode(&other), Err(CharLayerError::BadHeader));
    }

    #[test]
    fn a_line_is_one_box_and_each_line_its_own() {
        let layer = layer();
        let near =
            |a: [f32; 8], b: [f32; 4]| a.iter().zip(box_quad(b)).all(|(x, y)| (x - y).abs() < 1e-6);
        let quads = line_quads(&layer.chars, 0, layer.chars.len());
        assert_eq!(quads.len(), 2);
        assert!(near(quads[0], [0.1, 0.1, 0.3, 0.12]), "{:?}", quads[0]);
        assert!(near(quads[1], [0.1, 0.2, 0.16, 0.23]), "{:?}", quads[1]);
        // Part of a line: from "á" to "m".
        let part = line_quads(&layer.chars, 2, 5);
        assert_eq!(part.len(), 1);
        assert!(near(part[0], [0.14, 0.1, 0.22, 0.12]), "{:?}", part[0]);
        assert!(
            line_quads(&layer.chars, 3, 4).is_empty(),
            "a made-up space alone has no quad"
        );
    }

    #[test]
    fn a_turned_character_keeps_its_own_quad() {
        let turned = Glyph {
            ch: 'x',
            quad: [0.5, 0.5, 0.52, 0.52, 0.51, 0.53, 0.49, 0.51],
            flags: 0,
        };
        assert!(!turned.is_upright());
        assert_eq!(line_quads(&[turned], 0, 1), vec![turned.quad]);
    }

    #[test]
    fn the_characters_under_quads_are_found_again() {
        let layer = layer();
        let quads = line_quads(&layer.chars, 4, 9);
        assert_eq!(
            chars_in_quads(&layer.chars, &quads),
            Some((4, 9)),
            "\"mundo\""
        );
        assert_eq!(
            chars_in_quads(&layer.chars, &[box_quad([0.6, 0.6, 0.7, 0.7])]),
            None
        );
    }
}

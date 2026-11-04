//! A page's text layer on its way from a worker to the host: words and their boxes, written into a
//! slot of the pixel slab, because a dense page holds far more than a control message can.
//!
//! ```text
//! 0   "MTXT"
//! 4   version
//! 8   word count
//! 12  text length in bytes
//! 16  words, 24 bytes each: left, top, right, bottom as f32 fractions of the page as it is shown,
//!     from its top-left corner; then the word's offset into the text (u32), its length in bytes
//!     (u16) and flags (u16)
//! ..  the text of every word, UTF-8, one after another
//! ```
//!
//! The host decodes this from memory a worker controls, so decoding checks every offset and
//! length, and the text has to be valid UTF-8.

use thiserror::Error;

/// `"MTXT"`, the first word of an encoded text layer.
pub const TEXT_MAGIC: u32 = u32::from_le_bytes(*b"MTXT");
/// Version of this layout.
pub const TEXT_VERSION: u32 = 1;
/// The word is the last of its line.
pub const LINE_END: u16 = 1;

const HEADER: usize = 16;
const WORD: usize = 24;

/// One word and where it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    /// The word, without surrounding white space.
    pub text: String,
    /// `[left, top, right, bottom]`, as fractions of the page as it is shown.
    pub rect: [f32; 4],
    /// Whether a line ends after it.
    pub line_end: bool,
}

/// A page's text layer, in reading order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextLayer {
    /// Every word.
    pub words: Vec<Word>,
}

/// Why an encoded text layer was refused.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum TextLayerError {
    /// It is shorter than its own header says.
    #[error("the text layer is truncated")]
    Truncated,
    /// The header is not this layout's.
    #[error("not a text layer of a known version")]
    BadHeader,
    /// A word points outside the text, splits a character, or has no finite box.
    #[error("word {0} is malformed")]
    BadWord(usize),
    /// The text is not UTF-8.
    #[error("the text is not valid UTF-8")]
    NotUtf8,
}

impl TextLayer {
    /// Bytes the encoded layer takes.
    pub fn encoded_len(&self) -> usize {
        HEADER
            + self.words.len() * WORD
            + self.words.iter().map(|word| word.text.len()).sum::<usize>()
    }

    /// Characters in the layer, white space not counted.
    pub fn char_count(&self) -> u32 {
        let count: usize = self
            .words
            .iter()
            .map(|word| word.text.chars().filter(|c| !c.is_whitespace()).count())
            .sum();
        u32::try_from(count).unwrap_or(u32::MAX)
    }

    /// Writes the layer at the start of `out` and returns its length, or `None` if it does not
    /// fit, or a word is longer than the layout allows.
    pub fn encode_into(&self, out: &mut [u8]) -> Option<usize> {
        let len = self.encoded_len();
        if len > out.len() || u32::try_from(len).is_err() {
            return None;
        }
        let text_len: usize = self.words.iter().map(|word| word.text.len()).sum();
        put(out, 0, TEXT_MAGIC);
        put(out, 4, TEXT_VERSION);
        put(out, 8, self.words.len() as u32);
        put(out, 12, text_len as u32);

        let mut text_at = HEADER + self.words.len() * WORD;
        let mut start = 0usize;
        for (index, word) in self.words.iter().enumerate() {
            let length = u16::try_from(word.text.len()).ok()?;
            let at = HEADER + index * WORD;
            for (corner, value) in word.rect.iter().enumerate() {
                out[at + corner * 4..at + corner * 4 + 4].copy_from_slice(&value.to_le_bytes());
            }
            put(out, at + 16, start as u32);
            out[at + 20..at + 22].copy_from_slice(&length.to_le_bytes());
            let flags = if word.line_end { LINE_END } else { 0 };
            out[at + 22..at + 24].copy_from_slice(&flags.to_le_bytes());

            out[text_at..text_at + word.text.len()].copy_from_slice(word.text.as_bytes());
            text_at += word.text.len();
            start += word.text.len();
        }
        Some(len)
    }

    /// Reads a layer a worker wrote. Boxes are clamped to the page, since glyphs can reach past
    /// its edge; anything else out of place refuses the whole layer.
    pub fn decode(bytes: &[u8]) -> Result<Self, TextLayerError> {
        if bytes.len() < HEADER {
            return Err(TextLayerError::Truncated);
        }
        if get(bytes, 0) != TEXT_MAGIC || get(bytes, 4) != TEXT_VERSION {
            return Err(TextLayerError::BadHeader);
        }
        let count = get(bytes, 8) as usize;
        let text_len = get(bytes, 12) as usize;
        let words_end = count
            .checked_mul(WORD)
            .and_then(|words| words.checked_add(HEADER))
            .ok_or(TextLayerError::Truncated)?;
        let end = words_end
            .checked_add(text_len)
            .ok_or(TextLayerError::Truncated)?;
        if end > bytes.len() {
            return Err(TextLayerError::Truncated);
        }
        let text =
            std::str::from_utf8(&bytes[words_end..end]).map_err(|_| TextLayerError::NotUtf8)?;

        let mut words = Vec::with_capacity(count);
        for index in 0..count {
            let at = HEADER + index * WORD;
            let mut rect = [0f32; 4];
            for (corner, value) in rect.iter_mut().enumerate() {
                let raw = f32::from_le_bytes(
                    bytes[at + corner * 4..at + corner * 4 + 4]
                        .try_into()
                        .expect("four bytes"),
                );
                if !raw.is_finite() {
                    return Err(TextLayerError::BadWord(index));
                }
                *value = raw.clamp(0.0, 1.0);
            }
            let start = get(bytes, at + 16) as usize;
            let length = usize::from(u16::from_le_bytes([bytes[at + 20], bytes[at + 21]]));
            let flags = u16::from_le_bytes([bytes[at + 22], bytes[at + 23]]);
            let word = start
                .checked_add(length)
                .and_then(|stop| text.get(start..stop))
                .ok_or(TextLayerError::BadWord(index))?;
            words.push(Word {
                text: word.to_owned(),
                rect,
                line_end: flags & LINE_END != 0,
            });
        }
        Ok(Self { words })
    }
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

    fn layer() -> TextLayer {
        TextLayer {
            words: vec![
                Word {
                    text: "Olá".to_owned(),
                    rect: [0.1, 0.1, 0.2, 0.15],
                    line_end: false,
                },
                Word {
                    text: "mundo".to_owned(),
                    rect: [0.25, 0.1, 0.4, 0.15],
                    line_end: true,
                },
                Word {
                    text: "日本".to_owned(),
                    rect: [0.1, 0.2, 0.2, 0.25],
                    line_end: true,
                },
            ],
        }
    }

    #[test]
    fn round_trips_and_counts_characters() {
        let layer = layer();
        let mut out = vec![0u8; 4096];
        let len = layer.encode_into(&mut out).unwrap();
        assert_eq!(len, layer.encoded_len());
        assert_eq!(TextLayer::decode(&out[..len]).unwrap(), layer);
        assert_eq!(layer.char_count(), 10);
    }

    #[test]
    fn a_layer_that_does_not_fit_is_not_written() {
        let layer = layer();
        let mut out = vec![0u8; layer.encoded_len() - 1];
        assert_eq!(layer.encode_into(&mut out), None);
    }

    #[test]
    fn damage_is_refused_and_boxes_are_clamped() {
        let layer = layer();
        let mut out = vec![0u8; 4096];
        let len = layer.encode_into(&mut out).unwrap();

        assert_eq!(
            TextLayer::decode(&out[..len - 1]),
            Err(TextLayerError::Truncated)
        );

        let mut bad = out[..len].to_vec();
        bad[0] = b'X';
        assert_eq!(TextLayer::decode(&bad), Err(TextLayerError::BadHeader));

        // The first word's length now runs past the text.
        let mut bad = out[..len].to_vec();
        bad[HEADER + 20] = 200;
        assert_eq!(TextLayer::decode(&bad), Err(TextLayerError::BadWord(0)));

        // The second word now starts inside the two-byte "á".
        let mut bad = out[..len].to_vec();
        bad[HEADER + WORD + 16] = 3;
        assert_eq!(TextLayer::decode(&bad), Err(TextLayerError::BadWord(1)));

        let mut bad = out[..len].to_vec();
        bad[HEADER..HEADER + 4].copy_from_slice(&f32::NAN.to_le_bytes());
        assert_eq!(TextLayer::decode(&bad), Err(TextLayerError::BadWord(0)));

        let mut outside = out[..len].to_vec();
        outside[HEADER + 8..HEADER + 12].copy_from_slice(&1.5f32.to_le_bytes());
        assert_eq!(TextLayer::decode(&outside).unwrap().words[0].rect[2], 1.0);

        // A count that would overflow is refused, not allocated.
        let mut bad = out[..len].to_vec();
        bad[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(TextLayer::decode(&bad), Err(TextLayerError::Truncated));
    }
}

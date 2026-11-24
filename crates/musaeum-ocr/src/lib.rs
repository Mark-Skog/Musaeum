//! The text of scanned pages, read by Tesseract in an isolated worker.
//!
//! A page's picture never reaches Tesseract whole. The line detector of `musaeum-vision` has found
//! its lines; each line is cut out of the picture along its baseline and straightened into a level
//! strip ([`strip`]), whatever its curve or its angle, and the worker reads that one line. Words come
//! back as stretches of the line, fractions from its start to its end, which is how selections and
//! marks on scans already name places on a line: so the words a selection covers are found by
//! arithmetic, and a line read once serves every later selection over it.
//!
//! [`protocol`] is the conversation with the worker, [`client`] the host's side of it. The worker
//! itself is `musaeum-ocr-worker`, the only program that links Tesseract.

pub mod client;
pub mod models;
pub mod protocol;
pub mod strip;

pub use client::{OcrClient, OcrConfig, OcrError};
pub use models::{KNOWN_MODELS, KnownModel, ModelFile, verified};
pub use protocol::{Mode, Word};
pub use strip::{Gray, LinePath, Strip, straighten};

/// Bumped when what is read from the same picture would change: the strip's geometry, the model, the
/// engine. Text kept from another version is read again.
pub const OCR_VERSION: u32 = 1;

/// Words under this confidence are what Tesseract makes of a smudge or a texture, not text.
pub const MIN_CONFIDENCE: f32 = 20.0;

/// A line read under this mean confidence is tried again upside down: the detector may have taken
/// it the wrong way round. Text read upside down still scores about 50, and level text on a clean
/// scan 90 or more, so the threshold sits between; the reading that wins by 10 points is kept.
const FLIP_BELOW: f32 = 75.0;

/// A word of a line, as a stretch of it.
#[derive(Clone, Debug, PartialEq)]
pub struct LineWord {
    pub text: String,
    /// Where it starts and ends, as fractions of the line from its start to its end.
    pub from: f32,
    pub to: f32,
    /// From 0 to 100.
    pub confidence: f32,
}

/// What one line says.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct LineText {
    /// In reading order, which is from the line's end when it is flipped.
    pub words: Vec<LineWord>,
    /// The mean confidence of its words, 0 with none.
    pub confidence: f32,
    /// Whether it read better upside down, the line running the other way.
    pub flipped: bool,
}

impl LineText {
    /// Its words between two fractions of the line: those whose middle is inside.
    pub fn between(&self, from: f32, to: f32) -> impl Iterator<Item = &LineWord> {
        self.words.iter().filter(move |word| {
            let middle = (word.from + word.to) / 2.0;
            middle >= from && middle <= to
        })
    }
}

fn line_text(strip: &Strip, words: &[Word], flipped: bool) -> LineText {
    let width = strip.picture.width as f32;
    let mut out: Vec<LineWord> = words
        .iter()
        .filter(|word| word.confidence >= MIN_CONFIDENCE)
        // The rule between two cells of a table, read as a bar: no word.
        .filter(|word| !word.text.chars().all(|ch| matches!(ch, '|' | '¦' | '│')))
        .map(|word| {
            // Upside down, the strip's columns run from the line's end.
            let (left, right) = if flipped {
                (width - word.right as f32, width - word.left as f32)
            } else {
                (word.left as f32, word.right as f32)
            };
            let (from, to) = strip.fractions(left, right);
            LineWord {
                text: word.text.clone(),
                from,
                to,
                confidence: word.confidence,
            }
        })
        .collect();
    // Only what lies along the line: past its ends the strip holds the tick of a chart or the edge
    // of a frame, which is no word of it.
    out.retain(|word| {
        let middle = (word.from + word.to) / 2.0;
        (-0.02..=1.02).contains(&middle)
    });
    // In reading order: upside down, the line is read from its end.
    if flipped {
        out.sort_by(|a, b| b.from.total_cmp(&a.from));
    } else {
        out.sort_by(|a, b| a.from.total_cmp(&b.from));
    }
    let confidence = if out.is_empty() {
        0.0
    } else {
        out.iter().map(|word| word.confidence).sum::<f32>() / out.len() as f32
    };
    LineText {
        words: out,
        confidence,
        flipped,
    }
}

/// The picture turned half a turn.
fn half_turn(picture: &Gray) -> Gray {
    let mut pixels = picture.pixels.clone();
    pixels.reverse();
    Gray {
        width: picture.width,
        height: picture.height,
        pixels,
    }
}

/// Reads one line of `picture`. `ppi` is how many of its pixels make an inch of the page.
pub fn read_line(
    client: &mut OcrClient,
    languages: &str,
    picture: &Gray,
    line: &LinePath,
    ppi: u32,
) -> Result<LineText, OcrError> {
    let strip = straighten(picture, line);
    let words = client.recognize(languages, &strip.picture, Mode::Line, ppi)?;
    let read = line_text(&strip, &words, false);
    if read.confidence >= FLIP_BELOW {
        return Ok(read);
    }
    // Cut again with the margins the other way round: upside down, the capitals and accents are
    // on the side the first strip gave less room.
    let other_strip = strip::straighten_upside_down(picture, line);
    let turned = half_turn(&other_strip.picture);
    let words = client.recognize(languages, &turned, Mode::Line, ppi)?;
    let other = line_text(&other_strip, &words, true);
    let best = if other.confidence > read.confidence + 10.0 {
        other
    } else {
        read
    };
    if best.confidence >= FLIP_BELOW {
        return Ok(best);
    }
    // Neither way reads well: the line may be set so close to the lines around it that its strip
    // holds parts of them, which a strip of one line cannot be read with. Cut tight to its own
    // letters, it may read.
    let tight_strip = strip::straighten_tight(picture, line);
    let words = client.recognize(languages, &tight_strip.picture, Mode::Line, ppi)?;
    let tight = line_text(&tight_strip, &words, false);
    Ok(if tight.confidence > best.confidence + 10.0 {
        tight
    } else {
        best
    })
}

/// The text of words in reading order, one space between words, `separator` between lines.
pub fn join_lines<'a>(
    lines: impl IntoIterator<Item = Vec<&'a LineWord>>,
    separator: &str,
) -> String {
    lines
        .into_iter()
        .map(|words| {
            words
                .iter()
                .map(|word| word.text.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(separator)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, left: i32, right: i32, confidence: f32) -> Word {
        Word {
            text: text.to_owned(),
            left,
            top: 0,
            right,
            bottom: 10,
            confidence,
            block: 0,
            paragraph: 0,
            line: 0,
        }
    }

    #[test]
    fn words_become_stretches_of_the_line() {
        let strip = Strip {
            picture: Gray {
                width: 120,
                height: 10,
                pixels: vec![255; 1200],
            },
            lead: 10.0,
            length: 100.0,
        };
        let words = [
            word("dois", 60, 100, 90.0),
            word("um", 10, 50, 80.0),
            word("~", 102, 108, 5.0),
        ];
        let read = line_text(&strip, &words, false);
        assert_eq!(read.words.len(), 2, "the smudge is left out");
        assert_eq!(read.words[0].text, "um");
        assert!((read.words[0].from - 0.0).abs() < 1e-5 && (read.words[0].to - 0.4).abs() < 1e-5);
        assert!((read.confidence - 85.0).abs() < 1e-4);
        let picked: Vec<&str> = read.between(0.3, 1.0).map(|w| w.text.as_str()).collect();
        assert_eq!(picked, ["dois"]);
        // Upside down, the first columns are the line's end.
        let flipped = line_text(&strip, &[word("um", 70, 110, 80.0)], true);
        assert!((flipped.words[0].from - 0.0).abs() < 1e-5);
        let text = join_lines(
            [
                read.words.iter().collect(),
                Vec::new(),
                flipped.words.iter().collect(),
            ],
            "\n",
        );
        assert_eq!(text, "um dois\num");
    }
}

//! The lines of a page as the reader uses them, and their encoding.
//!
//! Every position is a fraction of the page's crop box before its rotation, as the character quads
//! of Phase 4 are, so a model kept today still names the same ink after a new revision turns the
//! page. The page's rotation at analysis comes along, for whoever draws or hit-tests to turn the
//! polylines into the frame the page is shown in; the heights are lengths measured across each
//! line, as fractions of the page's height as shown (a line at 90° measures them along x, in the
//! same unit). A line's gaps and every place along it are fractions of its baseline's length,
//! measured in the page as shown with x and y in the same unit, so a line at any angle or bent
//! toward the spine is measured the same way.
//!
//! ```text
//! 0   "MLIN"
//! 4   version
//! 8   rotation (u8), three bytes zero
//! 12  skew, radians (f32)
//! 16  column count (u32)
//! 20  line count (u32)
//! 24  columns, 16 bytes each: left, top, right, bottom (f32)
//! ..  lines: column (u16), point count (u16), gap count (u16), flags (u16), x-height, ascent,
//!     descent (f32); then the points, x and y (f32); then the gaps, start and end (f32), as
//!     fractions of the baseline's length
//! ```
//!
//! Version 2 measures gaps along the baseline's length rather than its x-span, and has the flags:
//! [`LINE_TURNED`] marks a line of a block at another angle than the page's text.

use thiserror::Error;

/// The version of the detector whose lines a model holds. Annotations anchored on one version
/// are checked again on another, and a model of another version is traced again. Version 2
/// filters pictures out, finds turned text, and measures along the baseline's length. Version 3
/// turns a page on its side to trace its text level, keeps a short line between the lines of its
/// column, and leaves the leader of a table of contents out of a line's letters.
pub const LINES_VERSION: u32 = 3;

/// The most columns a model has.
pub const MAX_COLUMNS: usize = 64;

/// A line's flag: it belongs to a block at another angle than the page's own text.
pub const LINE_TURNED: u16 = 1;

/// `"MLIN"`, the first word of an encoded model.
pub const LINES_MAGIC: u32 = u32::from_le_bytes(*b"MLIN");

/// A column of text.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Column {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

/// One line of text as it was found.
#[derive(Clone, Debug, PartialEq)]
pub struct VisualLine {
    /// The column it is in.
    pub column: u16,
    /// [`LINE_TURNED`], or nothing.
    pub flags: u16,
    /// The baseline, as fractions of the unrotated crop box, from the line's start to its end.
    pub baseline: Vec<[f32; 2]>,
    /// The height of the small letters above the baseline, as a fraction of the page's height.
    pub x_height: f32,
    /// How far the tallest ink of the line reaches above the baseline.
    pub ascent: f32,
    /// How far the lowest ink reaches below it.
    pub descent: f32,
    /// The spaces between words, as fractions of the baseline's length from its start.
    pub gaps: Vec<[f32; 2]>,
}

/// The lines of one page.
#[derive(Clone, Debug, PartialEq)]
pub struct LineModel {
    pub version: u32,
    /// Quarter turns clockwise the page was shown with when analyzed.
    pub rotation: u8,
    /// The page's skew, radians, positive when lines go down to the right as shown.
    pub skew: f32,
    pub columns: Vec<Column>,
    /// In reading order: column by column, top to bottom.
    pub lines: Vec<VisualLine>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ModelError {
    #[error("the line model is truncated")]
    Truncated,
    #[error("not a line model of a known version")]
    BadHeader,
    #[error("line {0} is malformed")]
    BadLine(usize),
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_f32(out: &mut Vec<u8>, value: f32) {
    out.extend_from_slice(&(if value.is_finite() { value } else { 0.0 }).to_le_bytes());
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, count: usize) -> Result<&[u8], ModelError> {
        let end = self.at.checked_add(count).ok_or(ModelError::Truncated)?;
        let slice = self.bytes.get(self.at..end).ok_or(ModelError::Truncated)?;
        self.at = end;
        Ok(slice)
    }

    fn u32(&mut self) -> Result<u32, ModelError> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("four bytes"),
        ))
    }

    fn u16(&mut self) -> Result<u16, ModelError> {
        Ok(u16::from_le_bytes(
            self.take(2)?.try_into().expect("two bytes"),
        ))
    }

    fn f32(&mut self) -> Result<f32, ModelError> {
        let value = f32::from_le_bytes(self.take(4)?.try_into().expect("four bytes"));
        Ok(if value.is_finite() { value } else { 0.0 })
    }
}

impl LineModel {
    pub fn empty(rotation: u8) -> Self {
        Self {
            version: LINES_VERSION,
            rotation,
            skew: 0.0,
            columns: Vec::new(),
            lines: Vec::new(),
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(24 + self.columns.len() * 16 + self.lines.len() * 64);
        put_u32(&mut out, LINES_MAGIC);
        put_u32(&mut out, self.version);
        out.extend_from_slice(&[self.rotation % 4, 0, 0, 0]);
        put_f32(&mut out, self.skew);
        put_u32(&mut out, self.columns.len() as u32);
        put_u32(&mut out, self.lines.len() as u32);
        for column in &self.columns {
            for value in [column.left, column.top, column.right, column.bottom] {
                put_f32(&mut out, value);
            }
        }
        for line in &self.lines {
            let points = line.baseline.len().min(u16::MAX as usize);
            let gaps = line.gaps.len().min(u16::MAX as usize);
            out.extend_from_slice(&line.column.to_le_bytes());
            out.extend_from_slice(&(points as u16).to_le_bytes());
            out.extend_from_slice(&(gaps as u16).to_le_bytes());
            out.extend_from_slice(&line.flags.to_le_bytes());
            put_f32(&mut out, line.x_height);
            put_f32(&mut out, line.ascent);
            put_f32(&mut out, line.descent);
            for point in &line.baseline[..points] {
                put_f32(&mut out, point[0]);
                put_f32(&mut out, point[1]);
            }
            for gap in &line.gaps[..gaps] {
                put_f32(&mut out, gap[0]);
                put_f32(&mut out, gap[1]);
            }
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ModelError> {
        let mut reader = Reader { bytes, at: 0 };
        if reader.u32()? != LINES_MAGIC {
            return Err(ModelError::BadHeader);
        }
        let version = reader.u32()?;
        if version != LINES_VERSION {
            return Err(ModelError::BadHeader);
        }
        let rotation = reader.take(4)?[0];
        if rotation > 3 {
            return Err(ModelError::BadHeader);
        }
        let skew = reader.f32()?;
        let column_count = reader.u32()? as usize;
        let line_count = reader.u32()? as usize;
        if column_count > MAX_COLUMNS || line_count > 100_000 {
            return Err(ModelError::BadHeader);
        }
        let mut columns = Vec::with_capacity(column_count);
        for _ in 0..column_count {
            columns.push(Column {
                left: reader.f32()?,
                top: reader.f32()?,
                right: reader.f32()?,
                bottom: reader.f32()?,
            });
        }
        let mut lines = Vec::with_capacity(line_count.min(4096));
        for index in 0..line_count {
            let column = reader.u16()?;
            let points = reader.u16()? as usize;
            let gaps = reader.u16()? as usize;
            let flags = reader.u16()?;
            if points < 2 || usize::from(column) >= column_count.max(1) {
                return Err(ModelError::BadLine(index));
            }
            let x_height = reader.f32()?;
            let ascent = reader.f32()?;
            let descent = reader.f32()?;
            let mut baseline = Vec::with_capacity(points);
            for _ in 0..points {
                baseline.push([reader.f32()?, reader.f32()?]);
            }
            let mut gap_list = Vec::with_capacity(gaps);
            for _ in 0..gaps {
                gap_list.push([reader.f32()?, reader.f32()?]);
            }
            lines.push(VisualLine {
                column,
                flags,
                baseline,
                x_height,
                ascent,
                descent,
                gaps: gap_list,
            });
        }
        Ok(Self {
            version,
            rotation,
            skew,
            columns,
            lines,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> LineModel {
        LineModel {
            version: LINES_VERSION,
            rotation: 1,
            skew: 0.01,
            columns: vec![Column {
                left: 0.1,
                top: 0.1,
                right: 0.9,
                bottom: 0.9,
            }],
            lines: vec![VisualLine {
                column: 0,
                flags: LINE_TURNED,
                baseline: vec![[0.1, 0.2], [0.5, 0.21], [0.9, 0.2]],
                x_height: 0.01,
                ascent: 0.015,
                descent: 0.004,
                gaps: vec![[0.2, 0.25], [0.6, 0.66]],
            }],
        }
    }

    #[test]
    fn a_model_comes_back_as_it_went() {
        let model = model();
        let bytes = model.encode();
        assert_eq!(LineModel::decode(&bytes).unwrap(), model);
        assert_eq!(
            LineModel::decode(&bytes[..bytes.len() - 1]),
            Err(ModelError::Truncated)
        );
        let mut wrong = bytes.clone();
        wrong[4] = 9;
        assert_eq!(LineModel::decode(&wrong), Err(ModelError::BadHeader));
        let mut one_point = bytes;
        one_point[24 + 16 + 2] = 1;
        assert_eq!(LineModel::decode(&one_point), Err(ModelError::BadLine(0)));
    }
}

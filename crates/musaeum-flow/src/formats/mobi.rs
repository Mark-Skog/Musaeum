//! Mobipocket and Kindle: MOBI (version 6 and older) and KF8, which AZW3 files and the second half
//! of many `.mobi` files are.
//!
//! Both are Palm databases. Record 0 holds the headers and the EXTH metadata; the text follows in
//! records of 4 KiB, compressed with PalmDOC's LZ77 or with HUFF/CDIC, each with trailing entries to
//! strip first. MOBI 6 text is one HTML stream that `<mbp:pagebreak/>` divides and `filepos`
//! attributes link into by byte offset. KF8 text is a set of skeletons with fragments to insert into
//! them, described by two indexes, with stylesheets in separate flows and links of the form
//! `kindle:pos:fid:…:off:…`.
//!
//! Everything a file says about where things are is checked against what is there: a hostile file
//! gets an error, not a panic.

use std::collections::{HashMap, HashSet};

use super::{Markup, empty_page, parse_markup};
use crate::dom::{Dom, Name, NodeId};
use crate::package::{Contributor, Identifier, SourceFormat, Title};
use crate::source::{
    Limits, NormalizeError, Parsed, Result, SourceBook, SourceDocument, SourceNav, SourceResource,
    clean_field, fold_space,
};

const NULL_INDEX: u32 = 0xFFFF_FFFF;
const MAX_FIELD: usize = 4096;

fn malformed(what: &str) -> NormalizeError {
    NormalizeError::Malformed(format!("Mobipocket: {what}"))
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// A Palm database: its records over the file's bytes.
struct Pdb<'a> {
    bytes: &'a [u8],
    offsets: Vec<usize>,
}

impl<'a> Pdb<'a> {
    fn open(bytes: &'a [u8]) -> Result<Self> {
        if bytes.len() < 78 {
            return Err(NormalizeError::NotThisFormat("Mobipocket"));
        }
        let kind = &bytes[60..68];
        if kind != b"BOOKMOBI" && kind != b"TEXtREAd" {
            return Err(NormalizeError::NotThisFormat("Mobipocket"));
        }
        let count = u16_at(bytes, 76).ok_or_else(|| malformed("no record count"))? as usize;
        let mut offsets = Vec::with_capacity(count);
        for index in 0..count {
            let offset = u32_at(bytes, 78 + 8 * index)
                .ok_or_else(|| malformed("the record list is cut short"))?
                as usize;
            if offset > bytes.len() || offsets.last().is_some_and(|&last| offset < last) {
                return Err(malformed("records out of order or outside the file"));
            }
            offsets.push(offset);
        }
        Ok(Self { bytes, offsets })
    }

    fn len(&self) -> usize {
        self.offsets.len()
    }

    fn record(&self, index: usize) -> Option<&'a [u8]> {
        let start = *self.offsets.get(index)?;
        let end = self
            .offsets
            .get(index + 1)
            .copied()
            .unwrap_or(self.bytes.len());
        self.bytes.get(start..end)
    }
}

/// What record 0 says, for the MOBI 6 part or the KF8 part of a file.
#[derive(Debug, Default)]
struct Header {
    /// The record this header is in; every other record number in it counts from here.
    base: usize,
    compression: u16,
    text_length: usize,
    text_records: usize,
    encryption: u16,
    utf8: bool,
    version: u32,
    full_name: String,
    first_resource: u32,
    huff_offset: u32,
    huff_count: u32,
    extra_flags: u16,
    ncx_index: u32,
    fdst_index: u32,
    skeleton_index: u32,
    fragment_index: u32,
    exth: Vec<(u32, Vec<u8>)>,
}

impl Header {
    fn read(pdb: &Pdb<'_>, base: usize) -> Result<Self> {
        let r0 = pdb.record(base).ok_or_else(|| malformed("no record 0"))?;
        let mut header = Header {
            base,
            compression: u16_at(r0, 0).ok_or_else(|| malformed("a short record 0"))?,
            text_length: u32_at(r0, 4).unwrap_or(0) as usize,
            text_records: u16_at(r0, 8).unwrap_or(0) as usize,
            encryption: u16_at(r0, 12).unwrap_or(0),
            utf8: false,
            first_resource: NULL_INDEX,
            huff_offset: NULL_INDEX,
            ncx_index: NULL_INDEX,
            fdst_index: NULL_INDEX,
            skeleton_index: NULL_INDEX,
            fragment_index: NULL_INDEX,
            ..Header::default()
        };
        if r0.get(16..20) != Some(b"MOBI".as_slice()) {
            // Plain PalmDOC: text and nothing else, in Windows-1252.
            return Ok(header);
        }
        let length = u32_at(r0, 20).unwrap_or(0) as usize;
        let field = |at: usize| {
            if at + 4 <= 16 + length {
                u32_at(r0, at)
            } else {
                None
            }
        };
        header.utf8 = field(28) == Some(65001);
        header.version = field(36).unwrap_or(0);
        let name_at = field(84).unwrap_or(0) as usize;
        let name_len = field(88).unwrap_or(0) as usize;
        if let Some(name) = r0.get(name_at..name_at.saturating_add(name_len)) {
            header.full_name = decode(name, header.utf8);
        }
        header.first_resource = field(108).unwrap_or(NULL_INDEX);
        header.huff_offset = field(112).unwrap_or(NULL_INDEX);
        header.huff_count = field(116).unwrap_or(0);
        let exth_flags = field(128).unwrap_or(0);
        if length >= 0xE4 {
            header.extra_flags = u16_at(r0, 0xF2).unwrap_or(0);
            header.ncx_index = field(0xF4).unwrap_or(NULL_INDEX);
        }
        if header.version >= 8 {
            header.fdst_index = field(0xC0).unwrap_or(NULL_INDEX);
            header.fragment_index = field(0xF8).unwrap_or(NULL_INDEX);
            header.skeleton_index = field(0xFC).unwrap_or(NULL_INDEX);
        }
        if exth_flags & 0x40 != 0 {
            let at = 16 + length;
            if r0.get(at..at + 4) == Some(b"EXTH".as_slice()) {
                let count = u32_at(r0, at + 8).unwrap_or(0) as usize;
                let mut position = at + 12;
                for _ in 0..count.min(10_000) {
                    let (Some(kind), Some(size)) = (u32_at(r0, position), u32_at(r0, position + 4))
                    else {
                        break;
                    };
                    let size = size as usize;
                    if size < 8 {
                        break;
                    }
                    let Some(data) = r0.get(position + 8..position + size) else {
                        break;
                    };
                    header.exth.push((kind, data.to_vec()));
                    position += size;
                }
            }
        }
        Ok(header)
    }

    fn exth(&self, kind: u32) -> impl Iterator<Item = &[u8]> {
        self.exth
            .iter()
            .filter(move |(k, _)| *k == kind)
            .map(|(_, data)| data.as_slice())
    }

    fn exth_u32(&self, kind: u32) -> Option<u32> {
        self.exth(kind).next().and_then(|data| u32_at(data, 0))
    }

    /// A record number from this header, as an index into the file.
    fn record(&self, number: u32) -> Option<usize> {
        (number != NULL_INDEX).then(|| self.base + number as usize)
    }
}

fn decode(bytes: &[u8], utf8: bool) -> String {
    if utf8 {
        String::from_utf8_lossy(bytes).into_owned()
    } else {
        crate::encoding::decode_label(bytes, "windows-1252").text
    }
}

/// The size of the trailing entries at the end of a text record.
fn trailing_size(record: &[u8], flags: u16) -> usize {
    let mut size = 0usize;
    let mut bits = flags >> 1;
    while bits != 0 {
        if bits & 1 != 0 {
            // A variable-width size, written backwards from the end.
            let end = record.len().saturating_sub(size);
            let mut value = 0usize;
            let mut shift = 0;
            let mut position = end;
            while position > 0 {
                position -= 1;
                let byte = record[position];
                value |= usize::from(byte & 0x7F) << shift;
                shift += 7;
                if byte & 0x80 != 0 || shift >= 28 {
                    break;
                }
            }
            size += value;
        }
        bits >>= 1;
    }
    if flags & 1 != 0 && size < record.len() {
        size += usize::from(record[record.len() - size - 1] & 0x3) + 1;
    }
    size.min(record.len())
}

/// PalmDOC's LZ77, stopping at `limit` bytes of output.
fn palmdoc(input: &[u8], output: &mut Vec<u8>, limit: usize) -> Result<()> {
    let mut index = 0;
    while index < input.len() && output.len() < limit {
        let byte = input[index];
        index += 1;
        match byte {
            0x01..=0x08 => {
                let count = usize::from(byte);
                let literal = input
                    .get(index..index + count)
                    .ok_or_else(|| malformed("a literal past the end"))?;
                output.extend_from_slice(literal);
                index += count;
            }
            0x00 | 0x09..=0x7F => output.push(byte),
            0x80..=0xBF => {
                let next = *input
                    .get(index)
                    .ok_or_else(|| malformed("a reference past the end"))?;
                index += 1;
                let pair = (u16::from(byte) << 8) | u16::from(next);
                let distance = usize::from((pair >> 3) & 0x07FF);
                let length = usize::from(pair & 0x0007) + 3;
                if distance == 0 || distance > output.len() {
                    return Err(malformed("a reference before the start"));
                }
                for _ in 0..length {
                    let value = output[output.len() - distance];
                    output.push(value);
                }
            }
            _ => {
                output.push(b' ');
                output.push(byte ^ 0x80);
            }
        }
    }
    Ok(())
}

/// HUFF/CDIC, the Kindle's dictionary compression.
struct Huffman {
    dict1: Vec<(u32, bool, u64)>,
    mincode: Vec<u64>,
    maxcode: Vec<u64>,
    dictionary: Vec<Option<(Vec<u8>, bool)>>,
}

impl Huffman {
    fn new(huff: &[u8], cdics: &[&[u8]]) -> Result<Self> {
        if huff.get(0..8) != Some(b"HUFF\x00\x00\x00\x18".as_slice()) {
            return Err(malformed("not a HUFF record"));
        }
        let off1 = u32_at(huff, 8).ok_or_else(|| malformed("HUFF"))? as usize;
        let off2 = u32_at(huff, 12).ok_or_else(|| malformed("HUFF"))? as usize;
        let mut dict1 = Vec::with_capacity(256);
        for i in 0..256 {
            let v = u32_at(huff, off1 + 4 * i).ok_or_else(|| malformed("HUFF table"))?;
            let codelen = v & 0x1F;
            let term = v & 0x80 != 0;
            if codelen == 0 || (codelen <= 8 && !term) {
                return Err(malformed("HUFF code length"));
            }
            let maxcode = ((u64::from(v >> 8) + 1) << (32 - codelen)).wrapping_sub(1);
            dict1.push((codelen, term, maxcode));
        }
        let mut mincode = vec![0u64; 33];
        let mut maxcode = vec![0u64; 33];
        for codelen in 1..=32usize {
            let low =
                u32_at(huff, off2 + 8 * (codelen - 1)).ok_or_else(|| malformed("HUFF table"))?;
            let high = u32_at(huff, off2 + 8 * (codelen - 1) + 4)
                .ok_or_else(|| malformed("HUFF table"))?;
            mincode[codelen] = u64::from(low) << (32 - codelen);
            maxcode[codelen] = ((u64::from(high) + 1) << (32 - codelen)).wrapping_sub(1);
        }
        let mut dictionary = Vec::new();
        for cdic in cdics {
            if cdic.get(0..8) != Some(b"CDIC\x00\x00\x00\x10".as_slice()) {
                return Err(malformed("not a CDIC record"));
            }
            let phrases = u32_at(cdic, 8).ok_or_else(|| malformed("CDIC"))? as usize;
            let bits = u32_at(cdic, 12).ok_or_else(|| malformed("CDIC"))?;
            if bits > 16 {
                return Err(malformed("CDIC bits"));
            }
            let count = (1usize << bits).min(phrases.saturating_sub(dictionary.len()));
            for i in 0..count {
                let offset =
                    u16_at(cdic, 16 + 2 * i).ok_or_else(|| malformed("CDIC offsets"))? as usize;
                let length = u16_at(cdic, 16 + offset).ok_or_else(|| malformed("CDIC entry"))?;
                let data = cdic
                    .get(18 + offset..18 + offset + usize::from(length & 0x7FFF))
                    .ok_or_else(|| malformed("CDIC entry"))?;
                dictionary.push(Some((data.to_vec(), length & 0x8000 != 0)));
            }
        }
        Ok(Self {
            dict1,
            mincode,
            maxcode,
            dictionary,
        })
    }

    fn unpack(
        &mut self,
        data: &[u8],
        output: &mut Vec<u8>,
        depth: usize,
        limit: usize,
    ) -> Result<()> {
        if depth > 32 {
            return Err(malformed("HUFF phrases nest too deep"));
        }
        let mut padded = data.to_vec();
        padded.extend_from_slice(&[0; 8]);
        let mut bits_left = data.len() as i64 * 8;
        let mut position = 0usize;
        let read = |at: usize| u64::from_be_bytes(padded[at..at + 8].try_into().expect("8 bytes"));
        let mut x = read(position);
        let mut n: i64 = 32;
        loop {
            if output.len() > limit {
                return Err(NormalizeError::TooLarge("Mobipocket text".to_owned()));
            }
            if n <= 0 {
                position += 4;
                if position + 8 > padded.len() {
                    break;
                }
                x = read(position);
                n += 32;
            }
            let code = (x >> n) & 0xFFFF_FFFF;
            let (mut codelen, term, mut maxcode) = self.dict1[(code >> 24) as usize];
            if !term {
                while codelen < 32 && code < self.mincode[codelen as usize] {
                    codelen += 1;
                }
                maxcode = self.maxcode[codelen as usize];
            }
            n -= i64::from(codelen);
            bits_left -= i64::from(codelen);
            if bits_left < 0 {
                break;
            }
            let index = (maxcode.wrapping_sub(code) >> (32 - codelen)) as usize;
            let entry = self
                .dictionary
                .get_mut(index)
                .ok_or_else(|| malformed("a HUFF code outside the dictionary"))?
                .take()
                .ok_or_else(|| malformed("a HUFF phrase that contains itself"))?;
            let (phrase, flag) = entry;
            let unpacked = if flag {
                phrase
            } else {
                let mut inner = Vec::new();
                self.unpack(&phrase, &mut inner, depth + 1, limit)?;
                inner
            };
            output.extend_from_slice(&unpacked);
            self.dictionary[index] = Some((unpacked, true));
        }
        Ok(())
    }
}

/// The text of the book, every record decompressed and joined.
fn text(pdb: &Pdb<'_>, header: &Header, limits: &Limits) -> Result<Vec<u8>> {
    if header.encryption != 0 {
        return Err(NormalizeError::Drm);
    }
    let limit = limits.max_total_bytes.min(256 << 20);
    let mut output = Vec::with_capacity(header.text_length.min(limit));
    let mut huffman = if header.compression == 17480 {
        let first = header
            .record(header.huff_offset)
            .ok_or_else(|| malformed("no HUFF record"))?;
        let huff = pdb
            .record(first)
            .ok_or_else(|| malformed("no HUFF record"))?;
        let cdics: Vec<&[u8]> = (1..header.huff_count as usize)
            .filter_map(|i| pdb.record(first + i))
            .collect();
        Some(Huffman::new(huff, &cdics)?)
    } else {
        None
    };
    for number in 1..=header.text_records {
        let record = pdb
            .record(header.base + number)
            .ok_or_else(|| malformed("a text record is missing"))?;
        let record = &record[..record.len() - trailing_size(record, header.extra_flags)];
        match header.compression {
            1 => output.extend_from_slice(record),
            2 => palmdoc(record, &mut output, limit)?,
            17480 => {
                huffman
                    .as_mut()
                    .expect("a HUFF table")
                    .unpack(record, &mut output, 0, limit)?
            }
            other => {
                return Err(NormalizeError::Unsupported(format!(
                    "Mobipocket compression {other}"
                )));
            }
        }
        if output.len() > limit {
            return Err(NormalizeError::TooLarge("Mobipocket text".to_owned()));
        }
    }
    if header.text_length > 0 && output.len() > header.text_length {
        output.truncate(header.text_length);
    }
    Ok(output)
}

/// A variable-width integer read forward: the last byte has its high bit set.
fn varint(bytes: &[u8]) -> Option<(u32, usize)> {
    let mut value = 0u32;
    for (index, &byte) in bytes.iter().take(5).enumerate() {
        value = (value << 7) | u32::from(byte & 0x7F);
        if byte & 0x80 != 0 {
            return Some((value, index + 1));
        }
    }
    None
}

/// An entry of an INDX index: its name and its tags' values.
#[derive(Debug, Default)]
struct IndexEntry {
    name: Vec<u8>,
    tags: Vec<(u8, Vec<u32>)>,
}

impl IndexEntry {
    fn tag(&self, tag: u8) -> Option<&[u32]> {
        self.tags
            .iter()
            .find(|(t, _)| *t == tag)
            .map(|(_, v)| v.as_slice())
    }
}

/// Reads an index and its strings (CNCX), starting at the header record `first`.
/// An index's strings (CNCX), by their offset.
type Strings = HashMap<u32, Vec<u8>>;

/// Where a link or an entry of the table of contents leads: a document and a fragment.
type Target = Option<(String, Option<String>)>;

fn read_index(pdb: &Pdb<'_>, first: usize) -> Result<(Vec<IndexEntry>, Strings)> {
    let head = pdb
        .record(first)
        .ok_or_else(|| malformed("no index record"))?;
    if head.get(0..4) != Some(b"INDX".as_slice()) {
        return Err(malformed("not an INDX record"));
    }
    let records = u32_at(head, 24).ok_or_else(|| malformed("INDX"))? as usize;
    let cncx_count = u32_at(head, 52).unwrap_or(0) as usize;
    let tagx_at = u32_at(head, 180)
        .filter(|&at| at != 0)
        .or_else(|| u32_at(head, 4))
        .unwrap_or(0) as usize;
    if records > 10_000 || cncx_count > 10_000 {
        return Err(malformed("an index claims too many records"));
    }
    let tagx = head.get(tagx_at..).ok_or_else(|| malformed("TAGX"))?;
    if tagx.get(0..4) != Some(b"TAGX".as_slice()) {
        return Err(malformed("no TAGX"));
    }
    let tagx_length = u32_at(tagx, 4).ok_or_else(|| malformed("TAGX"))? as usize;
    let control_bytes = u32_at(tagx, 8).ok_or_else(|| malformed("TAGX"))? as usize;
    let mut tags: Vec<(u8, u8, u8, u8)> = Vec::new();
    let mut at = 12;
    while at + 4 <= tagx_length.min(tagx.len()) {
        tags.push((tagx[at], tagx[at + 1], tagx[at + 2], tagx[at + 3]));
        at += 4;
    }

    let mut strings = HashMap::new();
    for number in 0..cncx_count {
        let Some(record) = pdb.record(first + 1 + records + number) else {
            break;
        };
        let mut position = 0usize;
        while position < record.len() {
            let Some((length, consumed)) = varint(&record[position..]) else {
                break;
            };
            let start = position + consumed;
            let end = (start + length as usize).min(record.len());
            if length > 0 {
                strings.insert(
                    (number * 0x10000 + position) as u32,
                    record[start..end].to_vec(),
                );
            }
            position = end.max(position + 1);
        }
    }

    let mut entries = Vec::new();
    for number in 1..=records {
        let record = pdb
            .record(first + number)
            .ok_or_else(|| malformed("an index record is missing"))?;
        if record.get(0..4) != Some(b"INDX".as_slice()) {
            return Err(malformed("not an INDX record"));
        }
        let idxt = u32_at(record, 20).ok_or_else(|| malformed("IDXT"))? as usize;
        let count = u32_at(record, 24).ok_or_else(|| malformed("IDXT"))? as usize;
        if record.get(idxt..idxt + 4) != Some(b"IDXT".as_slice()) || count > 100_000 {
            return Err(malformed("IDXT"));
        }
        let mut starts: Vec<usize> = (0..count)
            .map(|i| u16_at(record, idxt + 4 + 2 * i).map(usize::from))
            .collect::<Option<_>>()
            .ok_or_else(|| malformed("IDXT offsets"))?;
        starts.push(idxt);
        for pair in starts.windows(2) {
            let data = record
                .get(pair[0]..pair[1])
                .ok_or_else(|| malformed("an index entry"))?;
            let name_length =
                usize::from(*data.first().ok_or_else(|| malformed("an index entry"))?);
            let name = data
                .get(1..1 + name_length)
                .ok_or_else(|| malformed("an index entry name"))?;
            let rest = &data[1 + name_length..];
            entries.push(IndexEntry {
                name: name.to_vec(),
                tags: tag_values(&tags, control_bytes, rest),
            });
        }
    }
    Ok((entries, strings))
}

/// The values of an entry's tags, from its control bytes, as the TAGX describes them.
fn tag_values(tagx: &[(u8, u8, u8, u8)], control_count: usize, data: &[u8]) -> Vec<(u8, Vec<u32>)> {
    let Some(controls) = data.get(..control_count) else {
        return Vec::new();
    };
    let mut controls = controls.to_vec();
    let mut rest = &data[control_count..];
    let mut wanted: Vec<(u8, Option<u32>, Option<u32>, u8)> = Vec::new();
    for &(tag, per_entry, mask, end) in tagx {
        if end == 1 {
            if !controls.is_empty() {
                controls.remove(0);
            }
            continue;
        }
        let Some(&control) = controls.first() else {
            break;
        };
        let mut value = control & mask;
        if value == 0 {
            continue;
        }
        if value == mask && mask.count_ones() > 1 {
            // All the mask's bits set: a count of bytes follows, not of values.
            let Some((bytes, consumed)) = varint(rest) else {
                break;
            };
            rest = &rest[consumed..];
            wanted.push((tag, None, Some(bytes), per_entry));
        } else {
            let mut mask = mask;
            while mask & 1 == 0 {
                mask >>= 1;
                value >>= 1;
            }
            wanted.push((tag, Some(u32::from(value)), None, per_entry));
        }
    }
    let mut out = Vec::new();
    for (tag, count, bytes, per_entry) in wanted {
        let mut values = Vec::new();
        match (count, bytes) {
            (Some(count), _) => {
                for _ in 0..(count as usize * usize::from(per_entry)).min(10_000) {
                    let Some((value, consumed)) = varint(rest) else {
                        break;
                    };
                    rest = &rest[consumed..];
                    values.push(value);
                }
            }
            (None, Some(bytes)) => {
                let mut used = 0u32;
                while used < bytes {
                    let Some((value, consumed)) = varint(rest) else {
                        break;
                    };
                    rest = &rest[consumed..];
                    used += consumed as u32;
                    values.push(value);
                }
            }
            _ => {}
        }
        out.push((tag, values));
    }
    out
}

/// What a resource record holds, if it is an image or a font.
fn resource(record: &[u8]) -> Option<Vec<u8>> {
    if record.starts_with(b"FONT") {
        let size = u32_at(record, 4)? as usize;
        let flags = u32_at(record, 8)?;
        let start = u32_at(record, 12)? as usize;
        let xor_length = u32_at(record, 16)? as usize;
        let xor_start = u32_at(record, 20)? as usize;
        let mut font = record.get(start..)?.to_vec();
        if flags & 2 != 0 && xor_length > 0 {
            let key = record.get(xor_start..xor_start + xor_length)?.to_vec();
            for (index, byte) in font.iter_mut().take(1040).enumerate() {
                *byte ^= key[index % key.len()];
            }
        }
        if flags & 1 != 0 {
            use std::io::Read;
            let mut out = Vec::with_capacity(size.min(64 << 20));
            flate2::read::ZlibDecoder::new(font.as_slice())
                .take(64 << 20)
                .read_to_end(&mut out)
                .ok()?;
            font = out;
        }
        return Some(font);
    }
    super::sniff(record).map(|_| record.to_vec())
}

/// Metadata from EXTH, and the title from the header if EXTH has none.
fn metadata(header: &Header, book: &mut SourceBook) {
    let text = |data: &[u8]| clean_field(&decode(data, header.utf8), MAX_FIELD);
    let m = &mut book.metadata;
    let title = header
        .exth(503)
        .next()
        .and_then(text)
        .or_else(|| clean_field(&header.full_name, MAX_FIELD));
    m.titles.extend(title.map(|value| Title {
        value,
        ..Title::default()
    }));
    m.creators
        .extend(header.exth(100).filter_map(text).map(|name| Contributor {
            name,
            ..Contributor::default()
        }));
    m.contributors
        .extend(header.exth(108).filter_map(text).map(|name| Contributor {
            name,
            ..Contributor::default()
        }));
    m.publisher = header.exth(101).next().and_then(text);
    m.description = header.exth(103).next().and_then(|data| {
        let raw = decode(data, header.utf8);
        let plain = if raw.contains('<') {
            crate::html::parse(&raw, 100_000)
                .and_then(|parsed| parsed.dom.body().map(|body| parsed.dom.text_content(body)))
                .unwrap_or(raw)
        } else {
            raw
        };
        clean_field(&plain, 20_000)
    });
    for isbn in header.exth(104).filter_map(text) {
        m.identifiers.push(Identifier {
            value: isbn,
            scheme: Some("ISBN".to_owned()),
        });
    }
    for asin in header.exth(113).filter_map(text) {
        m.identifiers.push(Identifier {
            value: asin,
            scheme: Some("ASIN".to_owned()),
        });
    }
    m.subjects.extend(header.exth(105).filter_map(text));
    m.published = header.exth(106).next().and_then(text);
    m.rights = header.exth(109).next().and_then(text);
    m.languages.extend(header.exth(524).filter_map(text));
    if header.exth(527).next().and_then(text).as_deref() == Some("rtl") {
        book.direction = Some("rtl".to_owned());
    }
}

/// Only what the headers say, without decompressing the text.
pub fn read_metadata(bytes: &[u8]) -> Result<crate::package::Metadata> {
    let pdb = Pdb::open(bytes)?;
    let first = Header::read(&pdb, 0)?;
    let header = match first
        .exth_u32(121)
        .filter(|&b| b > 0 && (b as usize) < pdb.len())
    {
        Some(boundary) => Header::read(&pdb, boundary as usize)
            .ok()
            .filter(|h| h.version >= 8 && !h.exth.is_empty())
            .unwrap_or(first),
        None => first,
    };
    let mut book = SourceBook::new(SourceFormat::Mobi);
    metadata(&header, &mut book);
    Ok(book.metadata)
}

/// Reads a MOBI or AZW3 file: the KF8 part if there is one, the MOBI 6 part otherwise.
pub fn read(bytes: &[u8], limits: &Limits) -> Result<SourceBook> {
    let pdb = Pdb::open(bytes)?;
    let first = Header::read(&pdb, 0)?;
    if first.version >= 8 {
        return kf8_book(&pdb, first, None, limits);
    }
    // A joint file: EXTH 121 is where the KF8 half's record 0 is.
    if let Some(boundary) = first
        .exth_u32(121)
        .filter(|&b| b > 0 && (b as usize) < pdb.len())
        && let Ok(header) = Header::read(&pdb, boundary as usize)
        && header.version >= 8
    {
        return kf8_book(&pdb, header, Some(&first), limits);
    }
    mobi6_book(&pdb, first, limits)
}

/// The records after the first resource that are images or fonts, by their number from it.
fn resources(pdb: &Pdb<'_>, header: &Header, book: &mut SourceBook) -> HashMap<usize, String> {
    let mut paths = HashMap::new();
    let Some(first) = header.record(header.first_resource) else {
        return paths;
    };
    for index in first..pdb.len() {
        let Some(record) = pdb.record(index) else {
            break;
        };
        if record.starts_with(b"BOUNDARY") || record.starts_with(b"\xE9\x8E\r\n") {
            break;
        }
        if let Some(data) = resource(record) {
            let number = index - first;
            let path = format!("text/res{number:05}");
            book.resources.push(SourceResource {
                path: path.clone(),
                media_type: String::new(),
                data,
            });
            paths.insert(number, path);
        }
    }
    paths
}

fn cover(header: &Header, paths: &HashMap<usize, String>) -> Option<String> {
    let offset = header.exth_u32(201).filter(|&o| o != NULL_INDEX)?;
    paths.get(&(offset as usize)).cloned()
}

/// Navigation from an NCX index: nested by each entry's parent.
fn ncx(pdb: &Pdb<'_>, header: &Header, target: &dyn Fn(&IndexEntry) -> Target) -> Vec<SourceNav> {
    let Some(first) = header.record(header.ncx_index) else {
        return Vec::new();
    };
    let Ok((entries, strings)) = read_index(pdb, first) else {
        return Vec::new();
    };
    let mut nodes: Vec<(SourceNav, Option<usize>)> = entries
        .iter()
        .map(|entry| {
            let label = entry
                .tag(3)
                .and_then(|values| values.first())
                .and_then(|offset| strings.get(offset))
                .map(|bytes| fold_space(&decode(bytes, header.utf8)))
                .unwrap_or_default();
            let parent = entry
                .tag(21)
                .and_then(|values| values.first())
                .map(|&p| p as usize);
            (
                SourceNav {
                    label,
                    target: target(entry),
                    ..SourceNav::default()
                },
                parent,
            )
        })
        .collect();
    // Children are attached to their parents from the last entry back, so indexes stay valid.
    let mut roots = Vec::new();
    for index in (0..nodes.len()).rev() {
        let (node, parent) = std::mem::take(&mut nodes[index]);
        match parent.filter(|&p| p < index) {
            Some(parent) => nodes[parent].0.children.insert(0, node),
            None => roots.insert(0, node),
        }
    }
    roots
}

/// Inserts empty anchors at every byte offset a `filepos` points at, without breaking a tag, an
/// entity or a character.
fn add_anchors(html: &[u8], positions: &[usize]) -> Vec<u8> {
    let mut out = Vec::with_capacity(html.len() + positions.len() * 32);
    let mut copied = 0;
    for &target in positions {
        if target == 0 || target >= html.len() || target < copied {
            continue;
        }
        let mut at = target;
        // Inside a tag: after its end.
        let next_open = html[at..].iter().position(|&b| b == b'<').map(|p| at + p);
        let next_close = html[at..].iter().position(|&b| b == b'>').map(|p| at + p);
        if let Some(close) = next_close
            && next_open.is_none_or(|open| close < open)
        {
            at = close + 1;
        }
        // Inside an entity: after its semicolon.
        if let Some(amp) = html[at.saturating_sub(10)..at]
            .iter()
            .rposition(|&b| b == b'&')
        {
            let amp = at.saturating_sub(10) + amp;
            if !html[amp..at].contains(&b';')
                && let Some(semi) = html[at..html.len().min(at + 12)]
                    .iter()
                    .position(|&b| b == b';')
            {
                at += semi + 1;
            }
        }
        // Inside a UTF-8 character: after it.
        while at < html.len() && html[at] & 0xC0 == 0x80 {
            at += 1;
        }
        if at < copied {
            continue;
        }
        out.extend_from_slice(&html[copied..at]);
        out.extend_from_slice(format!("<a id=\"filepos{target}\"></a>").as_bytes());
        copied = at;
    }
    out.extend_from_slice(&html[copied..]);
    out
}

/// Every number after `filepos=` in the markup.
fn filepos_targets(html: &[u8]) -> Vec<usize> {
    let mut targets = Vec::new();
    let needle = b"filepos=";
    let mut at = 0;
    while let Some(found) = html[at..]
        .windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle))
    {
        let start = at + found + needle.len();
        let digits: Vec<u8> = html[start..]
            .iter()
            .skip_while(|&&b| b == b'"' || b == b'\'')
            .take_while(|b| b.is_ascii_digit())
            .copied()
            .collect();
        if let Ok(number) = std::str::from_utf8(&digits).unwrap_or("").parse::<usize>() {
            targets.push(number);
        }
        at = start;
    }
    targets
}

fn mobi6_book(pdb: &Pdb<'_>, header: Header, limits: &Limits) -> Result<SourceBook> {
    let mut book = SourceBook::new(SourceFormat::Mobi);
    book.version = Some(header.version.to_string());
    metadata(&header, &mut book);
    let raw = text(pdb, &header, limits)?;
    let paths = resources(pdb, &header, &mut book);
    book.cover = cover(&header, &paths);

    // Every place a link or the NCX points at gets an anchor.
    let mut positions = filepos_targets(&raw);
    let ncx_entries = header
        .record(header.ncx_index)
        .and_then(|first| read_index(pdb, first).ok())
        .map(|(entries, _)| entries)
        .unwrap_or_default();
    positions.extend(
        ncx_entries
            .iter()
            .filter_map(|e| e.tag(1).and_then(|v| v.first()).map(|&p| p as usize)),
    );
    positions.sort_unstable();
    positions.dedup();
    let anchored = mobipocket_tags(&add_anchors(&raw, &positions));
    let markup = decode(&anchored, header.utf8);
    let mut warning = None;
    let (mut whole, _) =
        parse_markup(&markup, Markup::Html, limits, &mut warning).ok_or_else(|| {
            NormalizeError::TooLarge(warning.unwrap_or_else(|| "the text".to_owned()))
        })?;

    // Links, pictures and Mobipocket's own elements.
    let nodes: Vec<NodeId> = whole.descendants(Dom::DOCUMENT).collect();
    for node in nodes {
        let Some(name) = whole.name(node).map(|n| n.local.clone()) else {
            continue;
        };
        if name == "guide" {
            whole.detach(node);
            continue;
        }
        if let Some(position) = whole
            .attr(node, "filepos")
            .and_then(|v| v.trim_matches(['"', '\'']).parse::<usize>().ok())
        {
            whole.set_attr(node, Name::plain("href"), format!("#filepos{position}"));
            whole.remove_attr(node, &crate::dom::Ns::None, "filepos");
        }
        if name == "img" {
            let index = ["recindex", "hirecindex", "lorecindex"]
                .iter()
                .find_map(|key| whole.attr(node, key))
                .and_then(|v| v.trim_matches(['"', '\'']).parse::<usize>().ok());
            if let Some(path) = index
                .and_then(|i| i.checked_sub(1))
                .and_then(|i| paths.get(&i))
            {
                let file = path.rsplit('/').next().unwrap_or("").to_owned();
                whole.set_attr(node, Name::plain("src"), file);
            }
        }
    }
    let body = whole.body().ok_or_else(|| malformed("no body"))?;
    let title = book.metadata.title().unwrap_or("Mobipocket").to_owned();

    // Documents at top-level page breaks.
    let mut groups: Vec<Vec<NodeId>> = vec![Vec::new()];
    for &child in whole.children(body) {
        if whole.is_html(child, "hr") && whole.attr(child, "class") == Some(PAGEBREAK) {
            if !groups.last().expect("a group").is_empty() {
                groups.push(Vec::new());
            }
            continue;
        }
        groups.last_mut().expect("a group").push(child);
    }
    groups.retain(|group| !group.is_empty());
    if groups.is_empty() {
        groups.push(Vec::new());
    }
    for (index, group) in groups.into_iter().enumerate() {
        let (mut dom, page_body) =
            empty_page(&title, book.metadata.languages.first().map(String::as_str));
        for node in group {
            let copy = whole.copy_into(node, &mut dom);
            dom.append(page_body, copy);
        }
        dom.normalize_text(Dom::DOCUMENT);
        book.documents.push(SourceDocument {
            path: format!("text/{index:04}.xhtml"),
            id: None,
            linear: true,
            properties: Vec::new(),
            dom,
            parsed: Parsed::Generated,
            svg: false,
        });
    }
    super::retarget_fragment_links(&mut book.documents);
    let owner = id_owners(&book.documents);
    book.toc = ncx(pdb, &header, &|entry| {
        let position = entry.tag(1)?.first()?;
        let id = format!("filepos{position}");
        owner.get(&id).map(|path| (path.clone(), Some(id)))
    });
    if book.toc.is_empty() {
        book.toc = headings_toc(&book.documents);
    }
    Ok(book)
}

/// The class of the `<hr>` that stands for `<mbp:pagebreak/>`.
const PAGEBREAK: &str = "mbp-pagebreak";

/// Mobipocket's own tags, which HTML would read as elements that never end: a page break becomes
/// an `<hr>` to cut at, and the others go, their content staying.
fn mobipocket_tags(html: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(html.len());
    let mut at = 0;
    while let Some(found) = html[at..].iter().position(|&b| b == b'<') {
        let start = at + found;
        out.extend_from_slice(&html[at..start]);
        let tag = &html[start..];
        let lower: Vec<u8> = tag.iter().take(20).map(u8::to_ascii_lowercase).collect();
        let is_mbp = lower.starts_with(b"<mbp:") || lower.starts_with(b"</mbp:");
        if !is_mbp {
            out.push(b'<');
            at = start + 1;
            continue;
        }
        let end = tag
            .iter()
            .position(|&b| b == b'>')
            .map_or(tag.len(), |p| p + 1);
        if lower.starts_with(b"<mbp:pagebreak") {
            out.extend_from_slice(format!("<hr class=\"{PAGEBREAK}\"/>").as_bytes());
        }
        at = start + end;
    }
    out.extend_from_slice(&html[at..]);
    out
}

fn id_owners(documents: &[SourceDocument]) -> HashMap<String, String> {
    let mut owner = HashMap::new();
    for document in documents {
        for node in document.dom.descendants(Dom::DOCUMENT) {
            if let Some(id) = document.dom.attr(node, "id") {
                owner
                    .entry(id.to_owned())
                    .or_insert_with(|| document.path.clone());
            }
        }
    }
    owner
}

/// A table of contents from `<h1>` and `<h2>`, for books that have no other.
fn headings_toc(documents: &[SourceDocument]) -> Vec<SourceNav> {
    let mut toc = Vec::new();
    for document in documents {
        let dom = &document.dom;
        for node in dom.descendants(Dom::DOCUMENT) {
            if !(dom.is_html(node, "h1") || dom.is_html(node, "h2")) {
                continue;
            }
            let label = fold_space(&dom.text_content(node));
            if label.is_empty() {
                continue;
            }
            let fragment = dom.attr(node, "id").map(str::to_owned);
            toc.push(SourceNav {
                label,
                target: Some((document.path.clone(), fragment)),
                ..SourceNav::default()
            });
        }
    }
    toc
}

fn base32(text: &[u8]) -> Option<usize> {
    let mut value = 0usize;
    for &byte in text {
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'A'..=b'V' => byte - b'A' + 10,
            b'a'..=b'v' => byte - b'a' + 10,
            _ => return None,
        };
        value = value.checked_mul(32)?.checked_add(usize::from(digit))?;
    }
    Some(value)
}

/// One assembled KF8 file and where it is in the text.
struct Kf8Part {
    markup: Vec<u8>,
    /// Its skeleton's position in the text, which fragment positions count from.
    start: usize,
    end: usize,
    path: String,
}

/// A fragment: where it is inserted, and its file.
struct Fragment {
    insert: usize,
}

fn kf8_book(
    pdb: &Pdb<'_>,
    header: Header,
    mobi6: Option<&Header>,
    limits: &Limits,
) -> Result<SourceBook> {
    let mut book = SourceBook::new(SourceFormat::Azw3);
    book.version = Some(header.version.to_string());
    metadata(&header, &mut book);
    let raw = text(pdb, &header, limits)?;
    let mut paths = resources(pdb, &header, &mut book);
    // In a joint file the pictures may be where the MOBI 6 half put them.
    if paths.is_empty()
        && let Some(mobi6) = mobi6
    {
        paths = resources(pdb, mobi6, &mut book);
    }
    book.cover = cover(&header, &paths);

    // Flows: the first is the markup, the others stylesheets and the like.
    let mut flows: Vec<(usize, usize)> = vec![(0, raw.len())];
    if let Some(index) = header.record(header.fdst_index)
        && let Some(record) = pdb.record(index)
        && record.starts_with(b"FDST")
    {
        let count = u32_at(record, 8).unwrap_or(0) as usize;
        let mut found = Vec::new();
        for i in 0..count.min(10_000) {
            let (Some(start), Some(end)) = (u32_at(record, 12 + 8 * i), u32_at(record, 16 + 8 * i))
            else {
                break;
            };
            let (start, end) = (start as usize, (end as usize).min(raw.len()));
            if start <= end {
                found.push((start, end));
            }
        }
        if !found.is_empty() {
            flows = found;
        }
    }
    let markup_flow = &raw[flows[0].0..flows[0].1];

    // Skeletons and fragments.
    let skeletons = header
        .record(header.skeleton_index)
        .map(|first| read_index(pdb, first))
        .transpose()?
        .map(|(entries, _)| entries)
        .unwrap_or_default();
    let fragments_index = header
        .record(header.fragment_index)
        .map(|first| read_index(pdb, first))
        .transpose()?
        .map(|(entries, _)| entries)
        .unwrap_or_default();
    let mut fragments: Vec<Fragment> = Vec::new();
    let mut fragment_data: Vec<(usize, usize, usize)> = Vec::new();
    for entry in &fragments_index {
        let insert = std::str::from_utf8(&entry.name)
            .ok()
            .and_then(|n| n.trim().parse::<usize>().ok())
            .ok_or_else(|| malformed("a fragment position"))?;
        let position = entry
            .tag(6)
            .ok_or_else(|| malformed("a fragment without a position"))?;
        let (start, length) = (
            *position.first().unwrap_or(&0) as usize,
            *position.get(1).unwrap_or(&0) as usize,
        );
        fragments.push(Fragment { insert });
        fragment_data.push((insert, start, length));
    }
    let mut parts: Vec<Kf8Part> = Vec::new();
    let mut next_fragment = 0usize;
    for (number, skeleton) in skeletons.iter().enumerate() {
        let count = skeleton
            .tag(1)
            .and_then(|v| v.first())
            .copied()
            .unwrap_or(0) as usize;
        let position = skeleton
            .tag(6)
            .ok_or_else(|| malformed("a skeleton without a position"))?;
        let (start, length) = (
            *position.first().unwrap_or(&0) as usize,
            *position.get(1).unwrap_or(&0) as usize,
        );
        let end = start
            .checked_add(length)
            .filter(|&end| end <= markup_flow.len())
            .ok_or_else(|| malformed("a skeleton outside the text"))?;
        let mut assembled = markup_flow[start..end].to_vec();
        let mut base = end;
        for _ in 0..count {
            let Some(&(insert, _, length)) = fragment_data.get(next_fragment) else {
                break;
            };
            next_fragment += 1;
            let piece = markup_flow
                .get(base..base + length)
                .ok_or_else(|| malformed("a fragment outside the text"))?;
            let at = insert
                .checked_sub(start)
                .filter(|&at| at <= assembled.len())
                .ok_or_else(|| malformed("a fragment inserted outside its skeleton"))?;
            assembled.splice(at..at, piece.iter().copied());
            base += length;
        }
        parts.push(Kf8Part {
            markup: assembled,
            start,
            end: base,
            path: format!("text/part{number:04}.xhtml"),
        });
    }
    if parts.is_empty() {
        // No skeleton index: the whole flow is one file.
        parts.push(Kf8Part {
            markup: markup_flow.to_vec(),
            start: 0,
            end: markup_flow.len(),
            path: "text/part0000.xhtml".to_owned(),
        });
    }

    // Stylesheets and other flows become files of their own.
    for (index, &(start, end)) in flows.iter().enumerate().skip(1) {
        let data = raw[start..end].to_vec();
        let is_svg = data.windows(4).take(256).any(|w| w == b"<svg");
        book.resources.push(SourceResource {
            path: format!("text/flow{index:04}.{}", if is_svg { "svg" } else { "css" }),
            media_type: if is_svg {
                "image/svg+xml".to_owned()
            } else {
                "text/css".to_owned()
            },
            data,
        });
    }

    // The target of a kindle:pos link: the file, and the nearest named element before it.
    let mut aid_targets: HashSet<String> = HashSet::new();
    let locate = |fid: usize,
                  offset: usize,
                  aid_targets: &mut HashSet<String>|
     -> Option<(String, Option<String>)> {
        let insert = fragments.get(fid)?.insert;
        let position = insert + offset;
        let part = parts
            .iter()
            .find(|part| position >= part.start && position < part.end)
            .or(parts.last())?;
        let local = position.checked_sub(part.start)?.min(part.markup.len());
        let mut search_end = local;
        // Inside a tag: the whole tag counts.
        let next_open = part.markup[local..]
            .iter()
            .position(|&b| b == b'<')
            .map(|p| local + p);
        let next_close = part.markup[local..]
            .iter()
            .position(|&b| b == b'>')
            .map(|p| local + p);
        if let Some(close) = next_close
            && (next_open == Some(local) || next_open.is_none_or(|open| close < open))
        {
            search_end = close + 1;
        }
        let block = &part.markup[..search_end];
        for attribute in [&b" aid="[..], &b" id="[..], &b" name="[..]] {
            if let Some(value) = last_attribute(block, attribute) {
                if attribute == b" aid=" {
                    aid_targets.insert(value.clone());
                }
                return Some((part.path.clone(), Some(value)));
            }
        }
        Some((part.path.clone(), None))
    };

    // Links, pictures and flows, rewritten in the markup before it is parsed.
    let mut rewritten: Vec<Vec<u8>> = Vec::with_capacity(parts.len());
    for part in &parts {
        let mut out = Vec::with_capacity(part.markup.len());
        let mut rest = part.markup.as_slice();
        while let Some(at) = find(rest, b"kindle:") {
            out.extend_from_slice(&rest[..at]);
            let tail = &rest[at..];
            let end = tail
                .iter()
                .position(|&b| matches!(b, b'"' | b'\'' | b')' | b' ' | b'>' | b'<'))
                .unwrap_or(tail.len());
            let reference = &tail[..end];
            out.extend_from_slice(
                rewrite_kindle(
                    reference,
                    &paths,
                    &mut |fid, offset| locate(fid, offset, &mut aid_targets),
                    &part.path,
                )
                .as_bytes(),
            );
            rest = &tail[end..];
        }
        out.extend_from_slice(rest);
        rewritten.push(out);
    }
    for resource in &mut book.resources {
        if resource.media_type == "text/css" && find(&resource.data, b"kindle:").is_some() {
            let mut out = Vec::new();
            let mut rest = resource.data.as_slice();
            while let Some(at) = find(rest, b"kindle:") {
                out.extend_from_slice(&rest[..at]);
                let tail = &rest[at..];
                let end = tail
                    .iter()
                    .position(|&b| matches!(b, b'"' | b'\'' | b')' | b' ' | b';'))
                    .unwrap_or(tail.len());
                out.extend_from_slice(
                    rewrite_kindle(&tail[..end], &paths, &mut |_, _| None, &resource.path)
                        .as_bytes(),
                );
                rest = &tail[end..];
            }
            out.extend_from_slice(rest);
            resource.data = out;
        }
    }

    for (part, markup) in parts.iter().zip(rewritten) {
        let text = decode(&markup, header.utf8);
        let mut warning = None;
        let (dom, parsed) = match parse_markup(&text, Markup::Xhtml, limits, &mut warning) {
            Some(found) => found,
            None => {
                book.warnings
                    .push(format!("{}: {}", part.path, warning.unwrap_or_default()));
                let (dom, _) = empty_page(&part.path, None);
                (dom, Parsed::Generated)
            }
        };
        book.documents.push(SourceDocument {
            path: part.path.clone(),
            id: None,
            linear: true,
            properties: Vec::new(),
            dom,
            parsed,
            svg: false,
        });
    }

    // Elements linked by `aid` get it as their `id`, or links use the id they have.
    let mut aliases: HashMap<String, String> = HashMap::new();
    for document in &mut book.documents {
        let nodes: Vec<NodeId> = document.dom.descendants(Dom::DOCUMENT).collect();
        for node in nodes {
            let Some(aid) = document.dom.attr(node, "aid").map(str::to_owned) else {
                continue;
            };
            if aid_targets.contains(&aid) {
                match document.dom.attr(node, "id") {
                    Some(id) => {
                        aliases.insert(aid.clone(), id.to_owned());
                    }
                    None => document.dom.set_attr(node, Name::plain("id"), aid.clone()),
                }
            }
            document.dom.remove_attr(node, &crate::dom::Ns::None, "aid");
        }
    }
    if !aliases.is_empty() {
        for document in &mut book.documents {
            let nodes: Vec<NodeId> = document.dom.descendants(Dom::DOCUMENT).collect();
            for node in nodes {
                let Some(attrs) = document.dom.attrs_mut(node) else {
                    continue;
                };
                for attr in attrs.iter_mut().filter(|a| a.name.local == "href") {
                    if let Some((file, fragment)) = attr.value.split_once('#')
                        && let Some(id) = aliases.get(fragment)
                    {
                        attr.value = format!("{file}#{id}");
                    }
                }
            }
        }
    }

    let mut ncx_targets: Vec<Option<(String, Option<String>)>> = Vec::new();
    if let Some(first) = header.record(header.ncx_index)
        && let Ok((entries, _)) = read_index(pdb, first)
    {
        for entry in &entries {
            ncx_targets.push(entry.tag(6).and_then(|v| {
                let (fid, offset) = (*v.first()? as usize, *v.get(1).unwrap_or(&0) as usize);
                let mut ignored = HashSet::new();
                locate(fid, offset, &mut ignored)
            }));
        }
    }
    let targets = std::cell::RefCell::new(ncx_targets.into_iter());
    book.toc = ncx(pdb, &header, &|_| targets.borrow_mut().next().flatten());
    // A target named by an aid now has it as an id, or an alias.
    fn fix(points: &mut [SourceNav], aliases: &HashMap<String, String>) {
        for point in points {
            if let Some((_, Some(fragment))) = &mut point.target
                && let Some(id) = aliases.get(fragment.as_str())
            {
                *fragment = id.clone();
            }
            fix(&mut point.children, aliases);
        }
    }
    fix(&mut book.toc, &aliases);
    if book.toc.is_empty() {
        book.toc = headings_toc(&book.documents);
    }
    Ok(book)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// The value of the last `attribute` (with its leading space and `=`) in `block`, quoted either way.
fn last_attribute(block: &[u8], attribute: &[u8]) -> Option<String> {
    let lower: Vec<u8> = block.to_ascii_lowercase();
    let mut at = lower.len();
    while let Some(found) = lower[..at]
        .windows(attribute.len())
        .rposition(|w| w == attribute)
    {
        // Only inside a tag: no `>` between the last `<` and here.
        let open = lower[..found].iter().rposition(|&b| b == b'<');
        let close = lower[..found].iter().rposition(|&b| b == b'>');
        if open.is_some_and(|open| close.is_none_or(|close| close < open)) {
            let value_start = found + attribute.len();
            let quote = *block.get(value_start)?;
            if quote == b'"' || quote == b'\'' {
                let rest = &block[value_start + 1..];
                let end = rest.iter().position(|&b| b == quote)?;
                return Some(String::from_utf8_lossy(&rest[..end]).into_owned());
            }
        }
        at = found;
    }
    None
}

/// A `kindle:` reference as a path in the normalized book's source tree, relative to `from`.
fn rewrite_kindle(
    reference: &[u8],
    paths: &HashMap<usize, String>,
    locate: &mut dyn FnMut(usize, usize) -> Option<(String, Option<String>)>,
    from: &str,
) -> String {
    let text = String::from_utf8_lossy(reference);
    let relative = |path: &str| crate::url::relative(from, path);
    if let Some(rest) = text.strip_prefix("kindle:embed:") {
        let digits = rest.split(['?', '#']).next().unwrap_or("");
        if let Some(path) = base32(digits.as_bytes())
            .and_then(|n| n.checked_sub(1))
            .and_then(|n| paths.get(&n))
        {
            return relative(path);
        }
        return String::new();
    }
    if let Some(rest) = text.strip_prefix("kindle:flow:") {
        let digits = rest.split(['?', '#']).next().unwrap_or("");
        let svg = rest.contains("svg");
        if let Some(number) = base32(digits.as_bytes()) {
            return relative(&format!(
                "text/flow{number:04}.{}",
                if svg { "svg" } else { "css" }
            ));
        }
        return String::new();
    }
    if let Some(rest) = text.strip_prefix("kindle:pos:fid:") {
        let mut pieces = rest.split(':');
        let fid = pieces.next().and_then(|d| base32(d.as_bytes()));
        let offset = match (pieces.next(), pieces.next()) {
            (Some("off"), Some(digits)) => base32(digits.as_bytes()),
            _ => Some(0),
        };
        if let (Some(fid), Some(offset)) = (fid, offset)
            && let Some((path, fragment)) = locate(fid, offset)
        {
            let mut target = relative(&path);
            if let Some(fragment) = fragment {
                target.push('#');
                target.push_str(&crate::url::encode_fragment(&fragment));
            }
            return target;
        }
        return String::new();
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palmdoc_round_trips_its_three_kinds_of_code() {
        // "abc", a space pair, a back reference of 3 bytes at distance 3, literals with a count.
        let compressed = [b'a', b'b', b'c', 0xC0, 0x80, 0x28, 0x02, 0xE9, 0xE8];
        let mut out = Vec::new();
        palmdoc(&compressed, &mut out, 1000).unwrap();
        assert_eq!(out, b"abc @abc\xE9\xE8");
    }

    #[test]
    fn a_bad_reference_is_an_error() {
        let mut out = Vec::new();
        assert!(palmdoc(&[0x80, 0x18], &mut out, 1000).is_err());
    }

    #[test]
    fn trailing_entries_are_measured_from_the_end() {
        // Text, a multibyte byte (1 extra byte), and one trailing entry 3 bytes long.
        let record = [b'h', b'i', 0x7A, 0x00, b'x', b'y', 0x83];
        assert_eq!(trailing_size(&record, 0b11), 4);
        assert_eq!(trailing_size(&record, 0b10), 3);
        assert_eq!(trailing_size(&record, 0), 0);
    }

    #[test]
    fn anchors_never_land_inside_a_tag_or_an_entity() {
        let html = b"<p class=\"x\">caf&eacute; and more</p>";
        let out = add_anchors(html, &[3, 18]);
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.starts_with("<p class=\"x\"><a id=\"filepos3\"></a>caf"),
            "{text}"
        );
        assert!(text.contains("&eacute;<a id=\"filepos18\"></a>"), "{text}");
    }

    #[test]
    fn mobipocket_tags_do_not_swallow_the_book() {
        let out = mobipocket_tags(b"<p>a</p><mbp:pagebreak/><mbp:nu>b</mbp:nu><MBP:PAGEBREAK >c");
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "<p>a</p><hr class=\"mbp-pagebreak\"/>b<hr class=\"mbp-pagebreak\"/>c"
        );
    }

    #[test]
    fn base32_reads_kindle_numbers() {
        assert_eq!(base32(b"0001"), Some(1));
        assert_eq!(base32(b"000V"), Some(31));
        assert_eq!(base32(b"0010"), Some(32));
        assert_eq!(base32(b"00X0"), None);
    }

    #[test]
    fn garbage_is_refused_without_panic() {
        assert!(read(b"short", &Limits::default()).is_err());
        let mut fake = vec![0u8; 100];
        fake[60..68].copy_from_slice(b"BOOKMOBI");
        fake[76..78].copy_from_slice(&5u16.to_be_bytes());
        assert!(read(&fake, &Limits::default()).is_err());
    }
}

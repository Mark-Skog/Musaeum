//! Page thumbnails as JPEG, for the cache on disk.
//!
//! A worker hands over pixels and the host encodes them, so a compromised worker can at worst make
//! a thumbnail look wrong; what lands on disk, and is decoded later, is always this encoder's output.

use jpeg_encoder::{ColorType, Encoder};

/// JPEG quality of a thumbnail: small, and sharp enough for a page navigator.
pub const THUMBNAIL_QUALITY: u8 = 82;

/// Encodes `width` × `height` RGBA pixels, rows packed, as a JPEG. Alpha is ignored: rendered
/// pages are opaque.
pub fn encode_jpeg(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let (Ok(w), Ok(h)) = (u16::try_from(width), u16::try_from(height)) else {
        return Err(format!("a {width} × {height} thumbnail is too large"));
    };
    if rgba.len() != width as usize * height as usize * 4 || width == 0 || height == 0 {
        return Err("the pixels do not match the thumbnail's size".to_owned());
    }
    let mut out = Vec::with_capacity(rgba.len() / 10);
    Encoder::new(&mut out, THUMBNAIL_QUALITY)
        .encode(rgba, w, h, ColorType::Rgba)
        .map_err(|error| error.to_string())?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thumbnail_is_a_small_jpeg() {
        let (width, height) = (181u32, 256u32);
        let rgba: Vec<u8> = (0..width * height)
            .flat_map(|at| [(at % 251) as u8, 128, 255, 255])
            .collect();
        let jpeg = encode_jpeg(width, height, &rgba).unwrap();
        assert!(jpeg.starts_with(&[0xFF, 0xD8]) && jpeg.ends_with(&[0xFF, 0xD9]));
        assert!(jpeg.len() < rgba.len() / 4);
        assert!(encode_jpeg(width, height, &rgba[4..]).is_err());
        assert!(encode_jpeg(70_000, 1, &[]).is_err());
    }
}

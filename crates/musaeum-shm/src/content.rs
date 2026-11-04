//! What a page is made of: text the document describes, a picture of the page, or a picture with a
//! text layer over it, as OCR leaves one.
//!
//! A worker only measures. The class is decided here, from those measurements, so that every
//! format is classified by the same rules and a worker cannot simply claim a class.

/// How a page's content was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ContentClass {
    /// Text and drawings the document describes. Also a page with neither text nor pictures,
    /// since nothing on it was scanned.
    Text,
    /// A picture of the page, with no text layer.
    Scanned,
    /// A picture of the page with a text layer, which is what OCR leaves in a file.
    Hybrid,
}

impl ContentClass {
    /// The name the cache stores.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Scanned => "scanned",
            Self::Hybrid => "hybrid",
        }
    }

    /// The class with this cache name.
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "text" => Self::Text,
            "scanned" => Self::Scanned,
            "hybrid" => Self::Hybrid,
            _ => return None,
        })
    }
}

/// A page is a picture of itself when pictures cover at least this fraction of it. Scanners
/// produce pages that one picture covers entirely; a figure in a born-digital book rarely covers
/// more than half of its page.
pub const PICTURE_COVERAGE: f32 = 0.85;

/// A text layer needs at least this many characters to count, so that a page number stamped on a
/// scan does not make it hybrid.
pub const TEXT_LAYER_CHARS: u32 = 16;

/// What a worker measured on one page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageContent {
    /// Characters in the page's text layer, white space not counted.
    pub text_chars: u32,
    /// Fraction of the page that raster images cover, from 0 to 1.
    pub image_coverage: f32,
}

impl PageContent {
    /// Whether a worker's measurements can be true at all.
    pub fn is_plausible(&self) -> bool {
        self.image_coverage.is_finite() && (0.0..=1.0).contains(&self.image_coverage)
    }

    /// The page's class.
    pub fn class(&self) -> ContentClass {
        if self.image_coverage < PICTURE_COVERAGE {
            ContentClass::Text
        } else if self.text_chars >= TEXT_LAYER_CHARS {
            ContentClass::Hybrid
        } else {
            ContentClass::Scanned
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content(text_chars: u32, image_coverage: f32) -> PageContent {
        PageContent {
            text_chars,
            image_coverage,
        }
    }

    #[test]
    fn a_picture_of_the_page_is_scanned_unless_a_text_layer_covers_it() {
        assert_eq!(content(0, 1.0).class(), ContentClass::Scanned);
        assert_eq!(
            content(8, 1.0).class(),
            ContentClass::Scanned,
            "a stamped page number is not a text layer"
        );
        assert_eq!(content(2_000, 0.97).class(), ContentClass::Hybrid);
    }

    #[test]
    fn pages_without_a_page_sized_picture_are_text() {
        assert_eq!(content(3_000, 0.0).class(), ContentClass::Text);
        assert_eq!(content(400, 0.5).class(), ContentClass::Text);
        assert_eq!(content(0, 0.0).class(), ContentClass::Text, "a blank page");
    }

    #[test]
    fn impossible_coverage_is_refused() {
        assert!(content(0, 0.5).is_plausible());
        assert!(!content(0, 1.5).is_plausible());
        assert!(!content(0, -0.1).is_plausible());
        assert!(!content(0, f32::NAN).is_plausible());
    }

    #[test]
    fn names_round_trip() {
        for class in [
            ContentClass::Text,
            ContentClass::Scanned,
            ContentClass::Hybrid,
        ] {
            assert_eq!(ContentClass::from_name(class.as_str()), Some(class));
        }
        assert_eq!(ContentClass::from_name("texto"), None);
    }
}

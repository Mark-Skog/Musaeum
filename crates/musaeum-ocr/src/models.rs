//! The recognition models this program trusts: each one by its name and the SHA-256 of its bytes,
//! the same hashes `tools/fetch-ocr.ps1` checks when it downloads them.
//!
//! The host reads a model from the program's own `tessdata` folder, checks its bytes here, and only
//! then hands them to the worker, which opens no file of its own in its sandbox. A model in that
//! folder that is not here, or whose bytes are not these, is never given to it.

use sha2::{Digest, Sha256};

/// A model this program knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KnownModel {
    /// Tesseract's name for it (`por`), the file's name without `.traineddata`.
    pub code: &'static str,
    /// Its language as BCP 47 names it, for the interface to name it in its own language.
    pub bcp47: &'static str,
    /// The SHA-256 of the file, in lowercase hexadecimal.
    pub sha256: &'static str,
    /// Whether it is shipped (downloaded by default), or only on request.
    pub shipped: bool,
}

/// Every model this program trusts: tessdata_fast 4.1.0 (Apache-2.0).
pub const KNOWN_MODELS: &[KnownModel] = &[
    KnownModel {
        code: "por",
        bcp47: "pt",
        sha256: "c4932b937207a9514b7514d518b931a99938c02a28a5a5a553f8599ed58b7deb",
        shipped: true,
    },
    KnownModel {
        code: "eng",
        bcp47: "en",
        sha256: "7d4322bd2a7749724879683fc3912cb542f19906c83bcc1a52132556427170b2",
        shipped: true,
    },
    KnownModel {
        code: "spa",
        bcp47: "es",
        sha256: "6f2e04d02774a18f01bed44b1111f2cd7f3ba7ac9dc4373cd3f898a40ea6b464",
        shipped: false,
    },
    KnownModel {
        code: "fra",
        bcp47: "fr",
        sha256: "ced037562e8c80c13122dece28dd477d399af80911a28791a66a63ac1e3445ca",
        shipped: false,
    },
    KnownModel {
        code: "deu",
        bcp47: "de",
        sha256: "19d219bbb6672c869d20a9636c6816a81eb9a71796cb93ebe0cb1530e2cdb22d",
        shipped: false,
    },
    KnownModel {
        code: "ita",
        bcp47: "it",
        sha256: "b8f89e1e785118dac4d51ae042c029a64edb5c3ee42ef73027a6d412748d8827",
        shipped: false,
    },
    KnownModel {
        code: "lat",
        bcp47: "la",
        sha256: "3859d8ba60404f4b79830622625bbc76fb4ee2808eac1ad360ffa77f0a533328",
        shipped: false,
    },
];

/// A model file found in the models' folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelFile {
    pub code: String,
    pub bytes: u64,
    /// The model this program knows by that name, if it is one.
    pub known: Option<KnownModel>,
    /// Whether its bytes are the ones pinned for that name.
    pub verified: bool,
}

/// Whether `bytes` are the model this program trusts under `code`.
pub fn verified(code: &str, bytes: &[u8]) -> bool {
    let Some(known) = KNOWN_MODELS.iter().find(|model| model.code == code) else {
        return false;
    };
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
        hex.push(char::from_digit(u32::from(byte & 15), 16).unwrap_or('0'));
    }
    hex == known.sha256
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_model_is_trusted_only_by_its_name_and_its_bytes() {
        // The SHA-256 of nothing, pinned under no name.
        assert!(!verified("por", b""));
        assert!(!verified("xyz", b""));
        let codes: Vec<&str> = KNOWN_MODELS.iter().map(|model| model.code).collect();
        let mut unique = codes.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), codes.len(), "each name once");
        for model in KNOWN_MODELS {
            assert_eq!(model.sha256.len(), 64);
            assert!(
                model
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            );
        }
    }

    #[test]
    fn the_shipped_models_are_the_ones_in_vendor() {
        let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../vendor/tesseract/tessdata");
        for model in KNOWN_MODELS.iter().filter(|model| model.shipped) {
            let bytes = std::fs::read(folder.join(format!("{}.traineddata", model.code))).unwrap();
            assert!(verified(model.code, &bytes), "{}", model.code);
        }
    }
}

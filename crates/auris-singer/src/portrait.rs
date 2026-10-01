//! Bounded artwork supplied by singing services.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use std::sync::Arc;

/// Maximum encoded image size.
pub const PORTRAIT_MAX_BYTES: usize = 8 * 1024 * 1024;
const PORTRAIT_MAX_BASE64: usize = PORTRAIT_MAX_BYTES.div_ceil(3) * 4;

/// Shared encoded PNG, JPEG or WebP artwork supplied by a singing service.
///
/// These are image-file bytes, not pixels. A frontend decodes them off its UI thread, with its
/// own pixel-dimension limit; a corrupt image must never prevent the voice from singing.
#[derive(Clone, PartialEq, Eq)]
pub struct VoicePortrait {
    mime: &'static str,
    bytes: Arc<[u8]>,
}

impl std::fmt::Debug for VoicePortrait {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VoicePortrait")
            .field("mime", &self.mime)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

impl VoicePortrait {
    /// Accepts supported, nonempty artwork within the size limit.
    ///
    /// Image decoding is the frontend's responsibility. Invalid optional artwork returns
    /// `None`, so it can be omitted without affecting synthesis or speaker selection.
    pub fn from_bytes(mime: &str, bytes: Vec<u8>) -> Option<Self> {
        let mime = supported_mime(mime)?;
        if bytes.is_empty() || bytes.len() > PORTRAIT_MAX_BYTES {
            return None;
        }
        Some(Self {
            mime,
            bytes: bytes.into(),
        })
    }

    /// Decodes a service image encoded as base64.
    ///
    /// The encoded length is checked before allocating decoded bytes. Oversized, empty,
    /// malformed base64 and unsupported MIME types are all treated as absent artwork.
    pub fn from_base64(mime: &str, encoded: &str) -> Option<Self> {
        supported_mime(mime)?;
        if encoded.is_empty() || encoded.len() > PORTRAIT_MAX_BASE64 {
            return None;
        }
        Self::from_bytes(mime, STANDARD.decode(encoded).ok()?)
    }

    /// The image's declared MIME type: `image/png`, `image/jpeg` or `image/webp`.
    pub fn mime(&self) -> &'static str {
        self.mime
    }

    /// Shared image-file bytes; cloning the `Arc` does not copy the image.
    pub fn bytes(&self) -> &Arc<[u8]> {
        &self.bytes
    }
}

fn supported_mime(mime: &str) -> Option<&'static str> {
    match mime {
        "image/png" => Some("image/png"),
        "image/jpeg" => Some("image/jpeg"),
        "image/webp" => Some("image/webp"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artwork_supports_image_formats_and_clones_share_bytes() {
        for mime in ["image/png", "image/jpeg", "image/webp"] {
            let portrait = VoicePortrait::from_base64(mime, "AQIDBA==").unwrap();
            assert_eq!(portrait.mime(), mime);
            assert_eq!(portrait.bytes().as_ref(), &[1, 2, 3, 4]);
            assert!(Arc::ptr_eq(portrait.bytes(), portrait.clone().bytes()));
        }
        for (mime, encoded) in [
            ("image/svg+xml", "AQIDBA=="),
            ("image/png", ""),
            ("image/png", "not base64"),
            ("image/png", "AQIDBA"),
        ] {
            assert!(VoicePortrait::from_base64(mime, encoded).is_none());
        }
    }

    #[test]
    fn artwork_size_is_bounded_before_and_after_base64_decoding() {
        let bytes = vec![42; PORTRAIT_MAX_BYTES];
        let encoded = STANDARD.encode(&bytes);
        assert_eq!(
            VoicePortrait::from_base64("image/png", &encoded)
                .unwrap()
                .bytes()
                .len(),
            PORTRAIT_MAX_BYTES
        );
        // One extra decoded byte can still have the same encoded length because of padding.
        let oversized = STANDARD.encode(vec![42; PORTRAIT_MAX_BYTES + 1]);
        assert_eq!(encoded.len(), oversized.len());
        assert!(VoicePortrait::from_base64("image/png", &oversized).is_none());
        assert!(
            VoicePortrait::from_base64("image/png", &"A".repeat(oversized.len() + 4)).is_none()
        );
        assert!(VoicePortrait::from_bytes("image/png", Vec::new()).is_none());
    }
}

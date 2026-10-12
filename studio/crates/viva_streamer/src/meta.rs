//! Pure helpers for the `image/meta` Zenoh key and `ImageMeta` payloads.
//!
//! These functions are deliberately I/O-free so they can be unit-tested
//! without a Zenoh session.

use viva_pfnc::PixelFormat;
use viva_zenoh_api::ImageMeta;

/// Derive the `image/meta` key from an image key expression.
///
/// Strips a trailing `/` if present and appends `/meta`.
///
/// ```text
/// "genicam/devices/cam0/image"  → "genicam/devices/cam0/image/meta"
/// "genicam/devices/cam0/image/" → "genicam/devices/cam0/image/meta"
/// ```
pub fn derive_meta_key(image_key: &str) -> String {
    let key = image_key.trim_end_matches('/');
    format!("{key}/meta")
}

/// Build an `ImageMeta` using Mono8 as the default pixel format.
///
/// `payload_size` is set to `width * height` (one byte per pixel for Mono8).
pub fn default_image_meta(width: u32, height: u32) -> ImageMeta {
    ImageMeta::new(
        PixelFormat::Mono8,
        width,
        height,
        width as u64 * height as u64,
    )
}

/// Deserialize an `ImageMeta` from a raw Zenoh payload (JSON bytes).
pub fn parse_meta_payload(bytes: &[u8]) -> Result<ImageMeta, serde_json::Error> {
    serde_json::from_slice(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_meta_key_standard() {
        let result = derive_meta_key("genicam/devices/cam0/image");
        assert_eq!(result, "genicam/devices/cam0/image/meta");
    }

    #[test]
    fn test_derive_meta_key_trailing_slash() {
        let result = derive_meta_key("genicam/devices/cam0/image/");
        assert_eq!(result, "genicam/devices/cam0/image/meta");
    }

    #[test]
    fn test_parse_meta_payload_valid() {
        let json = r#"{"pixel_format":"Mono8","width":1920,"height":1080,"payload_size":2073600}"#;
        let meta = parse_meta_payload(json.as_bytes()).expect("should parse");
        assert_eq!(meta.width, 1920);
        assert_eq!(meta.height, 1080);
        assert_eq!(meta.payload_size, 2073600);
        assert_eq!(meta.pixel_format, PixelFormat::Mono8);
    }

    #[test]
    fn test_parse_meta_payload_invalid_json() {
        let result = parse_meta_payload(b"not json");
        assert!(result.is_err());
    }

    #[test]
    fn test_default_image_meta_dimensions() {
        let meta = default_image_meta(320, 240);
        assert_eq!(meta.width, 320);
        assert_eq!(meta.height, 240);
        assert_eq!(meta.payload_size, 76800);
        assert_eq!(meta.pixel_format, PixelFormat::Mono8);
    }
}

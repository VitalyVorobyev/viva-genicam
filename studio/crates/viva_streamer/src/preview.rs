//! Atomic preview envelope: metadata and its BMP travel in one WS message.

use bytes::Bytes;
use viva_zenoh_api::{FrameMetadata, MAX_FRAME_METADATA_BYTES};

/// Bound metadata independently of the image dimensions.
const MAX_METADATA_BYTES: usize = MAX_FRAME_METADATA_BYTES;

/// `VFRM`, a u32 LE JSON length, frame metadata, then the bare BMP.
/// A source with no metadata retains the legacy bare-BMP representation.
pub fn encode(bmp: &Bytes, metadata: Option<&FrameMetadata>) -> Result<Bytes, String> {
    let Some(metadata) = metadata else {
        return Ok(bmp.clone());
    };
    let json = metadata.encode().map_err(|error| error.to_string())?;
    if json.len() > MAX_METADATA_BYTES {
        return Err("Preview frame metadata exceeds 64 KiB".into());
    }
    let size = 8usize
        .checked_add(json.len())
        .and_then(|length| length.checked_add(bmp.len()))
        .ok_or_else(|| "Preview frame size overflow".to_string())?;
    let mut output = Vec::with_capacity(size);
    output.extend_from_slice(b"VFRM");
    output.extend_from_slice(&(json.len() as u32).to_le_bytes());
    output.extend_from_slice(&json);
    output.extend_from_slice(bmp);
    Ok(output.into())
}

/// Split a preview message without copying its BMP. Used by native consumers.
pub fn decode(bytes: &[u8]) -> Result<(Option<FrameMetadata>, &[u8]), String> {
    if bytes.starts_with(b"BM") {
        return Ok((None, bytes));
    }
    if bytes.len() < 8 || !bytes.starts_with(b"VFRM") {
        return Err("Invalid preview frame envelope".into());
    }
    let length = u32::from_le_bytes(bytes[4..8].try_into().expect("four bytes")) as usize;
    if length > MAX_METADATA_BYTES || length > bytes.len() - 8 {
        return Err("Invalid preview metadata length".into());
    }
    let metadata = FrameMetadata::decode(&bytes[8..8 + length])
        .map_err(|error| format!("Invalid preview metadata: {error}"))?;
    let bmp = &bytes[8 + length..];
    if !bmp.starts_with(b"BM") {
        return Err("Preview frame contains no BMP".into());
    }
    Ok((Some(metadata), bmp))
}

/// Normalize once before retaining metadata with a native source. The live
/// envelope and later frozen capture must expose the same bounded values.
pub fn normalize(metadata: FrameMetadata) -> Result<FrameMetadata, String> {
    let bytes = metadata.encode().map_err(|error| error.to_string())?;
    FrameMetadata::decode(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn metadata() -> FrameMetadata {
        FrameMetadata {
            block_id: u64::MAX.to_string(),
            ts_dev: Some((u64::MAX - 1).to_string()),
            ts_host_ns: None,
            complete: true,
            chunks: vec![],
            features: vec![],
            chunk_error: None,
            truncated: false,
        }
    }

    #[test]
    fn preview_keeps_exact_ids_and_metadata_with_the_same_bmp() {
        let bmp = Bytes::from_static(b"BMexample");
        let facts = metadata();
        let encoded = encode(&bmp, Some(&facts)).unwrap();
        let (read, pixels) = decode(&encoded).unwrap();
        assert_eq!(read, Some(facts));
        assert_eq!(pixels, bmp.as_ref());
        let legacy = encode(&bmp, None).unwrap();
        assert_eq!(legacy.as_ptr(), bmp.as_ptr());
        assert_eq!(decode(&legacy).unwrap(), (None, bmp.as_ref()));
    }

    #[test]
    fn preview_rejects_invalid_lengths_json_and_image_signatures() {
        for bytes in [
            b"VFRM".as_slice(),
            b"junk",
            b"VFRM\xff\xff\xff\xff",
            b"VFRM\x02\0\0\0{}BM",
        ] {
            assert!(decode(bytes).is_err());
        }
        let mut encoded = encode(&Bytes::from_static(b"BMimage"), Some(&metadata()))
            .unwrap()
            .to_vec();
        *encoded.last_mut().unwrap() = b'x';
        let length = u32::from_le_bytes(encoded[4..8].try_into().unwrap()) as usize;
        encoded[8 + length] = 0;
        assert!(decode(&encoded).is_err());
    }

    #[test]
    fn normalized_metadata_is_bounded_and_stable_for_live_and_frozen_sources() {
        let large = metadata().with_features(
            (0..200).map(|index| (format!("Chunk{index}"), Ok::<_, String>("😀".repeat(2000)))),
        );
        let normalized = normalize(large).unwrap();
        assert!(normalized.truncated);
        assert!(normalized.encode().unwrap().len() <= MAX_METADATA_BYTES);
        assert_eq!(normalize(normalized.clone()).unwrap(), normalized);
        let encoded = encode(&Bytes::from_static(b"BMimage"), Some(&normalized)).unwrap();
        assert_eq!(decode(&encoded).unwrap().0, Some(normalized));
    }
}

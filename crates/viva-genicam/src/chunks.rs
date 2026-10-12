//! Chunk data in a GigE Vision payload.
//!
//! A camera in chunk mode sends each frame as a run of chunks — the image
//! first, then one chunk per enabled value — each its data followed by a tag
//! naming its chunk ID and length (TC-06, [`viva_gige::parse_chunks`]).
//! What a chunk *means* is not decided here: the camera's own XML declares
//! which registers sit on which chunk, at which offset and in which byte
//! order, and [`Camera::with_chunks`](crate::Camera::with_chunks) reads them
//! through GenApi like any other feature (ADR-0026).

use bytes::Bytes;
pub use viva_genapi::ChunkData;
pub use viva_gige::ChunkError;
use viva_gige::{PAYLOAD_TYPE_CHUNK_DATA, has_extended_chunks, is_image_payload};

/// Split a GigE Vision chunk payload into a [`ChunkData`], by chunk ID.
///
/// `payload` must end where the camera's data ends. Each chunk is a slice of
/// `payload`; nothing is copied.
pub fn chunk_data(payload: &Bytes) -> Result<ChunkData, ChunkError> {
    let chunks = viva_gige::parse_chunks(payload)?;
    Ok(chunks
        .into_iter()
        .map(|chunk| {
            let end = chunk.offset + chunk.data.len();
            (u64::from(chunk.id), payload.slice(chunk.offset..end))
        })
        .collect())
}

/// Whether a block of `payload_type` is made of chunks: chunk data
/// (`0x0004`), or any type with the extended chunk flag (`0x4000`).
pub(crate) fn carries_chunks(payload_type: u16) -> bool {
    has_extended_chunks(payload_type) || payload_type & 0x3FFF == PAYLOAD_TYPE_CHUNK_DATA
}

/// A block's image and chunks, from its whole payload.
///
/// For an image with chunks the image is the first chunk, at offset 0, and is
/// returned on its own; the chunks come back by ID, the image among them. A
/// chunk-data block has no image, so its payload comes back whole. A payload
/// that does not tile into chunks keeps its bytes and has none — the error
/// says where it went wrong.
pub(crate) fn split(
    payload: Bytes,
    payload_type: u16,
) -> (Bytes, Option<Result<ChunkData, ChunkError>>) {
    if !carries_chunks(payload_type) {
        return (payload, None);
    }
    match chunk_data(&payload) {
        Ok(data) => {
            let image = data
                .iter()
                .next()
                .filter(|_| is_image_payload(payload_type))
                .map(|(_, first)| first.clone());
            (image.unwrap_or(payload), Some(Ok(data)))
        }
        Err(err) => (payload, Some(Err(err))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `[image 4][tag id 1, len 4][value 4][tag id 0x0508000E, len 4]`,
    /// written out by hand.
    #[rustfmt::skip]
    const BLOCK: [u8; 24] = [
        0xA0, 0xA1, 0xA2, 0xA3,
        0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x04,
        0x2A, 0x00, 0x00, 0x00,
        0x05, 0x08, 0x00, 0x0E, 0x00, 0x00, 0x00, 0x04,
    ];

    #[test]
    fn an_image_with_chunks_splits_into_the_image_and_the_chunks() {
        let (image, chunks) = split(Bytes::from_static(&BLOCK), 0x4001);
        assert_eq!(&image[..], &[0xA0, 0xA1, 0xA2, 0xA3]);
        let chunks = chunks.expect("chunk flag").expect("well-formed");
        assert_eq!(chunks.ids().collect::<Vec<_>>(), vec![1, 0x0508_000E]);
        assert_eq!(
            &chunks.get(0x0508_000E).expect("value")[..],
            &[0x2A, 0, 0, 0]
        );
    }

    #[test]
    fn a_chunk_data_block_keeps_its_payload_whole() {
        let (payload, chunks) = split(Bytes::from_static(&BLOCK), 0x0004);
        assert_eq!(payload.len(), BLOCK.len());
        assert_eq!(chunks.expect("chunk data").expect("well-formed").len(), 2);
    }

    #[test]
    fn a_plain_image_has_no_chunks() {
        let (payload, chunks) = split(Bytes::from_static(&BLOCK), 0x0001);
        assert_eq!(payload.len(), BLOCK.len());
        assert!(chunks.is_none());
    }

    #[test]
    fn a_malformed_layout_keeps_the_bytes_and_reports_the_error() {
        let (payload, chunks) = split(Bytes::from_static(&BLOCK[1..]), 0x4001);
        assert_eq!(payload.len(), BLOCK.len() - 1);
        // The image chunk's tag now claims one byte more than precedes it.
        assert!(matches!(
            chunks,
            Some(Err(ChunkError::LengthPastStart {
                id: 1,
                tag_offset: 3,
                length: 4
            }))
        ));
    }
}

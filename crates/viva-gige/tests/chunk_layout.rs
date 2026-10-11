//! The GigE Vision chunk layout (TC-06), against byte vectors written out by
//! hand from the layout — never produced by our own encoder or fake camera
//! (ADR-0019).
//!
//! A chunk payload is a run of chunks, each its data followed by an 8-byte tag:
//! chunk ID, then data length, both 32-bit big-endian. It is read from the
//! end. The GigE Vision specification is not available to us; aravis
//! (`arv_buffer_get_chunk_data`) and the vendor corpus's 32-bit `<ChunkID>`s
//! corroborate it.

use viva_gige::gvsp::{ChunkError, ChunkRaw, parse_chunks};

/// An image with two chunks, as a device lays out an extended chunk payload
/// (`0x4001`): the image first, as a chunk of its own, then each chunk's data
/// and tag. The IDs are FLIR's (`ImagePort` 1, `TimestampPort` 0x0508000E,
/// `ExposureTimePort` 0x05040008); the values little-endian, as FLIR declares
/// its chunk registers.
#[rustfmt::skip]
const PAYLOAD: [u8; 44] = [
    // image data, 8 bytes
    0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17,
    // tag: chunk ID 0x00000001, length 8
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x08,
    // ChunkTimestamp data: 0x0000_0001_2345_6789, little-endian
    0x89, 0x67, 0x45, 0x23, 0x01, 0x00, 0x00, 0x00,
    // tag: chunk ID 0x0508000E, length 8
    0x05, 0x08, 0x00, 0x0E, 0x00, 0x00, 0x00, 0x08,
    // ChunkExposureTime data: 5000, little-endian
    0x88, 0x13, 0x00, 0x00,
    // tag: chunk ID 0x05040008, length 4
    0x05, 0x04, 0x00, 0x08, 0x00, 0x00, 0x00, 0x04,
];

/// Shown failing on the unfixed parser (which read `[id u16][reserved u16]
/// [len u32][data]` front to back) with `left: []`.
#[test]
fn spec_layout_parses_backward_with_32_bit_ids() {
    let chunks = parse_chunks(&PAYLOAD).expect("well-formed");
    let got: Vec<(u64, usize, Vec<u8>)> = chunks
        .iter()
        .map(|c| (u64::from(c.id), c.offset, c.data.to_vec()))
        .collect();
    assert_eq!(
        got,
        vec![
            (
                0x0000_0001,
                0,
                vec![0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17]
            ),
            (
                0x0508_000E,
                16,
                vec![0x89, 0x67, 0x45, 0x23, 0x01, 0x00, 0x00, 0x00]
            ),
            (0x0504_0008, 32, vec![0x88, 0x13, 0x00, 0x00]),
        ]
    );
}

/// An empty payload has no chunks; it is not an error.
#[test]
fn an_empty_payload_has_no_chunks() {
    assert_eq!(parse_chunks(&[]), Ok(vec![]));
}

/// A zero-length chunk is a chunk. The scan still steps back over its tag,
/// so it cannot loop on it.
#[test]
fn a_zero_length_chunk_is_a_chunk() {
    #[rustfmt::skip]
    let payload = [
        0xAB, 0xCD, 0xEF, 0x01, 0x00, 0x00, 0x00, 0x00, // chunk 0xABCDEF01, empty
        0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, // chunk 2, empty
    ];
    assert_eq!(
        parse_chunks(&payload),
        Ok(vec![
            ChunkRaw {
                id: 0xABCD_EF01,
                offset: 0,
                data: &[]
            },
            ChunkRaw {
                id: 2,
                offset: 8,
                data: &[]
            },
        ])
    );
}

/// A tag claiming more data than precedes it is an error naming the tag's
/// offset and the length it claims.
#[test]
fn a_length_past_the_start_is_an_error_naming_the_offset() {
    let mut payload = PAYLOAD;
    // The timestamp chunk's tag starts at 24; make it claim 25 bytes.
    payload[28..32].copy_from_slice(&25u32.to_be_bytes());
    let err = parse_chunks(&payload).expect_err("malformed");
    assert_eq!(
        err,
        ChunkError::LengthPastStart {
            id: 0x0508_000E,
            tag_offset: 24,
            length: 25
        }
    );
    assert!(err.to_string().contains("offset 24"), "{err}");

    // A length near u32::MAX, which `start - length` must not wrap around.
    payload[28..32].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(matches!(
        parse_chunks(&payload),
        Err(ChunkError::LengthPastStart {
            tag_offset: 24,
            length: u32::MAX,
            ..
        })
    ));
}

/// Bytes in front of the first tag that are too few for a tag: an image sent
/// without its own tag, say, or a payload cut short.
#[test]
fn a_truncated_tag_is_an_error() {
    let mut payload = vec![0xEE; 5];
    payload.extend_from_slice(&PAYLOAD[16..]);
    assert_eq!(
        parse_chunks(&payload),
        Err(ChunkError::TruncatedTag { remaining: 5 })
    );
    assert_eq!(
        parse_chunks(&[0, 0, 0]),
        Err(ChunkError::TruncatedTag { remaining: 3 })
    );
}

/// Chunks written out with random IDs (the full 32 bits) and lengths parse
/// back to exactly what was written.
#[test]
fn random_chunk_runs_round_trip() {
    for _ in 0..256 {
        let mut payload = Vec::new();
        let mut written = Vec::new();
        for _ in 0..fastrand::usize(..6) {
            let id = fastrand::u32(..);
            let data: Vec<u8> = (0..fastrand::usize(..24))
                .map(|_| fastrand::u8(..))
                .collect();
            let offset = payload.len();
            payload.extend_from_slice(&data);
            payload.extend_from_slice(&id.to_be_bytes());
            payload.extend_from_slice(&(data.len() as u32).to_be_bytes());
            written.push((id, offset, data));
        }
        let parsed: Vec<(u32, usize, Vec<u8>)> = parse_chunks(&payload)
            .expect("well-formed")
            .into_iter()
            .map(|c| (c.id, c.offset, c.data.to_vec()))
            .collect();
        assert_eq!(parsed, written);
    }
}

/// Arbitrary bytes never panic: the parse either tiles the payload exactly or
/// says where it could not.
#[test]
fn arbitrary_bytes_never_panic() {
    for _ in 0..2048 {
        let payload: Vec<u8> = (0..fastrand::usize(..64))
            .map(|_| fastrand::u8(..))
            .collect();
        if let Ok(chunks) = parse_chunks(&payload) {
            let covered: usize = chunks.iter().map(|c| c.data.len() + 8).sum();
            assert_eq!(covered, payload.len());
        }
    }
}

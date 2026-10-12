//! Any chunk payload: `parse_chunks` tiles it exactly into chunks or says
//! where it could not — never a panic, never a loop, never a slice outside
//! the payload (TC-06). Every byte of a chunk payload is remote input.
#![no_main]

use libfuzzer_sys::fuzz_target;
use viva_gige::parse_chunks;

fuzz_target!(|data: &[u8]| {
    let Ok(chunks) = parse_chunks(data) else {
        return;
    };
    let mut expected_offset = 0usize;
    for chunk in &chunks {
        // In payload order, back to back: data, then an 8-byte tag.
        assert_eq!(chunk.offset, expected_offset);
        assert_eq!(
            &data[chunk.offset..chunk.offset + chunk.data.len()],
            chunk.data
        );
        let tag = &data[chunk.offset + chunk.data.len()..][..8];
        assert_eq!(u32::from_be_bytes(tag[..4].try_into().unwrap()), chunk.id);
        assert_eq!(
            u32::from_be_bytes(tag[4..].try_into().unwrap()) as usize,
            chunk.data.len()
        );
        expected_offset = chunk.offset + chunk.data.len() + 8;
    }
    assert_eq!(expected_offset, data.len());
});

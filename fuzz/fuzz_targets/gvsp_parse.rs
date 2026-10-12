//! Any datagram: `parse_packet_ref` returns a value or an error, never panics,
//! and what it borrows lies inside the datagram.
#![no_main]

use libfuzzer_sys::fuzz_target;
use viva_gige::{GvspContent, parse_packet_ref};

fuzz_target!(|data: &[u8]| {
    let Ok(packet) = parse_packet_ref(data) else {
        return;
    };
    assert!(packet.header.header_len <= data.len());
    match packet.content {
        GvspContent::Payload(bytes) => {
            assert_eq!(bytes.len(), data.len() - packet.header.header_len);
        }
        GvspContent::Trailer(trailer) => {
            assert_eq!(
                trailer.chunk_layout_id.is_some(),
                trailer.chunk_payload_length.is_some()
            );
        }
        _ => {}
    }
});

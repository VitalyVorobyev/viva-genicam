//! Any sequence of datagrams at any packet size: the assembler neither panics
//! nor lets a block outgrow its bound (`viva_genicam::stream::fuzz_assemble`
//! checks the invariants).
//!
//! Input: packet size (2 bytes), a `PayloadSize` selector (1 byte), then
//! datagrams, each prefixed by its length (2 bytes, big-endian).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() < 3 {
        return;
    }
    // At least the IPv4 + UDP + extended GVSP headers plus one byte.
    let packet_size = u32::from(u16::from_be_bytes([data[0], data[1]])).max(49);
    let payload_size = match data[2] {
        0 => None,
        n => Some(usize::from(n) * 64),
    };
    let mut datagrams = Vec::new();
    let mut rest = &data[3..];
    while rest.len() >= 2 {
        let len = usize::from(u16::from_be_bytes([rest[0], rest[1]])).min(rest.len() - 2);
        datagrams.push(&rest[2..2 + len]);
        rest = &rest[2 + len..];
    }
    viva_genicam::stream::fuzz_assemble(packet_size, payload_size, &datagrams);
});

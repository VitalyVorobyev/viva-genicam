//! Entry point for `cargo fuzz` (CI-09). Compiled only under `--cfg fuzzing`,
//! which cargo-fuzz sets; nothing here is part of the library's API.

use std::time::{Duration, Instant};

use super::assembly::{Assembler, AssemblyConfig, ESTIMATE_CAP};
use super::stats::StreamStatsAccumulator;

/// Feed `datagrams` to a fresh assembler, one millisecond apart, then let every
/// timeout fire. Panics if an invariant breaks; returns the blocks that left.
pub fn fuzz_assemble(packet_size: u32, payload_size: Option<usize>, datagrams: &[&[u8]]) -> usize {
    let mut config = AssemblyConfig::new(packet_size);
    config.payload_size = payload_size;
    let limit = ESTIMATE_CAP.max(payload_size.unwrap_or(0));
    let stats = StreamStatsAccumulator::new();
    let mut assembler = Assembler::new(config, stats.clone());
    let t0 = Instant::now();
    let mut out = Vec::new();
    for (i, datagram) in datagrams.iter().enumerate() {
        let now = t0 + Duration::from_millis(i as u64);
        assembler.ingest(datagram, now, &mut out);
        assembler.poll(now, &mut out);
    }
    assembler.poll(t0 + Duration::from_secs(3600), &mut out);
    for block in &out {
        assert!(block.payload.len() <= limit, "a block outgrew its bound");
    }
    let snapshot = stats.snapshot();
    assert!(
        snapshot.frames_unsupported as usize + out.len() <= datagrams.len(),
        "more blocks than datagrams"
    );
    out.len()
}

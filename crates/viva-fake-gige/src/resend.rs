//! Packet resend, as a GigE Vision device answers `PACKETRESEND_CMD`.
//!
//! Written with this crate's **own** constants and offsets (ADR-0019), never
//! the client's encoder: a fake that parsed requests with `viva-gige`'s layout
//! would agree with any mistake in it. The request payload, from the field
//! table Wireshark's `packet-gvcp.c` dissects (`dissect_packetresend_cmd`):
//!
//! | Bytes | Standard ids (12)              | Extended ids (20, flag `0x10`) |
//! |-------|--------------------------------|--------------------------------|
//! | 0–1   | stream channel index           | stream channel index           |
//! | 2–3   | block id (16 bits)             | reserved                       |
//! | 4–7   | first packet id (low 24 bits)  | first packet id (32 bits)      |
//! | 8–11  | last packet id (low 24 bits)   | last packet id (32 bits)       |
//! | 12–19 | —                              | block id (64 bits)             |
//!
//! The command asks for no acknowledgement, and the fake sends none. The
//! packets go out again from the **stream** socket, byte for byte as first
//! built; a range the fake no longer holds is answered with one header-only
//! GVSP packet carrying `PACKET_UNAVAILABLE` (`0x800C`), the block id and the
//! range's first packet id. Whether real cameras answer that way, or stay
//! silent, is open (backlog).

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::net::UdpSocket;

use crate::gvsp::{Datagram, header_only};

/// GVCP `PACKETRESEND_CMD`.
pub const PACKETRESEND_CMD: u16 = 0x0040;
/// Flag byte bit 4 on `PACKETRESEND_CMD`: 64-bit block and 32-bit packet ids.
const EXTENDED_IDS_FLAG: u8 = 0x10;
/// GVSP status of a packet the device can no longer send.
pub const STATUS_PACKET_UNAVAILABLE: u16 = 0x800C;
/// Packets of the last blocks the fake keeps for resending, by default.
pub const DEFAULT_RETENTION: usize = 4;

/// One `PACKETRESEND_CMD`, as the fake reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResendRequest {
    /// Stream channel index.
    pub channel: u16,
    /// Block id: 16 bits widened, or the 64-bit extended id.
    pub block_id: u64,
    /// First packet id asked for.
    pub first: u32,
    /// Last packet id asked for, inclusive.
    pub last: u32,
    /// Whether the request used the extended-id layout.
    pub extended_ids: bool,
}

/// Read a `PACKETRESEND_CMD` payload, or `None` if it is too short for the
/// layout its flag announces.
pub fn parse_request(flags: u8, payload: &[u8]) -> Option<ResendRequest> {
    let u16_at = |at: usize| u16::from_be_bytes([payload[at], payload[at + 1]]);
    let u32_at = |at: usize| {
        u32::from_be_bytes([
            payload[at],
            payload[at + 1],
            payload[at + 2],
            payload[at + 3],
        ])
    };
    if flags & EXTENDED_IDS_FLAG != 0 {
        if payload.len() < 20 {
            return None;
        }
        let mut block = [0u8; 8];
        block.copy_from_slice(&payload[12..20]);
        Some(ResendRequest {
            channel: u16_at(0),
            block_id: u64::from_be_bytes(block),
            first: u32_at(4),
            last: u32_at(8),
            extended_ids: true,
        })
    } else {
        if payload.len() < 12 {
            return None;
        }
        Some(ResendRequest {
            channel: u16_at(0),
            block_id: u64::from(u16_at(2)),
            first: u32_at(4) & 0x00FF_FFFF,
            last: u32_at(8) & 0x00FF_FFFF,
            extended_ids: false,
        })
    }
}

/// What the fake did with resend requests, counted on the device side.
#[derive(Debug, Default)]
pub struct ResendCounters {
    requests: AtomicU64,
    packets_resent: AtomicU64,
    packets_unavailable: AtomicU64,
}

/// A snapshot of [`ResendCounters`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResendStats {
    /// `PACKETRESEND_CMD`s received, served or not.
    pub requests: u64,
    /// Packets sent again.
    pub packets_resent: u64,
    /// Packets asked for that the fake no longer held.
    pub packets_unavailable: u64,
}

impl ResendCounters {
    /// Take a snapshot.
    pub fn snapshot(&self) -> ResendStats {
        ResendStats {
            requests: self.requests.load(Ordering::Relaxed),
            packets_resent: self.packets_resent.load(Ordering::Relaxed),
            packets_unavailable: self.packets_unavailable.load(Ordering::Relaxed),
        }
    }
}

/// The stream socket and where it sends, as the current acquisition set them.
#[derive(Debug, Clone)]
pub(crate) struct StreamPath {
    pub socket: Arc<UdpSocket>,
    pub dest: SocketAddr,
    pub max_on_wire: Option<u32>,
}

/// The packets of the last few blocks, as built, before any scripted damage.
#[derive(Debug)]
pub(crate) struct Retention {
    capacity: usize,
    blocks: VecDeque<(u64, Vec<Vec<u8>>)>,
    path: Option<StreamPath>,
}

/// How a request is answered: packets to send again, and the header-only
/// packet for whatever the fake no longer holds.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Answer {
    pub resend: Vec<Vec<u8>>,
    pub unavailable: Option<(Vec<u8>, u64)>,
}

impl Retention {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            blocks: VecDeque::new(),
            path: None,
        }
    }

    pub fn set_path(&mut self, path: StreamPath) {
        self.path = Some(path);
    }

    pub fn path(&self) -> Option<StreamPath> {
        self.path.clone()
    }

    /// Keep a block's datagrams, indexed by packet id, evicting the oldest.
    pub fn retain(&mut self, block_id: u64, datagrams: &[Datagram]) {
        if self.capacity == 0 {
            return;
        }
        let mut by_id: Vec<Vec<u8>> = Vec::new();
        for d in datagrams {
            let at = d.packet_id as usize;
            if by_id.len() <= at {
                by_id.resize(at + 1, Vec::new());
            }
            by_id[at] = d.bytes.clone();
        }
        self.blocks.retain(|(id, _)| *id != block_id);
        while self.blocks.len() >= self.capacity {
            self.blocks.pop_front();
        }
        self.blocks.push_back((block_id, by_id));
    }

    /// Answer `request`: the packets still held, and one unavailable packet
    /// for the first id of the part that is not.
    pub fn answer(&self, request: &ResendRequest) -> Answer {
        let mut answer = Answer::default();
        if request.first > request.last {
            return answer;
        }
        let held = self
            .blocks
            .iter()
            .find(|(id, _)| *id == request.block_id)
            .map(|(_, packets)| packets);
        let mut first_missing = None;
        let mut missing = 0u64;
        for id in request.first..=request.last {
            match held
                .and_then(|p| p.get(id as usize))
                .filter(|b| !b.is_empty())
            {
                Some(bytes) => answer.resend.push(bytes.clone()),
                None => {
                    first_missing.get_or_insert(id);
                    missing += 1;
                }
            }
        }
        if let Some(first) = first_missing {
            answer.unavailable = Some((
                header_only(
                    request.block_id,
                    request.extended_ids,
                    first,
                    STATUS_PACKET_UNAVAILABLE,
                ),
                missing,
            ));
        }
        answer
    }
}

/// Serve one request: count it, and if the device supports resend, send what
/// it still holds from the stream socket.
pub(crate) async fn serve(
    supported: bool,
    retention: &std::sync::Mutex<Retention>,
    counters: &ResendCounters,
    flags: u8,
    payload: &[u8],
) {
    counters.requests.fetch_add(1, Ordering::Relaxed);
    if !supported {
        tracing::debug!("PACKETRESEND_CMD ignored: the device does not support resend");
        return;
    }
    let Some(request) = parse_request(flags, payload) else {
        tracing::debug!(len = payload.len(), "malformed PACKETRESEND_CMD");
        return;
    };
    let (answer, path) = match retention.lock() {
        Ok(retention) => (retention.answer(&request), retention.path()),
        Err(_) => return,
    };
    let Some(path) = path else {
        return;
    };
    tracing::debug!(?request, resent = answer.resend.len(), "PACKETRESEND_CMD");
    for packet in &answer.resend {
        let on_wire = packet.len() + 28;
        if path.max_on_wire.is_some_and(|max| on_wire > max as usize) {
            continue;
        }
        if path.socket.send_to(packet, path.dest).await.is_ok() {
            counters.packets_resent.fetch_add(1, Ordering::Relaxed);
        }
    }
    if let Some((packet, count)) = answer.unavailable {
        counters
            .packets_unavailable
            .fetch_add(count, Ordering::Relaxed);
        let _ = path.socket.send_to(&packet, path.dest).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gvsp::{BlockSpec, block_datagrams};

    /// The standard request, written out from the field table.
    #[test]
    fn a_standard_request_is_read_at_its_own_offsets() {
        #[rustfmt::skip]
        let payload = [
            0x00, 0x02,             // channel 2
            0x12, 0x34,             // block 0x1234
            0xFF, 0x00, 0x00, 0x05, // first 5 (the top byte is not part of the id)
            0x00, 0x00, 0x00, 0x09, // last 9
        ];
        assert_eq!(
            parse_request(0x00, &payload),
            Some(ResendRequest {
                channel: 2,
                block_id: 0x1234,
                first: 5,
                last: 9,
                extended_ids: false,
            })
        );
        assert_eq!(parse_request(0x00, &payload[..11]), None);
    }

    #[test]
    fn an_extended_request_is_read_at_its_own_offsets() {
        #[rustfmt::skip]
        let payload = [
            0x00, 0x00,             // channel 0
            0x00, 0x00,             // reserved
            0x01, 0x00, 0x00, 0x05, // first 0x0100_0005: all 32 bits
            0x01, 0x00, 0x00, 0x07, // last
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x02, // block 2^32 + 2
        ];
        assert_eq!(
            parse_request(0x10, &payload),
            Some(ResendRequest {
                channel: 0,
                block_id: (1 << 32) + 2,
                first: 0x0100_0005,
                last: 0x0100_0007,
                extended_ids: true,
            })
        );
        assert_eq!(parse_request(0x10, &payload[..19]), None);
    }

    fn block(id: u64) -> Vec<Datagram> {
        block_datagrams(
            &BlockSpec::image(id, 10, 1, 0x0108_0001, 28 + 8 + 4),
            &[7u8; 10],
        )
    }

    fn request(block_id: u64, first: u32, last: u32) -> ResendRequest {
        ResendRequest {
            channel: 0,
            block_id,
            first,
            last,
            extended_ids: false,
        }
    }

    #[test]
    fn a_retained_range_is_sent_again_as_built() {
        let mut retention = Retention::new(2);
        let datagrams = block(1);
        retention.retain(1, &datagrams);
        let answer = retention.answer(&request(1, 1, 2));
        assert_eq!(
            answer.resend,
            vec![datagrams[1].bytes.clone(), datagrams[2].bytes.clone()]
        );
        assert!(answer.unavailable.is_none());
    }

    #[test]
    fn a_block_beyond_retention_is_answered_unavailable() {
        let mut retention = Retention::new(2);
        for id in 1..=3 {
            retention.retain(id, &block(id));
        }
        let answer = retention.answer(&request(1, 2, 3));
        assert!(answer.resend.is_empty());
        let (packet, count) = answer.unavailable.expect("unavailable");
        assert_eq!(count, 2);
        assert_eq!(packet, vec![0x80, 0x0C, 0x00, 0x01, 0x03, 0x00, 0x00, 0x02]);
        // Block 3 is still held.
        assert_eq!(retention.answer(&request(3, 1, 1)).resend.len(), 1);
    }
}

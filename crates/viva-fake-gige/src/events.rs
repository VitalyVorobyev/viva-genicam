//! The message channel: events sent one message at a time, each until it is
//! acknowledged.
//!
//! Modelled on what an Allied Vision Manta G-125B (firmware 00.01.44.28514)
//! did on the wire, with this fake's own numbers:
//!
//! - every message asks for an acknowledgement;
//! - one message is outstanding at a time; unacknowledged, it is sent again,
//!   same request id and bytes, every `GevMCTT` (`0x0B14`) until
//!   `GevMCRC` (`0x0B18`) retries are spent, and then abandoned;
//! - events that occur meanwhile wait in a bounded queue and leave packed
//!   into the next `EVENT_CMD` once the outstanding one is acknowledged (the
//!   Manta packed 11, and at most 32, into one); an `EVENTDATA_CMD` leaves on
//!   its own;
//! - an event that finds the queue full is lost, and counted.
//!
//! The Manta's own numbers were `GevMCTT` 100 ms, `GevMCRC` `0xFFFFFFFF` and
//! about 44 events held; it also sends an `EventOverflow` for the events it
//! loses, which this fake does not.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use bytes::{BufMut, BytesMut};
use tokio::net::UdpSocket;
use tokio::sync::{Mutex, mpsc};
use tokio::time::Instant;
use tracing::{debug, trace};

use crate::registers::RegisterMap;

// GVCP vocabulary, imported (ADR-0030).
use viva_gige::consts::{
    EVENT_ACK, EVENT_COMMAND as EVENT_CMD, EVENTDATA_ACK, EVENTDATA_COMMAND as EVENTDATA_CMD,
    GVCP_CMD_KEY,
};

/// Flags byte: acknowledgement required, 16-bit block ids.
const ACK_REQUIRED: u8 = 0x01;
/// Size of one 16-bit-block-id event entry.
const ENTRY_LEN: usize = 16;
/// Events this fake holds while a message waits for its acknowledgement.
pub(crate) const QUEUE_CAPACITY: usize = 16;
/// Events this fake packs into one `EVENT_CMD`.
pub(crate) const EVENTS_PER_MESSAGE: usize = 8;

/// One event as the camera produced it.
#[derive(Debug, Clone)]
pub(crate) struct Occurred {
    pub dest: SocketAddr,
    pub event_id: u16,
    pub block_id: u16,
    pub timestamp: u64,
    /// Device data: sent as an `EVENTDATA_CMD` when present.
    pub data: Option<Vec<u8>>,
}

/// What the message channel did, counted as the device sees it.
#[derive(Debug, Default)]
pub(crate) struct Counters {
    messages: AtomicU64,
    transmissions: AtomicU64,
    acknowledged: AtomicU64,
    abandoned: AtomicU64,
    lost: AtomicU64,
    events: AtomicU64,
}

/// What a fake camera's message channel did: [`FakeCamera::message_channel`](crate::FakeCamera::message_channel).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct MessageChannelStats {
    /// Distinct messages sent.
    pub messages: u64,
    /// Datagrams sent: every message once, plus each retransmission.
    pub transmissions: u64,
    /// Messages acknowledged.
    pub acknowledged: u64,
    /// Messages given up on after `GevMCRC` retries.
    pub abandoned: u64,
    /// Events that found the queue full.
    pub lost: u64,
    /// Events sent, each counted once whatever its message's retransmissions.
    pub events: u64,
}

impl Counters {
    pub(crate) fn snapshot(&self) -> MessageChannelStats {
        MessageChannelStats {
            messages: self.messages.load(Ordering::Relaxed),
            transmissions: self.transmissions.load(Ordering::Relaxed),
            acknowledged: self.acknowledged.load(Ordering::Relaxed),
            abandoned: self.abandoned.load(Ordering::Relaxed),
            lost: self.lost.load(Ordering::Relaxed),
            events: self.events.load(Ordering::Relaxed),
        }
    }
}

struct Outstanding {
    request_id: u16,
    ack_opcode: u16,
    bytes: Vec<u8>,
    dest: SocketAddr,
    sent_at: Instant,
    retries: u32,
}

/// Run the message channel until every sender is gone.
pub(crate) async fn run(
    socket: UdpSocket,
    mut occurred: mpsc::UnboundedReceiver<Occurred>,
    regs: Arc<Mutex<RegisterMap>>,
    counters: Arc<Counters>,
) {
    let mut queue: VecDeque<Occurred> = VecDeque::new();
    let mut outstanding: Option<Outstanding> = None;
    let mut request_id: u16 = 0;
    let mut buf = [0u8; 64];
    loop {
        let (timeout, retry_count) = {
            let regs = regs.lock().await;
            (
                Duration::from_millis(u64::from(regs.message_timeout_ms())),
                regs.message_retry_count(),
            )
        };
        let due = outstanding.as_ref().map(|o| o.sent_at + timeout);
        tokio::select! {
            event = occurred.recv() => match event {
                None => return,
                Some(event) if queue.len() >= QUEUE_CAPACITY => {
                    counters.lost.fetch_add(1, Ordering::Relaxed);
                    debug!(event_id = event.event_id, "event queue full: event lost");
                }
                Some(event) => queue.push_back(event),
            },
            received = socket.recv_from(&mut buf) => {
                let Ok((len, from)) = received else { continue };
                if let Some(waiting) = &outstanding
                    && from == waiting.dest
                    && is_ack(&buf[..len], waiting.ack_opcode, waiting.request_id)
                {
                    counters.acknowledged.fetch_add(1, Ordering::Relaxed);
                    trace!(request_id = waiting.request_id, "event acknowledged");
                    outstanding = None;
                }
            }
            () = sleep_until(due), if due.is_some() => {
                if let Some(waiting) = outstanding.as_mut() {
                    if waiting.retries >= retry_count {
                        counters.abandoned.fetch_add(1, Ordering::Relaxed);
                        debug!(request_id = waiting.request_id, "event message abandoned");
                        outstanding = None;
                    } else {
                        waiting.retries += 1;
                        waiting.sent_at = Instant::now();
                        let _ = socket.send_to(&waiting.bytes, waiting.dest).await;
                        counters.transmissions.fetch_add(1, Ordering::Relaxed);
                        trace!(request_id = waiting.request_id, retry = waiting.retries, "event resent");
                    }
                }
            }
        }
        if outstanding.is_none()
            && let Some(message) = next_message(&mut queue, &mut request_id)
        {
            let _ = socket.send_to(&message.bytes, message.dest).await;
            counters.messages.fetch_add(1, Ordering::Relaxed);
            counters.transmissions.fetch_add(1, Ordering::Relaxed);
            counters
                .events
                .fetch_add(message.events as u64, Ordering::Relaxed);
            outstanding = Some(Outstanding {
                request_id,
                ack_opcode: message.ack_opcode,
                bytes: message.bytes,
                dest: message.dest,
                sent_at: Instant::now(),
                retries: 0,
            });
        }
    }
}

async fn sleep_until(due: Option<Instant>) {
    match due {
        Some(due) => tokio::time::sleep_until(due).await,
        None => std::future::pending().await,
    }
}

struct Message {
    bytes: Vec<u8>,
    dest: SocketAddr,
    ack_opcode: u16,
    events: usize,
}

/// The next message from the front of the queue: an `EVENTDATA_CMD` alone,
/// or up to [`EVENTS_PER_MESSAGE`] bare events for one destination.
fn next_message(queue: &mut VecDeque<Occurred>, request_id: &mut u16) -> Option<Message> {
    let first = queue.pop_front()?;
    *request_id = request_id.wrapping_add(1).max(1);
    let dest = first.dest;
    if let Some(data) = &first.data {
        let mut buf = BytesMut::with_capacity(8 + ENTRY_LEN + data.len());
        put_header(&mut buf, EVENTDATA_CMD, ENTRY_LEN + data.len(), *request_id);
        put_entry(&mut buf, &first);
        buf.extend_from_slice(data);
        return Some(Message {
            bytes: buf.to_vec(),
            dest,
            ack_opcode: EVENTDATA_ACK,
            events: 1,
        });
    }
    let mut entries = vec![first];
    while entries.len() < EVENTS_PER_MESSAGE
        && queue
            .front()
            .is_some_and(|next| next.data.is_none() && next.dest == dest)
    {
        entries.extend(queue.pop_front());
    }
    let mut buf = BytesMut::with_capacity(8 + ENTRY_LEN * entries.len());
    put_header(&mut buf, EVENT_CMD, ENTRY_LEN * entries.len(), *request_id);
    for entry in &entries {
        put_entry(&mut buf, entry);
    }
    Some(Message {
        bytes: buf.to_vec(),
        dest,
        ack_opcode: EVENT_ACK,
        events: entries.len(),
    })
}

/// GVCP command header, written out from the GigE Vision field table: key,
/// flags, opcode, length, request id.
fn put_header(buf: &mut BytesMut, opcode: u16, length: usize, request_id: u16) {
    buf.put_u8(GVCP_CMD_KEY);
    buf.put_u8(ACK_REQUIRED);
    buf.put_u16(opcode);
    buf.put_u16(length as u16);
    buf.put_u16(request_id);
}

/// One 16-byte entry: reserved, event id, stream channel, block id,
/// timestamp.
fn put_entry(buf: &mut BytesMut, event: &Occurred) {
    buf.put_u16(0);
    buf.put_u16(event.event_id);
    buf.put_u16(0);
    buf.put_u16(event.block_id);
    buf.put_u64(event.timestamp);
}

/// A successful acknowledgement of `request_id` under `opcode`.
fn is_ack(datagram: &[u8], opcode: u16, request_id: u16) -> bool {
    datagram.len() >= 8
        && datagram[0..2] == [0, 0]
        && u16::from_be_bytes([datagram[2], datagram[3]]) == opcode
        && u16::from_be_bytes([datagram[6], datagram[7]]) == request_id
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(block_id: u16, data: Option<Vec<u8>>) -> Occurred {
        Occurred {
            dest: SocketAddr::from(([127, 0, 0, 1], 10_000)),
            event_id: 0x9001,
            block_id,
            timestamp: u64::from(block_id),
            data,
        }
    }

    #[test]
    fn bare_events_pack_and_data_events_go_alone() {
        let mut queue: VecDeque<_> = (1..=10).map(|b| event(b, None)).collect();
        queue.insert(9, event(99, Some(vec![1, 2])));
        let mut id = 0;
        let first = next_message(&mut queue, &mut id).expect("message");
        assert_eq!((first.events, first.bytes.len()), (8, 8 + 8 * 16));
        assert_eq!(first.bytes[..4], [0x42, 0x01, 0x00, 0xC0]);
        let second = next_message(&mut queue, &mut id).expect("message");
        assert_eq!(second.events, 1, "the bare event before the data one");
        let third = next_message(&mut queue, &mut id).expect("message");
        assert_eq!(third.bytes[2..4], [0x00, 0xC2]);
        assert_eq!(third.ack_opcode, EVENTDATA_ACK);
        assert_eq!(id, 3);
    }
}

//! GVCP message/event channel handling.
//!
//! A device delivers events to the controller as GVCP **commands** on the
//! message channel — `EVENT_CMD` (0x00C0) for bare notifications and
//! `EVENTDATA_CMD` (0x00C2) when the event carries device-specific data. Both
//! are commands, so a datagram begins with the 0x42 key byte and a flags byte,
//! not with a status code, and both may ask the controller to acknowledge.
//!
//! One `EVENT_CMD` can pack several events: the payload is an array of
//! fixed-size entries, 16 bytes each with 16-bit block IDs or 24 bytes each
//! when the device sets the extended-ID flag (GigE Vision 2.0). Layout per
//! entry, from the GigE Vision field table and corroborated by Wireshark's
//! `dissect_event_cmd`:
//!
//! ```text
//! 16-bit block IDs (16 bytes)      64-bit block IDs (24 bytes)
//!  0  reserved            u16       0  reserved            u16
//!  2  event identifier    u16       2  event identifier    u16
//!  4  stream channel      u16       4  stream channel      u16
//!  6  block id            u16       6  reserved            u16
//!  8  timestamp           u64       8  block id            u64
//!                                  16  timestamp           u64
//! ```

use std::collections::{HashMap, VecDeque};
use std::io;
use std::io::ErrorKind;
use std::net::{IpAddr, SocketAddr, UdpSocket as StdUdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use bytes::Bytes;
#[cfg(test)]
use bytes::{BufMut, BytesMut};
use socket2::{Domain, Protocol, Socket, Type};
use tokio::sync::Notify;
use tracing::{debug, info, trace, warn};

use crate::consts as gvcp;

/// Constants related to GVCP message packets. Events arrive as commands from
/// the device, not as acknowledgements — which is why the first two bytes are
/// the key and the flags, and not a status code.
mod consts {
    pub(super) use crate::consts::{FLAG_ACK_REQUIRED, FLAG_EXTENDED_IDS, GVCP_CMD_KEY};
    /// Size of the GVCP message header in bytes.
    pub(super) const GVCP_HEADER: usize = crate::consts::GVCP_HEADER_SIZE;
    /// Default receive buffer size requested for the UDP socket (bytes).
    pub(super) const DEFAULT_RCVBUF: usize = 1 << 20; // 1 MiB.
    /// Maximum datagram size accepted on the event channel (bytes).
    pub(super) const MAX_EVENT_SIZE: usize = 2048;
}

/// Header of a datagram received on the GVCP message channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MessageHeader {
    command: u16,
    length: usize,
    request_id: u16,
    ack_required: bool,
    extended_ids: bool,
}

impl MessageHeader {
    fn parse(data: &[u8]) -> io::Result<Self> {
        if data.len() < consts::GVCP_HEADER {
            return Err(io::Error::new(ErrorKind::InvalidData, "packet too short"));
        }
        if data.len() > consts::MAX_EVENT_SIZE {
            return Err(io::Error::new(ErrorKind::InvalidData, "packet too large"));
        }
        if data[0] != consts::GVCP_CMD_KEY {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "not a GVCP command packet",
            ));
        }
        let flags = data[1];
        let command = u16::from_be_bytes([data[2], data[3]]);
        let length = u16::from_be_bytes([data[4], data[5]]) as usize;
        let request_id = u16::from_be_bytes([data[6], data[7]]);

        if !matches!(command, gvcp::EVENT_COMMAND | gvcp::EVENTDATA_COMMAND) {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "unexpected opcode for event packet",
            ));
        }
        if length + consts::GVCP_HEADER != data.len() {
            return Err(io::Error::new(ErrorKind::InvalidData, "length mismatch"));
        }

        Ok(Self {
            command,
            length,
            request_id,
            ack_required: flags & consts::FLAG_ACK_REQUIRED != 0,
            extended_ids: flags & consts::FLAG_EXTENDED_IDS != 0,
        })
    }

    /// Opcode of the acknowledgement this command expects.
    fn ack_opcode(&self) -> u16 {
        match self.command {
            gvcp::EVENTDATA_COMMAND => gvcp::EVENTDATA_ACK,
            _ => gvcp::EVENT_ACK,
        }
    }

    /// Size of one event entry given the extended-ID flag.
    fn entry_size(&self) -> usize {
        if self.extended_ids {
            gvcp::EVENT_ENTRY_EXTENDED
        } else {
            gvcp::EVENT_ENTRY
        }
    }
}

/// Parsed representation of a single GVCP event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventPacket {
    /// Source address of the datagram.
    pub src: SocketAddr,
    /// Event identifier reported by the device.
    pub event_id: u16,
    /// Device timestamp carried by the event (ticks).
    pub timestamp_dev: u64,
    /// Stream channel associated with the event.
    pub stream_channel: u16,
    /// GVSP block identifier associated with the event.
    ///
    /// 16-bit on GigE Vision 1.x devices, widened here so a 2.0 device using
    /// extended block IDs fits without a second type.
    pub block_id: u64,
    /// Event data following the entry, empty for a bare `EVENT_CMD`.
    pub payload: Bytes,
    /// The whole event as the device sent it — the 16- or 24-byte entry, then
    /// any data — which is what a GenApi event port addresses: every corpus
    /// document that names an event's timestamp puts it at offset 0x8 of
    /// this, where the 16-bit-block-id entry has it (ADR-0026).
    pub raw: Bytes,
}

impl EventPacket {
    /// Decode one event entry. `entry` must be at least `header.entry_size()`;
    /// `raw` is the entry with any data after it.
    fn parse_entry(
        src: SocketAddr,
        header: &MessageHeader,
        entry: &[u8],
        payload: Bytes,
        raw: Bytes,
    ) -> Self {
        let event_id = u16::from_be_bytes([entry[2], entry[3]]);
        let stream_channel = u16::from_be_bytes([entry[4], entry[5]]);
        let (block_id, ts_at) = if header.extended_ids {
            // entry[6..8] is reserved in the extended layout.
            let id = u64::from_be_bytes([
                entry[8], entry[9], entry[10], entry[11], entry[12], entry[13], entry[14],
                entry[15],
            ]);
            (id, 16)
        } else {
            (u64::from(u16::from_be_bytes([entry[6], entry[7]])), 8)
        };
        let timestamp_dev = u64::from_be_bytes([
            entry[ts_at],
            entry[ts_at + 1],
            entry[ts_at + 2],
            entry[ts_at + 3],
            entry[ts_at + 4],
            entry[ts_at + 5],
            entry[ts_at + 6],
            entry[ts_at + 7],
        ]);
        Self {
            src,
            event_id,
            timestamp_dev,
            stream_channel,
            block_id,
            payload,
            raw,
        }
    }

    /// Decode every event carried by one message-channel datagram.
    fn parse_datagram(src: SocketAddr, data: &[u8]) -> io::Result<(MessageHeader, Vec<Self>)> {
        let header = MessageHeader::parse(data)?;
        let entry_size = header.entry_size();
        let body = &data[consts::GVCP_HEADER..];

        if body.len() < entry_size {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "event payload shorter than one entry",
            ));
        }

        let events = if header.command == gvcp::EVENTDATA_COMMAND {
            // One entry, then device-specific data to the end of the datagram.
            // Packing several variable-length events into one EVENTDATA_CMD
            // needs the GEV 2.1 per-event size field, which we do not read;
            // no corpus device has been observed doing it.
            let raw = Bytes::copy_from_slice(body);
            let payload = raw.slice(entry_size..);
            vec![Self::parse_entry(src, &header, body, payload, raw)]
        } else {
            if header.length % entry_size != 0 {
                return Err(io::Error::new(
                    ErrorKind::InvalidData,
                    "event payload is not a whole number of entries",
                ));
            }
            body.chunks_exact(entry_size)
                .map(|entry| {
                    let raw = Bytes::copy_from_slice(entry);
                    Self::parse_entry(src, &header, entry, Bytes::new(), raw)
                })
                .collect()
        };

        Ok((header, events))
    }
}

/// Build the acknowledgement for a message-channel command.
///
/// GVCP acknowledgements carry no payload here: status, opcode, a zero length
/// and the request id the device used.
fn encode_ack(ack_opcode: u16, request_id: u16) -> [u8; consts::GVCP_HEADER] {
    let mut buf = [0u8; consts::GVCP_HEADER];
    buf[0..2].copy_from_slice(&viva_gencp::StatusCode::Success.to_raw().to_be_bytes());
    buf[2..4].copy_from_slice(&ack_opcode.to_be_bytes());
    buf[4..6].copy_from_slice(&0u16.to_be_bytes());
    buf[6..8].copy_from_slice(&request_id.to_be_bytes());
    buf
}

/// Events kept for the application before the oldest is dropped.
pub const EVENT_QUEUE_CAPACITY: usize = 1024;

/// How long the receive thread waits for a datagram before checking whether
/// to stop.
const RECEIVE_TICK: Duration = Duration::from_millis(20);

/// What a GVCP message channel has received, counted by its receive thread.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct EventStats {
    /// Datagrams that parsed as `EVENT_CMD` or `EVENTDATA_CMD`, copies
    /// included.
    pub messages: u64,
    /// Messages that asked for an acknowledgement and were sent one.
    pub acknowledged: u64,
    /// Copies of the message before them, the device's retransmissions:
    /// acknowledged again, their events not queued twice.
    pub retransmissions: u64,
    /// Events queued for the application.
    pub events: u64,
    /// Events dropped, oldest first, because the application left
    /// [`EVENT_QUEUE_CAPACITY`] of them unread.
    pub dropped: u64,
    /// Datagrams that were not an event message.
    pub malformed: u64,
}

#[derive(Default)]
struct Counters {
    messages: AtomicU64,
    acknowledged: AtomicU64,
    retransmissions: AtomicU64,
    events: AtomicU64,
    dropped: AtomicU64,
    malformed: AtomicU64,
}

struct QueueState {
    events: VecDeque<EventPacket>,
    /// The receive thread's failure, reported once after the queue drains.
    failed: Option<io::Error>,
    /// The receive thread stopped.
    ended: bool,
}

/// State a socket shares with its receive thread.
struct Shared {
    queue: std::sync::Mutex<QueueState>,
    notify: Notify,
    stop: AtomicBool,
    counters: Counters,
    capacity: usize,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, QueueState> {
        // A thread that panicked holding the lock left a queue of values,
        // still coherent.
        self.queue.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// A GVCP message channel: the socket a device sends its events to.
///
/// Binding starts a receive thread. It **acknowledges each message as it
/// arrives**, whether or not the application is reading, and queues its
/// events for [`recv`](Self::recv), up to [`EVENT_QUEUE_CAPACITY`]; past
/// that the oldest is dropped, counted in [`EventStats::dropped`] and
/// logged. A device whose acknowledgement does not come resends the message
/// every `GevMCTT` and holds back the events behind it: an Allied Vision
/// Manta G-125B (`GevMCTT` 100 ms) sent one `ExposureEnd` twelve times to an
/// application that read a second late when the acknowledgement waited for
/// the read, and, left unread for ten seconds, kept 44 events and sent
/// `EventOverflow` for the rest.
///
/// A retransmission repeats the request id and the bytes of the message
/// before it; it is acknowledged again and its events are not queued twice.
/// Distinct events under one request id are each queued. Dropping the
/// socket stops the thread within one tick.
pub struct EventSocket {
    shared: Arc<Shared>,
    local: SocketAddr,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl EventSocket {
    /// Bind a GVCP message socket on the provided local address and start
    /// its receive thread.
    pub async fn bind(local_ip: IpAddr, port: u16) -> io::Result<Self> {
        Self::bind_with_capacity(local_ip, port, EVENT_QUEUE_CAPACITY)
    }

    fn bind_with_capacity(local_ip: IpAddr, port: u16, capacity: usize) -> io::Result<Self> {
        let domain = match local_ip {
            IpAddr::V4(_) => Domain::IPV4,
            IpAddr::V6(_) => Domain::IPV6,
        };
        let socket = Socket::new(domain, Type::DGRAM, Some(Protocol::UDP))?;
        socket.set_reuse_address(true)?;
        socket.set_nonblocking(true)?;
        if let Err(err) = socket.set_recv_buffer_size(consts::DEFAULT_RCVBUF) {
            warn!(?err, "failed to grow GVCP message buffer");
        }
        let addr = SocketAddr::new(local_ip, port);
        socket.bind(&addr.into())?;
        let socket: StdUdpSocket = socket.into();
        let local = socket.local_addr()?;
        info!(local = %local, "bound GVCP message socket");
        let shared = Arc::new(Shared {
            queue: std::sync::Mutex::new(QueueState {
                events: VecDeque::new(),
                failed: None,
                ended: false,
            }),
            notify: Notify::new(),
            stop: AtomicBool::new(false),
            counters: Counters::default(),
            capacity: capacity.max(1),
        });
        let thread = {
            let shared = Arc::clone(&shared);
            std::thread::Builder::new()
                .name("viva-gige-events".into())
                .spawn(move || receive_loop(&socket, &shared))?
        };
        Ok(Self {
            shared,
            local,
            thread: Some(thread),
        })
    }

    /// The next event, waiting until one arrives. Events come in the order
    /// the device sent them; a datagram carrying several is queued whole.
    ///
    /// Cancel-safe: an event is taken only by the poll that returns it.
    /// Concurrent callers are symmetric consumers of one queue. An error
    /// once the receive thread has failed, after the events it queued.
    pub async fn recv(&self) -> io::Result<EventPacket> {
        loop {
            let notified = self.shared.notify.notified();
            tokio::pin!(notified);
            // Register before checking, so an event queued in between still
            // wakes us.
            notified.as_mut().enable();
            if let Some(result) = self.try_take() {
                return result;
            }
            notified.await;
        }
    }

    /// The next event if one is queued: `None` when none is, an error once
    /// the receive thread has failed or stopped.
    fn try_take(&self) -> Option<io::Result<EventPacket>> {
        let mut state = self.shared.lock();
        if let Some(event) = state.events.pop_front() {
            return Some(Ok(event));
        }
        if let Some(err) = state.failed.take() {
            return Some(Err(err));
        }
        state.ended.then(|| {
            Err(io::Error::new(
                ErrorKind::BrokenPipe,
                "the GVCP message channel's receive thread has stopped",
            ))
        })
    }

    /// What the receive thread has counted so far.
    pub fn stats(&self) -> EventStats {
        let c = &self.shared.counters;
        EventStats {
            messages: c.messages.load(Ordering::Relaxed),
            acknowledged: c.acknowledged.load(Ordering::Relaxed),
            retransmissions: c.retransmissions.load(Ordering::Relaxed),
            events: c.events.load(Ordering::Relaxed),
            dropped: c.dropped.load(Ordering::Relaxed),
            malformed: c.malformed.load(Ordering::Relaxed),
        }
    }

    /// Return the local address bound to the socket.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.local)
    }
}

impl Drop for EventSocket {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The receive thread: read, acknowledge, queue, until told to stop or the
/// socket fails.
fn receive_loop(socket: &StdUdpSocket, shared: &Shared) {
    let mut buffer = vec![0u8; consts::MAX_EVENT_SIZE];
    let mut last_message: HashMap<SocketAddr, (u16, Vec<u8>)> = HashMap::new();
    let mut warned_overflow = false;
    let failure = loop {
        if shared.stop.load(Ordering::Acquire) {
            break None;
        }
        match crate::nic::wait_readable(socket, RECEIVE_TICK) {
            Ok(false) => continue,
            Ok(true) => {}
            Err(err) => break Some(err),
        }
        // Drain everything the socket holds before waiting again.
        let failed = loop {
            match socket.recv_from(&mut buffer) {
                Ok((len, src)) => handle_datagram(
                    socket,
                    shared,
                    &buffer[..len],
                    src,
                    &mut last_message,
                    &mut warned_overflow,
                ),
                Err(err) if err.kind() == ErrorKind::WouldBlock => break None,
                Err(err) if err.kind() == ErrorKind::Interrupted => {}
                // An ICMP reply to an acknowledgement, reported on the next
                // read: the socket is still good (SR-22).
                Err(err)
                    if matches!(
                        err.kind(),
                        ErrorKind::ConnectionReset | ErrorKind::ConnectionRefused
                    ) =>
                {
                    debug!(error = %err, "ignoring a connection reset on the GVCP message socket");
                }
                Err(err) => break Some(err),
            }
        };
        if failed.is_some() {
            break failed;
        }
    };
    let mut state = shared.lock();
    if let Some(err) = failure {
        warn!(error = %err, "GVCP message socket receive failed; no more events");
        state.failed = Some(err);
    }
    state.ended = true;
    drop(state);
    shared.notify.notify_waiters();
}

/// One datagram: acknowledge it if asked, then queue its events unless it
/// repeats the message before it.
fn handle_datagram(
    socket: &StdUdpSocket,
    shared: &Shared,
    data: &[u8],
    src: SocketAddr,
    last_message: &mut HashMap<SocketAddr, (u16, Vec<u8>)>,
    warned_overflow: &mut bool,
) {
    trace!(bytes = data.len(), %src, "received GVCP message");
    let counters = &shared.counters;
    let (header, events) = match EventPacket::parse_datagram(src, data) {
        Ok(parsed) => parsed,
        Err(err) => {
            counters.malformed.fetch_add(1, Ordering::Relaxed);
            warn!(%src, error = %err, "discarding malformed event packet");
            return;
        }
    };
    counters.messages.fetch_add(1, Ordering::Relaxed);
    if header.ack_required {
        let ack = encode_ack(header.ack_opcode(), header.request_id);
        match socket.send_to(&ack, src) {
            Ok(_) => {
                counters.acknowledged.fetch_add(1, Ordering::Relaxed);
                trace!(%src, request_id = header.request_id, "acknowledged event");
            }
            Err(err) => warn!(%src, error = %err, "failed to acknowledge event"),
        }
    }
    // A retransmission repeats both the request id and the events: a device
    // that reuses one request id for distinct events still has them
    // delivered, since their timestamps differ.
    let message = (header.request_id, data[consts::GVCP_HEADER..].to_vec());
    if last_message.insert(src, message.clone()).as_ref() == Some(&message) {
        counters.retransmissions.fetch_add(1, Ordering::Relaxed);
        debug!(%src, request_id = header.request_id, "dropping a retransmitted event");
        return;
    }
    if events.is_empty() {
        return;
    }
    let count = events.len() as u64;
    let dropped = {
        let mut state = shared.lock();
        state.events.extend(events);
        let excess = state.events.len().saturating_sub(shared.capacity);
        state.events.drain(..excess);
        excess as u64
    };
    counters.events.fetch_add(count, Ordering::Relaxed);
    debug!(events = count, %src, "queueing GVCP events");
    if dropped > 0 {
        let total = counters.dropped.fetch_add(dropped, Ordering::Relaxed) + dropped;
        if !*warned_overflow {
            *warned_overflow = true;
            warn!(
                capacity = shared.capacity,
                "events arrive faster than the application reads them; dropping the oldest \
                 (counted in EventStats::dropped)"
            );
        } else {
            debug!(dropped = total, "dropped unread events");
        }
    }
    shared.notify.notify_waiters();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use tokio::net::UdpSocket;

    fn src() -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 3956)
    }

    /// One `EVENT_CMD` with a single 16-byte entry, written out from the field
    /// table rather than produced by our own encoder (ADR-0019).
    ///
    /// Every field in this fixture used to land somewhere else: the parser read
    /// the event id out of the reserved word, the timestamp out of the stream
    /// channel and block id, and rejected the packet outright because it
    /// expected opcode 0x000D — which is not a GVCP opcode at all.
    #[rustfmt::skip]
    const EVENT_CMD_GOLDEN: [u8; 24] = [
        0x42,                   // command key
        0x01,                   // flags: acknowledge required
        0x00, 0xC0,             // EVENT_CMD
        0x00, 0x10,             // length: one 16-byte entry
        0xCA, 0xFE,             // request id
        0x00, 0x00,             // reserved
        0x12, 0x34,             // event identifier
        0x00, 0x07,             // stream channel index
        0x00, 0x08,             // block id (16-bit)
        0x00, 0x02, 0x00, 0x03, // timestamp, big-endian u64
        0x00, 0x04, 0x00, 0x05,
    ];

    #[test]
    fn event_cmd_matches_spec_offsets() {
        let (header, events) =
            EventPacket::parse_datagram(src(), &EVENT_CMD_GOLDEN).expect("parse");
        assert_eq!(header.command, gvcp::EVENT_COMMAND);
        assert!(header.ack_required);
        assert!(!header.extended_ids);
        assert_eq!(events.len(), 1);
        let ev = &events[0];
        assert_eq!(ev.event_id, 0x1234);
        assert_eq!(ev.stream_channel, 7);
        assert_eq!(ev.block_id, 8);
        assert_eq!(ev.timestamp_dev, 0x0002_0003_0004_0005);
        assert!(ev.payload.is_empty());
        assert_eq!(
            &ev.raw[..],
            &EVENT_CMD_GOLDEN[8..],
            "the entry, header included"
        );
    }

    /// The opcode the parser used to demand. 0x000D is in the GenCP register
    /// range, not the GVCP event range, so no device ever sent it.
    #[test]
    fn event_opcodes_are_in_the_gvcp_event_range() {
        assert_eq!(gvcp::EVENT_COMMAND, 0x00C0);
        assert_eq!(gvcp::EVENT_ACK, 0x00C1);
        assert_eq!(gvcp::EVENTDATA_COMMAND, 0x00C2);
        assert_eq!(gvcp::EVENTDATA_ACK, 0x00C3);

        let mut wrong = EVENT_CMD_GOLDEN;
        wrong[2..4].copy_from_slice(&0x000Du16.to_be_bytes());
        assert!(EventPacket::parse_datagram(src(), &wrong).is_err());
    }

    #[test]
    fn multiple_events_in_one_datagram_are_all_returned() {
        let mut buf = BytesMut::new();
        buf.put_u8(consts::GVCP_CMD_KEY);
        buf.put_u8(0);
        buf.put_u16(gvcp::EVENT_COMMAND);
        buf.put_u16((gvcp::EVENT_ENTRY * 3) as u16);
        buf.put_u16(0x0001);
        for i in 0..3u16 {
            buf.put_u16(0); // reserved
            buf.put_u16(0x1000 + i); // event id
            buf.put_u16(i); // stream channel
            buf.put_u16(100 + i); // block id
            buf.put_u64(u64::from(i) + 1);
        }
        let (_, events) = EventPacket::parse_datagram(src(), &buf).expect("parse");
        assert_eq!(events.len(), 3);
        assert_eq!(
            events.iter().map(|e| e.event_id).collect::<Vec<_>>(),
            vec![0x1000, 0x1001, 0x1002]
        );
        assert_eq!(events[2].block_id, 102);
        assert_eq!(events[2].timestamp_dev, 3);
    }

    #[test]
    fn extended_block_ids_shift_the_timestamp() {
        let mut buf = BytesMut::new();
        buf.put_u8(consts::GVCP_CMD_KEY);
        buf.put_u8(consts::FLAG_EXTENDED_IDS);
        buf.put_u16(gvcp::EVENT_COMMAND);
        buf.put_u16(gvcp::EVENT_ENTRY_EXTENDED as u16);
        buf.put_u16(0x0002);
        buf.put_u16(0); // reserved
        buf.put_u16(0x4321); // event id
        buf.put_u16(3); // stream channel
        buf.put_u16(0); // reserved
        buf.put_u64(0x0102_0304_0506_0708); // 64-bit block id
        buf.put_u64(0x1122_3344_5566_7788); // timestamp

        let (header, events) = EventPacket::parse_datagram(src(), &buf).expect("parse");
        assert!(header.extended_ids);
        assert_eq!(events[0].event_id, 0x4321);
        assert_eq!(events[0].block_id, 0x0102_0304_0506_0708);
        assert_eq!(events[0].timestamp_dev, 0x1122_3344_5566_7788);
    }

    #[test]
    fn eventdata_carries_a_payload() {
        let data = [0xAAu8, 0xBB, 0xCC, 0xDD];
        let mut buf = BytesMut::new();
        buf.put_u8(consts::GVCP_CMD_KEY);
        buf.put_u8(0);
        buf.put_u16(gvcp::EVENTDATA_COMMAND);
        buf.put_u16((gvcp::EVENT_ENTRY + data.len()) as u16);
        buf.put_u16(0x0003);
        buf.put_u16(0);
        buf.put_u16(0x0009);
        buf.put_u16(1);
        buf.put_u16(42);
        buf.put_u64(0xDEAD_BEEF);
        buf.extend_from_slice(&data);

        let (header, events) = EventPacket::parse_datagram(src(), &buf).expect("parse");
        assert_eq!(header.ack_opcode(), gvcp::EVENTDATA_ACK);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_id, 0x0009);
        assert_eq!(events[0].block_id, 42);
        assert_eq!(&events[0].payload[..], &data);
        assert_eq!(
            &events[0].raw[..],
            &buf[8..],
            "entry and data, as the device sent them"
        );
        assert_eq!(&events[0].raw[16..], &data);
    }

    #[test]
    fn ack_matches_spec_bytes() {
        let ack = encode_ack(gvcp::EVENT_ACK, 0xCAFE);
        assert_eq!(ack, [0x00, 0x00, 0x00, 0xC1, 0x00, 0x00, 0xCA, 0xFE]);
    }

    /// Two concurrent receivers must share a multi-event datagram.
    ///
    /// Both tasks start while the queue is empty, so both get past the fast
    /// path and one blocks on the receive lock. When the datagram lands, the
    /// winner queues both events, and the loser wakes holding a lock it must
    /// not carry into `recv_from` — the device has already said everything it
    /// is going to say. Without the recheck after acquiring the receive lock
    /// this hangs with an event sitting in the queue.
    ///
    /// It does *not* cover the narrower ordering the recheck depends on:
    /// publishing the decoded events before releasing the receive lock. That
    /// window is a few instructions wide, so no timing test holds it open
    /// reliably — which is why the events are published in one step inside the
    /// lock rather than split into a returned first and a queued remainder.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_queued_event_is_not_stranded_behind_the_receive_lock() {
        let sock = Arc::new(
            EventSocket::bind(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)
                .await
                .expect("bind"),
        );
        let dest = sock.local_addr().expect("local addr");

        let first = tokio::spawn({
            let sock = Arc::clone(&sock);
            async move { sock.recv().await.expect("first event") }
        });
        let second = tokio::spawn({
            let sock = Arc::clone(&sock);
            async move { sock.recv().await.expect("second event") }
        });

        // Let both tasks reach the receive lock before any data exists.
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut buf = BytesMut::new();
        buf.put_u8(consts::GVCP_CMD_KEY);
        // Ask for an acknowledgement, as a device wanting delivery confirmed
        // does: the ack is sent while the receive lock is held, so this also
        // covers that sending it does not stall the second consumer.
        buf.put_u8(consts::FLAG_ACK_REQUIRED);
        buf.put_u16(gvcp::EVENT_COMMAND);
        buf.put_u16((gvcp::EVENT_ENTRY * 2) as u16);
        buf.put_u16(0x0001);
        for i in 0..2u16 {
            buf.put_u16(0); // reserved
            buf.put_u16(0x2000 + i); // event id
            buf.put_u16(0); // stream channel
            buf.put_u16(i); // block id
            buf.put_u64(u64::from(i));
        }
        let sender = UdpSocket::bind("127.0.0.1:0").await.expect("sender");
        sender.send_to(&buf, dest).await.expect("send");

        let both = tokio::time::timeout(Duration::from_secs(5), async {
            (first.await.expect("join"), second.await.expect("join"))
        })
        .await
        .expect("both receivers finished");

        let mut ids = [both.0.event_id, both.1.event_id];
        ids.sort_unstable();
        assert_eq!(ids, [0x2000, 0x2001]);
    }

    /// An Allied Vision Manta G-125B (`GevMCTT` 100 ms) sent one
    /// `ExposureEnd` event eleven times, each with request id 1, to an
    /// application that read it a second late: a device resends a message
    /// until it is acknowledged, under the same request id. Each copy is
    /// acknowledged, and the event is delivered once.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_retransmitted_event_is_acknowledged_and_delivered_once() {
        let sock = EventSocket::bind(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)
            .await
            .expect("bind");
        let dest = sock.local_addr().expect("local addr");
        let device = UdpSocket::bind("127.0.0.1:0").await.expect("device");
        let event = |request_id: u16, block_id: u16| {
            let mut copy = EVENT_CMD_GOLDEN;
            copy[6..8].copy_from_slice(&request_id.to_be_bytes());
            copy[14..16].copy_from_slice(&block_id.to_be_bytes());
            copy
        };
        for _ in 0..3 {
            device.send_to(&event(1, 8), dest).await.expect("send");
        }
        device.send_to(&event(2, 9), dest).await.expect("send");

        let first = sock.recv().await.expect("first event");
        let second = tokio::time::timeout(Duration::from_secs(2), sock.recv())
            .await
            .expect("the next event")
            .expect("second event");
        assert_eq!((first.block_id, second.block_id), (8, 9), "each once");

        let mut acks = Vec::new();
        let mut buf = [0u8; 64];
        while let Ok(Ok((len, _))) =
            tokio::time::timeout(Duration::from_millis(200), device.recv_from(&mut buf)).await
        {
            acks.push(u16::from_be_bytes([buf[6], buf[7]]));
            assert_eq!(len, consts::GVCP_HEADER);
        }
        assert_eq!(acks, [1, 1, 1, 2], "every copy acknowledged");
        let stats = sock.stats();
        assert_eq!((stats.messages, stats.acknowledged), (4, 4));
        assert_eq!((stats.retransmissions, stats.events), (2, 2));
    }

    /// TC-42: the acknowledgement goes out when the message arrives, not
    /// when the application reads it. Acknowledged at the read, a device
    /// resends every `GevMCTT` until then: an Allied Vision Manta G-125B
    /// sent one event twelve times to a reader a second late.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_event_is_acknowledged_when_it_arrives_not_when_it_is_read() {
        let sock = EventSocket::bind(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)
            .await
            .expect("bind");
        let device = UdpSocket::bind("127.0.0.1:0").await.expect("device");
        device
            .send_to(&EVENT_CMD_GOLDEN, sock.local_addr().expect("local addr"))
            .await
            .expect("send");
        let mut ack = [0u8; 64];
        let (len, _) = tokio::time::timeout(Duration::from_millis(500), device.recv_from(&mut ack))
            .await
            .expect("acknowledged while nobody reads")
            .expect("ack");
        assert_eq!(&ack[..len], &encode_ack(gvcp::EVENT_ACK, 0xCAFE));
        let event = sock.recv().await.expect("the event, read later");
        assert_eq!(event.block_id, 8);
    }

    /// Events nobody reads are bounded: past the capacity the oldest goes,
    /// and is counted.
    #[tokio::test(flavor = "multi_thread")]
    async fn unread_events_beyond_the_capacity_drop_the_oldest() {
        let sock =
            EventSocket::bind_with_capacity(IpAddr::V4(Ipv4Addr::LOCALHOST), 0, 2).expect("bind");
        let dest = sock.local_addr().expect("local addr");
        let device = UdpSocket::bind("127.0.0.1:0").await.expect("device");
        for block in 1..=3u16 {
            let mut event = EVENT_CMD_GOLDEN;
            event[6..8].copy_from_slice(&block.to_be_bytes());
            event[14..16].copy_from_slice(&block.to_be_bytes());
            device.send_to(&event, dest).await.expect("send");
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while sock.stats().events < 3 && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(sock.stats().dropped, 1);
        let first = sock.recv().await.expect("event");
        let second = sock.recv().await.expect("event");
        assert_eq!(
            (first.block_id, second.block_id),
            (2, 3),
            "the oldest dropped"
        );
    }

    /// A device that gives every event the same request id — GigE Vision has
    /// it increment, but a device that does not still sends distinct events —
    /// loses none: a copy is a retransmission only when its events are the
    /// same bytes, and distinct events differ at least in their timestamps.
    #[tokio::test(flavor = "multi_thread")]
    async fn distinct_events_under_one_request_id_are_all_delivered() {
        let sock = EventSocket::bind(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)
            .await
            .expect("bind");
        let dest = sock.local_addr().expect("local addr");
        let device = UdpSocket::bind("127.0.0.1:0").await.expect("device");
        for timestamp in [1u8, 2] {
            let mut event = EVENT_CMD_GOLDEN;
            event[23] = timestamp;
            device.send_to(&event, dest).await.expect("send");
        }
        let first = sock.recv().await.expect("first event");
        let second = tokio::time::timeout(Duration::from_secs(2), sock.recv())
            .await
            .expect("the second event is delivered")
            .expect("second event");
        assert_ne!(first.timestamp_dev, second.timestamp_dev);
    }

    #[test]
    fn reject_short_packet() {
        let err = EventPacket::parse_datagram(src(), &[0x42, 0x00, 0x00]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidData);
    }

    #[test]
    fn reject_ack_shaped_packet() {
        // An acknowledgement, not a command: no 0x42 key byte.
        let mut buf = EVENT_CMD_GOLDEN;
        buf[0] = 0x00;
        assert!(EventPacket::parse_datagram(src(), &buf).is_err());
    }

    #[test]
    fn reject_partial_entry() {
        let mut buf = EVENT_CMD_GOLDEN.to_vec();
        buf.truncate(consts::GVCP_HEADER + 12);
        buf[4..6].copy_from_slice(&12u16.to_be_bytes());
        assert!(EventPacket::parse_datagram(src(), &buf).is_err());
    }
}

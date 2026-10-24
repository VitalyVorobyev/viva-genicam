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
use std::net::{IpAddr, SocketAddr};

use bytes::Bytes;
#[cfg(test)]
use bytes::{BufMut, BytesMut};
use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::UdpSocket;
use tokio::sync::Mutex;
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

/// Async GVCP message channel socket.
pub struct EventSocket {
    sock: UdpSocket,
    buffer: Mutex<Vec<u8>>,
    /// Events decoded from a multi-event datagram but not yet returned.
    pending: Mutex<VecDeque<EventPacket>>,
    /// The request id and the events (the datagram after its GVCP header)
    /// of the last message each device sent, to recognise its
    /// retransmissions.
    last_message: std::sync::Mutex<HashMap<SocketAddr, (u16, Vec<u8>)>>,
}

impl EventSocket {
    /// Bind a GVCP message socket on the provided local address.
    pub async fn bind(local_ip: IpAddr, port: u16) -> io::Result<Self> {
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
        let sock = UdpSocket::from_std(socket.into())?;
        info!(local = %addr, "bound GVCP message socket");
        Ok(Self {
            sock,
            buffer: Mutex::new(vec![0u8; consts::MAX_EVENT_SIZE]),
            pending: Mutex::new(VecDeque::new()),
            last_message: std::sync::Mutex::new(HashMap::new()),
        })
    }

    /// Receive and parse the next GVCP event.
    ///
    /// A datagram carrying several events is decoded once and drained across
    /// successive calls. When the device set the acknowledge-required flag the
    /// acknowledgement is sent before any of its events are returned — a device
    /// that does not get one will retransmit.
    ///
    /// A retransmission repeats the request id and the bytes of the message
    /// before it; it is acknowledged again and its events are not delivered
    /// twice. Distinct events under one request id are each delivered. The
    /// acknowledgement goes out when the message is read, so an application
    /// that reads late makes the device resend: an Allied Vision Manta G-125B
    /// (`GevMCTT` 100 ms) sent one event eleven times to a reader a second
    /// late.
    ///
    /// Concurrent callers are symmetric consumers of one queue, and none of
    /// them is promised the event its own `recv_from` decoded.
    pub async fn recv(&self) -> io::Result<EventPacket> {
        loop {
            if let Some(packet) = self.pending.lock().await.pop_front() {
                return Ok(packet);
            }

            // Two locks, and only one order: the receive lock, then the queue.
            // A caller blocked in `recv_from` must not hold the queue, or a
            // second caller could not drain events that have already arrived.
            let mut buffer = self.buffer.lock().await;
            // Recheck: waiting for the receive lock is the window in which
            // another caller queues a decoded datagram, and those events are
            // older than anything the socket will hand us next.
            if let Some(packet) = self.pending.lock().await.pop_front() {
                return Ok(packet);
            }

            let (len, src) = self.sock.recv_from(&mut buffer[..]).await?;
            trace!(bytes = len, %src, "received GVCP message");

            match EventPacket::parse_datagram(src, &buffer[..len]) {
                Ok((header, events)) => {
                    if events.is_empty() {
                        continue;
                    }
                    if header.ack_required {
                        let ack = encode_ack(header.ack_opcode(), header.request_id);
                        if let Err(err) = self.sock.send_to(&ack, src).await {
                            warn!(%src, error = %err, "failed to acknowledge event");
                        } else {
                            trace!(%src, request_id = header.request_id, "acknowledged event");
                        }
                    }
                    // A retransmission repeats both the request id and the
                    // events: a device that reuses one request id for
                    // distinct events still has them delivered, since their
                    // timestamps differ.
                    let message = (
                        header.request_id,
                        buffer[consts::GVCP_HEADER.min(len)..len].to_vec(),
                    );
                    let repeated = self
                        .last_message
                        .lock()
                        .map(|mut last| last.insert(src, message.clone()))
                        .ok()
                        .flatten()
                        .is_some_and(|previous| previous == message);
                    if repeated {
                        debug!(%src, request_id = header.request_id, "dropping a retransmitted event");
                        continue;
                    }
                    // Every event this datagram carried is published in one
                    // step, while the receive lock is still held. Returning
                    // one directly and queueing the remainder would be the
                    // same code with a window in it: between releasing the
                    // receive lock and queueing the rest, another caller can
                    // take the lock, find the queue empty and block in
                    // `recv_from` on a device that may never speak again,
                    // while the events it wanted sat undelivered.
                    debug!(events = events.len(), %src, "queueing GVCP events");
                    self.pending.lock().await.extend(events);
                }
                Err(err) => {
                    warn!(%src, error = %err, "discarding malformed event packet");
                }
            }
        }
    }

    /// Return the local address bound to the socket.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.sock.local_addr()
    }

    /// Access the underlying UDP socket (tests only).
    #[cfg(test)]
    pub fn socket(&self) -> &UdpSocket {
        &self.sock
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use std::sync::Arc;
    use std::time::Duration;

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

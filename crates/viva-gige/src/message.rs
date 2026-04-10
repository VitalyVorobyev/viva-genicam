//! GVCP message/event channel handling.

use std::io;
use std::io::ErrorKind;
use std::net::{IpAddr, SocketAddr};

use bytes::{Buf, Bytes};
#[cfg(test)]
use bytes::{BufMut, BytesMut};
use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::UdpSocket;
use tokio::sync::Mutex;
use tracing::{debug, info, trace, warn};

/// Constants related to GVCP message packets.
mod consts {
    /// Size of the GVCP message header in bytes.
    pub const GVCP_HEADER: usize = 8;
    /// Opcode identifying a GVCP event data acknowledgement.
    pub const OPCODE_EVENT_DATA_ACK: u16 = 0x000D;
    /// Default receive buffer size requested for the UDP socket (bytes).
    pub const DEFAULT_RCVBUF: usize = 1 << 20; // 1 MiB.
    /// Maximum datagram size accepted on the event channel (bytes).
    pub const MAX_EVENT_SIZE: usize = 2048;
}

/// Parsed representation of a GVCP event packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventPacket {
    /// Source address of the datagram.
    pub src: SocketAddr,
    /// Event identifier reported by the device.
    pub event_id: u16,
    /// Device timestamp carried by the event (ticks).
    pub timestamp_dev: u64,
    /// Stream channel associated with the event when present.
    pub stream_channel: u16,
    /// GVSP block identifier associated with the event when present.
    pub block_id: u16,
    /// Remaining payload bytes following the event header.
    pub payload: Bytes,
}

impl EventPacket {
    fn parse(src: SocketAddr, data: &[u8]) -> io::Result<Self> {
        if data.len() < consts::GVCP_HEADER + 2 {
            return Err(io::Error::new(ErrorKind::InvalidData, "packet too short"));
        }
        if data.len() > consts::MAX_EVENT_SIZE {
            return Err(io::Error::new(ErrorKind::InvalidData, "packet too large"));
        }

        let mut cursor = data;
        let status = cursor.get_u16();
        let opcode = cursor.get_u16();
        let length = cursor.get_u16() as usize;
        let _request_id = cursor.get_u16();

        if status != 0 {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "device reported error status",
            ));
        }
        if opcode != consts::OPCODE_EVENT_DATA_ACK {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "unexpected opcode for event packet",
            ));
        }
        if length + consts::GVCP_HEADER != data.len() {
            return Err(io::Error::new(ErrorKind::InvalidData, "length mismatch"));
        }

        if cursor.remaining() < 2 {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "event payload missing identifier",
            ));
        }
        let event_id = cursor.get_u16();

        if cursor.remaining() < 2 {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "event payload missing notification",
            ));
        }
        let _notification = cursor.get_u16();

        let timestamp_dev = if cursor.remaining() >= 8 {
            let high = cursor.get_u32() as u64;
            let low = cursor.get_u32() as u64;
            (high << 32) | low
        } else {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "event payload missing timestamp",
            ));
        };

        let mut stream_channel = 0u16;
        let mut block_id = 0u16;
        let mut payload_length = 0usize;
        if cursor.remaining() >= 6 {
            stream_channel = cursor.get_u16();
            block_id = cursor.get_u16();
            payload_length = cursor.get_u16() as usize;
        }

        if cursor.remaining() < 2 {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "event payload missing reserved field",
            ));
        }
        // Consume the reserved field when present.
        let _reserved = cursor.get_u16();

        let remaining = cursor.remaining();
        if payload_length != 0 && payload_length != remaining {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "event payload length mismatch",
            ));
        }

        let payload = if remaining > 0 {
            Bytes::copy_from_slice(cursor)
        } else {
            Bytes::new()
        };

        Ok(Self {
            src,
            event_id,
            timestamp_dev,
            stream_channel,
            block_id,
            payload,
        })
    }
}

/// Async GVCP message channel socket.
pub struct EventSocket {
    sock: UdpSocket,
    buffer: Mutex<Vec<u8>>,
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
        })
    }

    /// Receive and parse the next GVCP event packet.
    pub async fn recv(&self) -> io::Result<EventPacket> {
        loop {
            let mut buffer = self.buffer.lock().await;
            let (len, src) = self.sock.recv_from(&mut buffer[..]).await?;
            trace!(bytes = len, %src, "received GVCP message");
            match EventPacket::parse(src, &buffer[..len]) {
                Ok(packet) => {
                    debug!(event_id = packet.event_id, %src, "parsed GVCP event");
                    return Ok(packet);
                }
                Err(err) => {
                    warn!(%src, error = %err, "discarding malformed event packet");
                    continue;
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

    fn build_packet() -> Bytes {
        const EVENT_HEADER_LEN: usize = 20;
        let mut buf = BytesMut::with_capacity(consts::GVCP_HEADER + EVENT_HEADER_LEN + 4);
        buf.put_u16(0); // status
        buf.put_u16(consts::OPCODE_EVENT_DATA_ACK);
        buf.put_u16((EVENT_HEADER_LEN + 4) as u16);
        buf.put_u16(0xCAFE); // request id
        buf.put_u16(0x1234); // event id
        buf.put_u16(0x0001); // notification (unused)
        buf.put_u32(0x0002_0003); // ts high
        buf.put_u32(0x0004_0005); // ts low
        buf.put_u16(7); // stream channel
        buf.put_u16(8); // block id
        buf.put_u16(4); // payload length
        buf.put_u16(0); // reserved
        buf.extend_from_slice(&[1u8, 2, 3, 4]);
        buf.freeze()
    }

    #[tokio::test]
    async fn parse_valid_packet() {
        let packet = build_packet();
        let src = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 3956);
        let parsed = EventPacket::parse(src, &packet).expect("packet");
        assert_eq!(parsed.event_id, 0x1234);
        assert_eq!(parsed.timestamp_dev, 0x0002_0003_0004_0005);
        assert_eq!(parsed.stream_channel, 7);
        assert_eq!(parsed.block_id, 8);
        assert_eq!(&parsed.payload[..], &[1, 2, 3, 4]);
    }

    #[tokio::test]
    async fn reject_short_packet() {
        let src = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 3956);
        let data = Bytes::from_static(&[0x00, 0x01, 0x02]);
        let err = EventPacket::parse(src, &data).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidData);
    }
}

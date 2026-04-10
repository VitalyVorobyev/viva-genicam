//! GVCP action command helpers.

use std::collections::HashSet;
use std::io;
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use bytes::{BufMut, BytesMut};
use tokio::net::UdpSocket;
use tokio::time;
use tracing::{debug, info, trace, warn};

use crate::gvcp::{GVCP_PORT, GvcpAckHeader, GvcpRequestHeader};

/// Constants describing the layout of action command packets.
mod consts {
    /// GVCP opcode for an action command request.
    pub const ACTION_COMMAND: u16 = 0x0080;
    /// GVCP opcode for an action acknowledgement.
    pub const ACTION_ACK: u16 = 0x0081;
    /// Size of the action command payload in bytes.
    pub const ACTION_PAYLOAD: usize = 24;
}

/// Parameters used to construct an action command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionParams {
    /// Vendor-specific device key used to authorise the action.
    pub device_key: u32,
    /// Group key identifying which devices should react to the action.
    pub group_key: u32,
    /// Group mask applied to the device key by receivers.
    pub group_mask: u32,
    /// Optional scheduled time expressed in device clock ticks.
    pub scheduled_time: Option<u64>,
    /// Stream channel identifier associated with the action.
    pub channel: u16,
}

/// Summary of the broadcast performed by [`send_action`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct AckSummary {
    /// Number of GVCP datagrams transmitted.
    pub sent: usize,
    /// Number of distinct acknowledgement sources observed.
    pub acks: usize,
}

fn encode_payload(params: &ActionParams) -> BytesMut {
    let mut buf = BytesMut::with_capacity(consts::ACTION_PAYLOAD);
    buf.put_u32(params.device_key);
    buf.put_u32(params.group_key);
    buf.put_u32(params.group_mask);
    let ticks = params.scheduled_time.unwrap_or(0);
    buf.put_u32((ticks >> 32) as u32);
    buf.put_u32(ticks as u32);
    buf.put_u16(params.channel);
    buf.put_u16(0); // reserved
    buf
}

fn parse_ack(buf: &[u8]) -> io::Result<GvcpAckHeader> {
    if buf.len() < 8 {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "acknowledgement shorter than GVCP header",
        ));
    }
    let status = u16::from_be_bytes([buf[0], buf[1]]);
    let opcode = u16::from_be_bytes([buf[2], buf[3]]);
    let length = u16::from_be_bytes([buf[4], buf[5]]);
    let request_id = u16::from_be_bytes([buf[6], buf[7]]);
    Ok(GvcpAckHeader {
        status: viva_gencp::StatusCode::from_raw(status),
        command: opcode,
        length,
        request_id,
    })
}

fn is_broadcast(addr: &SocketAddr) -> bool {
    matches!(addr.ip(), IpAddr::V4(ip) if ip == Ipv4Addr::BROADCAST)
}

/// Send a GVCP action command and collect acknowledgements.
pub async fn send_action(
    broadcast: SocketAddr,
    params: &ActionParams,
    timeout_ms: u64,
) -> io::Result<AckSummary> {
    let destination = SocketAddr::new(broadcast.ip(), GVCP_PORT);
    let local_ip = match destination.ip() {
        IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        IpAddr::V6(_) => {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "IPv6 destinations are not supported for actions",
            ));
        }
    };
    let socket = UdpSocket::bind(SocketAddr::new(local_ip, 0)).await?;
    if is_broadcast(&destination) {
        socket.set_broadcast(true)?;
    }

    let mut summary = AckSummary::default();
    let payload = encode_payload(params);
    let request_id = fastrand::u16(0x8000..=0xFFFE);
    let mut flags = viva_gencp::CommandFlags::ACK_REQUIRED;
    if is_broadcast(&destination) {
        flags |= viva_gencp::CommandFlags::BROADCAST;
    }
    let header = GvcpRequestHeader {
        flags,
        command: consts::ACTION_COMMAND,
        length: payload.len() as u16,
        request_id,
    };
    let packet = header.encode(&payload);
    trace!(bytes = packet.len(), %destination, request_id, "sending action command");
    socket.send_to(&packet, destination).await?;
    summary.sent = 1;

    let timeout = Duration::from_millis(timeout_ms);
    if timeout.is_zero() {
        info!(acks = 0, "action command sent (no wait)");
        return Ok(summary);
    }

    let start = Instant::now();
    let mut buf = vec![0u8; 512];
    let mut seen = HashSet::new();
    while let Some(remaining) = timeout.checked_sub(start.elapsed()) {
        if remaining.is_zero() {
            break;
        }
        match time::timeout(remaining, socket.recv_from(&mut buf)).await {
            Ok(Ok((len, src))) => {
                trace!(bytes = len, %src, "received acknowledgement");
                let header = parse_ack(&buf[..len])?;
                if header.command != consts::ACTION_ACK {
                    debug!(
                        opcode = header.command,
                        "ignoring unrelated acknowledgement"
                    );
                    continue;
                }
                if header.request_id != request_id {
                    debug!(
                        expected = request_id,
                        got = header.request_id,
                        "acknowledgement id mismatch"
                    );
                    continue;
                }
                if header.status != viva_gencp::StatusCode::Success {
                    warn!(status = ?header.status, %src, "device reported action failure");
                    continue;
                }
                if seen.insert(src.ip()) {
                    summary.acks += 1;
                }
            }
            Ok(Err(err)) => {
                warn!(?err, "error receiving acknowledgement");
                break;
            }
            Err(_) => break,
        }
    }

    info!(acks = summary.acks, "action command completed");
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_layout() {
        let params = ActionParams {
            device_key: 0x1122_3344,
            group_key: 0x5566_7788,
            group_mask: 0xFFFF_0000,
            scheduled_time: Some(0x0102_0304_0506_0708),
            channel: 0x090A,
        };
        let payload = encode_payload(&params);
        assert_eq!(payload.len(), consts::ACTION_PAYLOAD);
        assert_eq!(&payload[..4], &0x1122_3344u32.to_be_bytes());
        assert_eq!(&payload[4..8], &0x5566_7788u32.to_be_bytes());
        assert_eq!(&payload[8..12], &0xFFFF_0000u32.to_be_bytes());
        assert_eq!(&payload[12..16], &0x0102_0304u32.to_be_bytes());
        assert_eq!(&payload[16..20], &0x0506_0708u32.to_be_bytes());
        assert_eq!(&payload[20..22], &0x090A_u16.to_be_bytes());
    }

    #[test]
    fn ack_parser() {
        let mut buf = BytesMut::with_capacity(8);
        buf.put_u16(viva_gencp::StatusCode::Success.to_raw());
        buf.put_u16(consts::ACTION_ACK);
        buf.put_u16(0);
        buf.put_u16(0xBEEF);
        let ack = parse_ack(&buf).expect("ack");
        assert_eq!(ack.command, consts::ACTION_ACK);
        assert_eq!(ack.request_id, 0xBEEF);
    }
}

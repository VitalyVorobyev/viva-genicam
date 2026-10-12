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

use crate::consts;
use crate::gvcp::{GVCP_PORT, GvcpAckHeader, GvcpRequestHeader};

/// Size of an unscheduled action command payload in bytes.
///
/// `device_key` + `group_key` + `group_mask`, per the GigE Vision `ACTION_CMD`
/// field table (Wireshark's `dissect_action_cmd` reads exactly these three).
const ACTION_PAYLOAD: usize = 12;
/// Size of a scheduled action command payload in bytes.
///
/// The base payload plus a 64-bit action time at offset 12, present only when
/// bit 7 of the GVCP flags byte is set.
const ACTION_PAYLOAD_SCHEDULED: usize = ACTION_PAYLOAD + 8;

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
    ///
    /// `Some` selects a scheduled action command: the time is appended to the
    /// payload and bit 7 of the GVCP flags byte is set. A device that does not
    /// implement scheduled actions rejects it, so leave this `None` unless the
    /// camera advertises `GevSupportedOptionScheduledAction`.
    pub scheduled_time: Option<u64>,
}

/// Summary of the broadcast performed by [`send_action`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct AckSummary {
    /// Number of GVCP datagrams transmitted.
    pub sent: usize,
    /// Number of distinct acknowledgement sources observed.
    pub acks: usize,
}

fn encode_payload(params: &ActionParams) -> BytesMut {
    let mut buf = BytesMut::with_capacity(ACTION_PAYLOAD_SCHEDULED);
    buf.put_u32(params.device_key);
    buf.put_u32(params.group_key);
    buf.put_u32(params.group_mask);
    // The action time is not a fixed field padded with zeros: an unscheduled
    // command is 12 bytes and stops here. Sending the extra 8 bytes anyway
    // makes the payload length disagree with the flags byte.
    if let Some(ticks) = params.scheduled_time {
        buf.put_u64(ticks);
    }
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
        status: crate::GigeStatus::from_raw(status),
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
    if params.scheduled_time.is_some() {
        flags |= viva_gencp::CommandFlags::SCHEDULED_ACTION;
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
                if !header.status.is_success() {
                    warn!(status = %header.status, %src, "device reported action failure");
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

    fn params() -> ActionParams {
        ActionParams {
            device_key: 0x1122_3344,
            group_key: 0x5566_7788,
            group_mask: 0xFFFF_0000,
            scheduled_time: None,
        }
    }

    /// The whole datagram, byte for byte, written out from the GigE Vision
    /// `ACTION_CMD` field table rather than from our own encoder.
    ///
    /// This is the fixture that would have caught the 0x0080 collision: the
    /// opcode is a literal here, so an encoder that emits `READREG` fails
    /// rather than agreeing with itself (ADR-0019).
    #[test]
    fn unscheduled_action_matches_spec_bytes() {
        let payload = encode_payload(&params());
        let packet = GvcpRequestHeader {
            flags: viva_gencp::CommandFlags::ACK_REQUIRED,
            command: consts::ACTION_COMMAND,
            length: payload.len() as u16,
            request_id: 0xBEEF,
        }
        .encode(&payload);

        #[rustfmt::skip]
        let expected: [u8; 20] = [
            0x42,                   // command key
            0x01,                   // flags: acknowledge required
            0x01, 0x00,             // ACTION_CMD — *not* 0x0080 (READREG)
            0x00, 0x0C,             // length: 12, no action time
            0xBE, 0xEF,             // request id
            0x11, 0x22, 0x33, 0x44, // device key
            0x55, 0x66, 0x77, 0x88, // group key
            0xFF, 0xFF, 0x00, 0x00, // group mask
        ];
        assert_eq!(&packet[..], &expected[..]);
    }

    /// A scheduled action appends the 64-bit time *and* sets flags bit 7.
    /// Either one alone is a malformed command.
    #[test]
    fn scheduled_action_sets_flag_and_appends_time() {
        let mut p = params();
        p.scheduled_time = Some(0x0102_0304_0506_0708);
        let payload = encode_payload(&p);
        let mut flags = viva_gencp::CommandFlags::ACK_REQUIRED;
        flags |= viva_gencp::CommandFlags::SCHEDULED_ACTION;
        let packet = GvcpRequestHeader {
            flags,
            command: consts::ACTION_COMMAND,
            length: payload.len() as u16,
            request_id: 0xBEEF,
        }
        .encode(&payload);

        #[rustfmt::skip]
        let expected: [u8; 28] = [
            0x42,
            0x81,                   // flags: acknowledge required | scheduled
            0x01, 0x00,
            0x00, 0x14,             // length: 20
            0xBE, 0xEF,
            0x11, 0x22, 0x33, 0x44,
            0x55, 0x66, 0x77, 0x88,
            0xFF, 0xFF, 0x00, 0x00,
            0x01, 0x02, 0x03, 0x04, // action time, big-endian u64
            0x05, 0x06, 0x07, 0x08,
        ];
        assert_eq!(&packet[..], &expected[..]);
        assert_eq!(payload.len(), ACTION_PAYLOAD_SCHEDULED);
    }

    #[test]
    fn unscheduled_payload_stops_at_twelve_bytes() {
        assert_eq!(encode_payload(&params()).len(), ACTION_PAYLOAD);
    }

    /// The two opcodes an action must never be confused with.
    #[test]
    fn action_opcodes_do_not_collide_with_register_access() {
        assert_eq!(consts::ACTION_COMMAND, 0x0100);
        assert_eq!(consts::ACTION_ACK, 0x0101);
        assert_ne!(consts::ACTION_COMMAND, 0x0080); // READREG_CMD
        assert_ne!(consts::ACTION_ACK, 0x0081); // READREG_ACK
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

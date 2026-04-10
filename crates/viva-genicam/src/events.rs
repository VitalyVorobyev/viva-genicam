//! High-level helpers for the GVCP message/event channel.

use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use tracing::{debug, info, warn};
use viva_gige::gvcp::consts as gvcp_consts;
use viva_gige::message::{EventPacket, EventSocket};

use crate::GenicamError;
use crate::time::TimeSync;

/// Public representation of a GigE Vision event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    /// Raw event identifier reported by the device.
    pub id: u16,
    /// Device timestamp associated with the event (ticks).
    pub ts_dev: u64,
    /// Host timestamp mapped from the device ticks when synchronisation is available.
    pub ts_host: SystemTime,
    /// Raw payload bytes following the event header.
    pub data: Bytes,
}

/// Asynchronous stream of events delivered over the GVCP message channel.
pub struct EventStream {
    socket: EventSocket,
    time_sync: Option<Arc<TimeSync>>,
}

impl EventStream {
    pub(crate) fn new(socket: EventSocket, time_sync: Option<Arc<TimeSync>>) -> Self {
        Self { socket, time_sync }
    }

    /// Receive the next event emitted by the device.
    pub async fn next(&self) -> Result<Event, GenicamError> {
        let packet = self
            .socket
            .recv()
            .await
            .map_err(|err| GenicamError::transport(format!("gvcp message recv: {err}")))?;
        debug!(
            event_id = packet.event_id,
            ts_dev = packet.timestamp_dev,
            "event received"
        );
        Ok(Self::map_packet(packet, self.time_sync.clone()))
    }

    /// Access the local socket address used by the stream.
    pub fn local_addr(&self) -> Result<std::net::SocketAddr, GenicamError> {
        self.socket
            .local_addr()
            .map_err(|err| GenicamError::transport(format!("gvcp local addr: {err}")))
    }

    fn map_packet(packet: EventPacket, sync: Option<Arc<TimeSync>>) -> Event {
        let ts_host = match sync {
            Some(sync) if sync.len() > 1 => sync.to_host_time(packet.timestamp_dev),
            Some(sync) => {
                warn!("insufficient time sync samples; using current system time");
                let _ = sync; // keep `sync` alive for future samples
                SystemTime::now()
            }
            None => SystemTime::now(),
        };
        Event {
            id: packet.event_id,
            ts_dev: packet.timestamp_dev,
            ts_host,
            data: packet.payload,
        }
    }
}

/// Attempt to configure the GVCP message channel directly when SFNC nodes are missing.
pub(crate) fn configure_message_channel_raw<T: crate::genapi::RegisterIo>(
    transport: &T,
    ip: Ipv4Addr,
    port: u16,
) -> Result<(), GenicamError> {
    let addr = gvcp_consts::MESSAGE_DESTINATION_ADDRESS;
    transport
        .write(addr, &ip.octets())
        .map_err(|err| GenicamError::transport(format!("write message addr: {err}")))?;
    transport
        .write(gvcp_consts::MESSAGE_DESTINATION_PORT, &port.to_be_bytes())
        .map_err(|err| GenicamError::transport(format!("write message port: {err}")))?;
    info!(%ip, port, "configured message channel via raw registers");
    Ok(())
}

/// Enable or disable delivery of a raw event identifier by toggling the notification mask.
pub(crate) fn enable_event_raw<T: crate::genapi::RegisterIo>(
    transport: &T,
    event_id: u16,
    on: bool,
) -> Result<(), GenicamError> {
    let index = (event_id / 32) as u64;
    let bit = 1u32 << (event_id % 32);
    let addr =
        gvcp_consts::EVENT_NOTIFICATION_BASE + index * gvcp_consts::EVENT_NOTIFICATION_STRIDE;
    let current = transport
        .read(addr, 4)
        .map_err(|err| GenicamError::transport(format!("read event mask: {err}")))?;
    if current.len() != 4 {
        return Err(GenicamError::transport("event mask length mismatch"));
    }
    let mut bytes = [0u8; 4];
    bytes.copy_from_slice(&current);
    let mut value = u32::from_be_bytes(bytes);
    if on {
        value |= bit;
    } else {
        value &= !bit;
    }
    transport
        .write(addr, &value.to_be_bytes())
        .map_err(|err| GenicamError::transport(format!("write event mask: {err}")))?;
    info!(event_id, enabled = on, "updated event notification mask");
    Ok(())
}

/// Parse a textual event identifier into a numeric value for raw fallbacks.
pub(crate) fn parse_event_id(text: &str) -> Option<u16> {
    if let Some(stripped) = text.strip_prefix("0x") {
        u16::from_str_radix(stripped, 16).ok()
    } else if let Some(stripped) = text.strip_prefix("0X") {
        u16::from_str_radix(stripped, 16).ok()
    } else {
        text.parse::<u16>().ok()
    }
}

/// Bind an [`EventSocket`] on the provided interface.
pub(crate) async fn bind_socket(ip: IpAddr, port: u16) -> Result<EventSocket, GenicamError> {
    EventSocket::bind(ip, port)
        .await
        .map_err(|err| GenicamError::transport(format!("bind event socket: {err}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;

    #[test]
    fn parse_numeric_event_ids() {
        assert_eq!(parse_event_id("1234"), Some(1234));
        assert_eq!(parse_event_id("0x00AF"), Some(0x00AF));
        assert_eq!(parse_event_id("0XFF10"), Some(0xFF10));
        assert_eq!(parse_event_id("not-a-number"), None);
    }

    #[test]
    fn map_packet_without_sync() {
        let packet = EventPacket {
            src: SocketAddr::from(([127, 0, 0, 1], 4000)),
            event_id: 0x1000,
            timestamp_dev: 42,
            stream_channel: 0,
            block_id: 0,
            payload: Bytes::from_static(b"abcd"),
        };
        let event = EventStream::map_packet(packet.clone(), None);
        assert_eq!(event.id, packet.event_id);
        assert_eq!(event.ts_dev, packet.timestamp_dev);
        assert_eq!(event.data, packet.payload);
    }
}

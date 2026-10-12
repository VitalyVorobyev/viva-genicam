//! High-level helpers for the GVCP message/event channel.

use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use tracing::{debug, info};
use viva_gige::consts as gvcp_consts;
use viva_gige::{EventPacket, EventSocket};

use crate::GenicamError;
use viva_gige::TimeSync;

/// Public representation of a GigE Vision event.
///
/// Its features — `EventExposureEndTimestamp`, `EventExposureEndFrameID` and
/// the rest — are read through GenApi from the port whose `<EventID>` matches
/// [`Event::id`]: see [`Camera::with_event`](crate::Camera::with_event).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    /// Event identifier reported by the device.
    pub id: u16,
    /// Stream channel the event concerns, when it concerns one.
    pub stream_channel: u16,
    /// GVSP block the event concerns: 16 bits widened, or the 64-bit
    /// extended id.
    pub block_id: u64,
    /// Device timestamp associated with the event (ticks).
    pub ts_dev: u64,
    /// Host time the device timestamp maps to: `None` until the camera's
    /// clock is fitted ([`Camera::time_calibrate`](crate::Camera::time_calibrate)),
    /// as for a frame. Never the time the event arrived, passed off as the
    /// device's (backlog API-20).
    pub ts_host: Option<SystemTime>,
    /// Event data following the event header; empty for an `EVENT_CMD`.
    pub data: Bytes,
    /// The whole event as the device sent it, header included: what the
    /// camera's event port addresses.
    pub raw: Bytes,
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
            .map_err(crate::error::network("receive a GVCP event"))?;
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
            .map_err(crate::error::network("read the event socket's address"))
    }

    fn map_packet(packet: EventPacket, sync: Option<Arc<TimeSync>>) -> Event {
        // A host time only from a fitted clock, as a frame's (DOC-11).
        let ts_host = sync.and_then(|sync| sync.to_host_time(packet.timestamp_dev));
        Event {
            id: packet.event_id,
            stream_channel: packet.stream_channel,
            block_id: packet.block_id,
            ts_dev: packet.timestamp_dev,
            ts_host,
            data: packet.payload,
            raw: packet.raw,
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
        .map_err(GenicamError::from)?;
    // GevMCP is 32-bit with the port in the low half; a bare `u16` write puts
    // it in the high half.
    transport
        .write(
            gvcp_consts::MESSAGE_DESTINATION_PORT,
            &u32::from(port).to_be_bytes(),
        )
        .map_err(GenicamError::from)?;
    info!(%ip, port, "configured message channel via raw registers");
    Ok(())
}

// There is deliberately no raw fallback for *enabling* an event.
//
// One used to live here, toggling a bit in a "notification mask" at
// 0x0900_0300 + id/32. No such bootstrap register exists: event delivery is
// selected through the GenApi `EventSelector` / `EventNotification` features,
// and the address was invented alongside the equally invented message-channel
// pair next to it. Writing to it could only corrupt whatever a real device
// keeps at that address, so a camera without those SFNC nodes now gets an
// error naming what is missing (ADR-0019).

/// Bind an [`EventSocket`] on the provided interface.
pub(crate) async fn bind_socket(ip: IpAddr, port: u16) -> Result<EventSocket, GenicamError> {
    EventSocket::bind(ip, port)
        .await
        .map_err(crate::error::network("bind the event socket"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;

    #[test]
    fn map_packet_without_sync() {
        let packet = EventPacket {
            src: SocketAddr::from(([127, 0, 0, 1], 4000)),
            event_id: 0x1000,
            timestamp_dev: 42,
            stream_channel: 3,
            block_id: 9,
            payload: Bytes::from_static(b"abcd"),
            raw: Bytes::from_static(b"0123456789abcdefabcd"),
        };
        let event = EventStream::map_packet(packet.clone(), None);
        assert_eq!(event.id, packet.event_id);
        assert_eq!(event.ts_dev, packet.timestamp_dev);
        assert_eq!(event.data, packet.payload);
        assert_eq!((event.stream_channel, event.block_id), (3, 9));
        assert_eq!(event.raw, packet.raw);
    }

    fn packet(timestamp_dev: u64) -> EventPacket {
        EventPacket {
            src: SocketAddr::from(([127, 0, 0, 1], 4000)),
            event_id: 0x9001,
            timestamp_dev,
            stream_channel: 0,
            block_id: 1,
            payload: Bytes::new(),
            raw: Bytes::new(),
        }
    }

    /// API-20: without a fitted clock an event has no host time. It used to
    /// get the time it arrived, passed off as the device's.
    #[test]
    fn an_event_has_no_host_time_without_a_fitted_clock() {
        assert_eq!(EventStream::map_packet(packet(42), None).ts_host, None);

        let mut one_sample = TimeSync::new();
        one_sample.update(10, std::time::Instant::now());
        assert_eq!(
            EventStream::map_packet(packet(42), Some(Arc::new(one_sample))).ts_host,
            None
        );
    }

    /// With a fitted clock, the host time is the fit's mapping of the
    /// event's ticks.
    #[test]
    fn an_event_maps_its_ticks_through_a_fitted_clock() {
        let mut sync = TimeSync::new();
        let start = std::time::Instant::now();
        for (ticks, ms) in [(0u64, 0u64), (1_000_000, 1), (2_000_000, 2)] {
            sync.update(ticks, start + std::time::Duration::from_millis(ms));
        }
        sync.fit(Some(1e9));
        let expected = sync.to_host_time(5_000_000);
        assert!(expected.is_some());
        let event = EventStream::map_packet(packet(5_000_000), Some(Arc::new(sync)));
        assert_eq!(event.ts_host, expected);
    }
}

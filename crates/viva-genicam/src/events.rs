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
///
/// A receive thread acknowledges each message as it arrives and queues its
/// events, so a late reader costs the camera no retransmission;
/// [`EventSocket`] says how many it keeps and what it does with more.
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

    /// What the message channel has received: messages, acknowledgements,
    /// the camera's retransmissions, events queued and events dropped
    /// unread.
    pub fn stats(&self) -> viva_gige::EventStats {
        self.socket.stats()
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

/// Events from a USB3 Vision camera's event interface, read on a thread of
/// its own (backlog TC-27 (4)).
///
/// Each is an [`Event`] whose `raw` is the event as GenCP lays it out — its
/// ID at 0x2, its timestamp at 0x4, its data at 0xC — which is what the
/// camera's event ports address ([`Camera::with_event`](crate::Camera::with_event)).
/// USB3 Vision events have no stream channel or block id of their own: both
/// are 0. Dropping the stream stops the reader and restores the device's
/// previous EventEnable control word. Use [`U3vEventStream::shutdown`] for
/// awaited cleanup with restoration errors.
#[cfg(feature = "u3v")]
#[cfg_attr(docsrs, doc(cfg(feature = "u3v")))]
pub struct U3vEventStream {
    shared: Arc<U3vEventShared>,
    reader: Option<std::thread::JoinHandle<()>>,
    disable: Option<Box<dyn FnOnce() -> Result<(), GenicamError> + Send + Sync>>,
}

#[cfg(feature = "u3v")]
struct U3vEventShared {
    queue: crate::stream::EventQueue,
    stop: std::sync::atomic::AtomicBool,
}

/// How long the event reader waits before checking whether to stop.
#[cfg(feature = "u3v")]
const U3V_EVENT_POLL: std::time::Duration = std::time::Duration::from_millis(50);

/// Events kept for the application before the oldest is dropped.
#[cfg(feature = "u3v")]
const U3V_EVENT_CAPACITY: usize = 256;

#[cfg(feature = "u3v")]
pub(crate) fn open_u3v_event_stream<T: viva_u3v::UsbTransfer + 'static>(
    transport: &crate::U3vRegisterIo<T>,
    time_sync: Option<TimeSync>,
) -> Result<U3vEventStream, GenicamError> {
    let device = transport.shared_device();
    let opened = (|| {
        let mut device = device
            .lock()
            .map_err(|_| GenicamError::Panicked("a thread holding the camera's device lock"))?;
        let address = device
            .read_eirm()
            .map_err(|source| GenicamError::U3v {
                during: "read the event interface",
                source,
            })?
            .base
            + viva_u3v::registers::eirm::CONTROL;
        let previous = device
            .read_mem(address, 4)
            .map_err(|source| GenicamError::U3v {
                during: "save EventEnable",
                source,
            })?;
        match device.open_events() {
            Ok(events) => Ok((events, address, previous)),
            Err(source) => {
                // A disconnected device is not written; loss is noted below.
                if !matches!(source, viva_u3v::U3vError::Disconnected)
                    && let Err(error) = device.write_mem(address, &previous)
                {
                    debug!(%error, "restoring EventEnable after event setup failed");
                }
                Err(GenicamError::U3v {
                    during: "open the event stream",
                    source,
                })
            }
        }
    })();
    let (mut events, enable_address, previous_enable) = opened.inspect_err(|error| {
        if matches!(
            error,
            GenicamError::U3v {
                source: viva_u3v::U3vError::Disconnected,
                ..
            }
        ) {
            transport.mark_lost();
        }
    })?;
    let shared = Arc::new(U3vEventShared {
        queue: crate::stream::EventQueue::new(U3V_EVENT_CAPACITY),
        stop: std::sync::atomic::AtomicBool::new(false),
    });
    let lost = transport.lost_flag();
    let reader = {
        let shared = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("viva-u3v-events".into())
            .spawn(move || {
                use std::sync::atomic::Ordering;
                while !shared.stop.load(Ordering::Acquire) {
                    match events.next_event_within(U3V_EVENT_POLL) {
                        Ok(None) => {}
                        Ok(Some(event)) => {
                            let ts_host = time_sync
                                .as_ref()
                                .and_then(|sync| sync.to_host_time(event.timestamp));
                            debug!(
                                event_id = event.id,
                                ts_dev = event.timestamp,
                                "event received"
                            );
                            shared.queue.push(Event {
                                id: event.id,
                                stream_channel: 0,
                                block_id: 0,
                                ts_dev: event.timestamp,
                                ts_host,
                                data: event.data,
                                raw: event.raw,
                            });
                        }
                        Err(error) => {
                            if matches!(error, viva_u3v::U3vError::Disconnected) {
                                lost.store(true, Ordering::Release);
                            }
                            shared.queue.fail("read an event", error);
                            break;
                        }
                    }
                }
                shared.queue.finish();
            })
    };
    let connection = transport.connection();
    let disable = Box::new(move || {
        if connection.is_lost() {
            return Err(GenicamError::U3v {
                during: "restore EventEnable",
                source: viva_u3v::U3vError::Disconnected,
            });
        }
        let mut device = device
            .lock()
            .map_err(|_| GenicamError::Panicked("a thread holding the camera's device lock"))?;
        device
            .write_mem(enable_address, &previous_enable)
            .map_err(|source| GenicamError::U3v {
                during: "restore EventEnable",
                source,
            })
    });
    let reader = match reader {
        Ok(reader) => reader,
        Err(error) => {
            if let Err(restore_error) = disable() {
                debug!(%restore_error, "restoring EventEnable after reader spawn failed");
            }
            return Err(crate::error::network("start the event reader thread")(
                error,
            ));
        }
    };
    Ok(U3vEventStream {
        shared,
        reader: Some(reader),
        disable: Some(disable),
    })
}

#[cfg(feature = "u3v")]
impl U3vEventStream {
    /// Stop and join the event reader on the blocking pool, then restore the
    /// exact EventEnable control word saved before the reader was opened.
    ///
    /// Do not hold the camera or device mutex while awaiting this method.
    /// A disconnected device is not written; the returned error records that
    /// its previous EventEnable state could not be restored.
    pub async fn shutdown(mut self) -> Result<(), GenicamError> {
        tokio::task::spawn_blocking(move || self.stop_and_restore())
            .await
            .map_err(|source| GenicamError::Task {
                during: "stop the event reader",
                source,
            })?
    }

    fn stop_and_restore(&mut self) -> Result<(), GenicamError> {
        self.shared
            .stop
            .store(true, std::sync::atomic::Ordering::Release);
        let joined = self.reader.take().map_or(Ok(()), |reader| {
            reader
                .join()
                .map_err(|_| GenicamError::Panicked("the USB event reader"))
        });
        let restored = self.disable.take().map_or(Ok(()), |restore| restore());
        joined.and(restored)
    }

    /// The next event; an error once the stream has ended, by failing —
    /// with [`U3vError::Disconnected`](viva_u3v::U3vError::Disconnected) as
    /// the source when the camera is gone — or by being closed.
    pub async fn next(&self) -> Result<Event, GenicamError> {
        let popped = self.shared.queue.pop().await;
        Self::take(popped)?.ok_or(GenicamError::Panicked("the event stream has ended"))
    }

    /// Wait up to `timeout` for the next event on the calling thread:
    /// `Ok(None)` when the stream has ended, [`GenicamError::Timeout`] when
    /// nothing arrived.
    pub fn next_blocking(
        &self,
        timeout: std::time::Duration,
    ) -> Result<Option<Event>, GenicamError> {
        match self.shared.queue.pop_blocking(timeout) {
            crate::stream::EventPopped::Empty => Err(GenicamError::Timeout(timeout)),
            popped => Self::take(popped),
        }
    }

    fn take(popped: crate::stream::EventPopped) -> Result<Option<Event>, GenicamError> {
        match popped {
            crate::stream::EventPopped::Item(event) => Ok(Some(event)),
            crate::stream::EventPopped::Failed(during, source) => {
                Err(GenicamError::U3v { during, source })
            }
            _ => Ok(None),
        }
    }
}

#[cfg(feature = "u3v")]
impl Drop for U3vEventStream {
    fn drop(&mut self) {
        if let Err(error) = self.stop_and_restore() {
            debug!(%error, "stopping the USB event reader");
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

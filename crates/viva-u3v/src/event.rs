//! USB3 Vision events: GenCP `EVENT_CMD` packets on the event endpoint
//! (backlog TC-27 (4)).
//!
//! An event command is GenCP's (GenCP 1.3.1 §4.4.10, Table 17), framed as
//! USB3 Vision frames every GenCP packet, little-endian:
//!
//! ```text
//! [ "U3VE" ][ flags ][ 0x0C00 ][ length ][ request_id ][ event … ]
//! event:    [ event_size ][ event_id ][ timestamp (ns) ][ data … ]
//! ```
//!
//! With the device's MultiEvent mode on, one command carries several events,
//! each `event_size` bytes; otherwise `event_size` is 0 and the one event
//! fills the command. The `U3VE` prefix is Wireshark's (`packet-u3v.c`,
//! `U3V_EVENT_PREFIX`); the event layout is GenCP's, and the Grasshopper3's
//! XML addresses it on its event ports as GenCP lays it out: the event ID at
//! 0x2, the timestamp at 0x4 and the frame ID, its data, at 0xC
//! (`PGR_GS3_U3_23S6M.xml`). No camera has sent us one.

use std::collections::VecDeque;
use std::time::Duration;

use bytes::Bytes;

use crate::U3vError;
use crate::usb::{BulkIn, GONE_GRACE, unless_gone};

/// `U3VE` read as a little-endian `u32`: the prefix of an event command.
const EVENT_PREFIX: u32 = 0x4556_3355;

/// Prefix plus common command data.
const HEADER_SIZE: usize = 12;

/// `event_size`, `event_id` and the timestamp: what every event carries.
const EVENT_HEADER_SIZE: usize = 12;

/// GenCP's `EVENT_CMD`.
const EVENT_CMD: u16 = viva_gencp::command_id::EVENT_CMD;

/// Event commands queued on the endpoint.
const EVENTS_IN_FLIGHT: usize = 4;

/// Transfer errors in a row after which the event stream gives up.
const MAX_CONSECUTIVE_FAILURES: u32 = 8;

/// One event a USB3 Vision device sent.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct U3vEvent {
    /// The event's ID: what the XML's event port names in its `<EventID>`.
    pub id: u16,
    /// Device time of the event, in ns (GenCP 1.3.1 Table 17).
    pub timestamp: u64,
    /// The data after the event's 12-byte header.
    pub data: Bytes,
    /// The event as the device laid it out, from its `event_size` field on:
    /// what the camera's event port addresses.
    pub raw: Bytes,
}

/// The events in one event command.
///
/// A packet that is not an event command, or whose events do not tile its
/// length, is an error; a command carrying no event is an empty list.
pub fn parse_event_command(packet: &[u8]) -> Result<Vec<U3vEvent>, U3vError> {
    let malformed = |problem| U3vError::Malformed {
        packet: "event command",
        problem,
    };
    let header = packet
        .get(..HEADER_SIZE)
        .ok_or(malformed("is shorter than its header"))?;
    if u32::from_le_bytes(header[..4].try_into().unwrap()) != EVENT_PREFIX {
        return Err(malformed("lacks the U3VE prefix"));
    }
    if u16::from_le_bytes([header[6], header[7]]) != EVENT_CMD {
        return Err(malformed("is not an EVENT_CMD"));
    }
    let length = usize::from(u16::from_le_bytes([header[8], header[9]]));
    let scd = packet
        .get(HEADER_SIZE..HEADER_SIZE + length)
        .ok_or(malformed("is shorter than its length field"))?;
    let shared = Bytes::copy_from_slice(scd);

    let mut events = Vec::new();
    let mut offset = 0;
    while offset < scd.len() {
        let rest = &scd[offset..];
        if rest.len() < EVENT_HEADER_SIZE {
            return Err(malformed("ends inside an event header"));
        }
        let size = match usize::from(u16::from_le_bytes([rest[0], rest[1]])) {
            // MultiEvent off: one event, the whole command.
            0 => rest.len(),
            size if size < EVENT_HEADER_SIZE => {
                return Err(malformed("declares an event shorter than its header"));
            }
            size if size > rest.len() => {
                return Err(malformed("declares an event longer than the command"));
            }
            size => size,
        };
        let raw = shared.slice(offset..offset + size);
        events.push(U3vEvent {
            id: u16::from_le_bytes([rest[2], rest[3]]),
            timestamp: u64::from_le_bytes(rest[4..12].try_into().unwrap()),
            data: raw.slice(EVENT_HEADER_SIZE..),
            raw,
        });
        offset += size;
    }
    Ok(events)
}

/// The events a device sends, read from its event endpoint with a few event
/// commands' transfers queued.
pub struct U3vEventStream {
    queue: Box<dyn BulkIn>,
    transfer: usize,
    events: VecDeque<U3vEvent>,
    failures: u32,
    skipped: u64,
    warned_ack: bool,
}

impl U3vEventStream {
    /// Events from `queue`, each command at most `max_transfer` bytes (the
    /// EIRM's Maximum Event Transfer Length; 1024 when it says 0), every
    /// transfer a whole number of `packet`-byte packets.
    pub fn new(queue: Box<dyn BulkIn>, max_transfer: u32, packet: Option<usize>) -> Self {
        let packet = packet.unwrap_or(1).max(1);
        let max = match max_transfer {
            0 => 1024,
            bytes => bytes as usize,
        };
        Self {
            queue,
            transfer: max.div_ceil(packet) * packet,
            events: VecDeque::new(),
            failures: 0,
            skipped: 0,
            warned_ack: false,
        }
    }

    /// Event transfers that did not parse as event commands, skipped.
    pub fn skipped(&self) -> u64 {
        self.skipped
    }

    /// The next event, or `Ok(None)` when none arrived within `idle`. A
    /// failed transfer resets the endpoint and the stream goes on;
    /// [`U3vError::Disconnected`], or transfers that keep failing, end it.
    pub fn next_event_within(&mut self, idle: Duration) -> Result<Option<U3vEvent>, U3vError> {
        loop {
            if let Some(event) = self.events.pop_front() {
                return Ok(Some(event));
            }
            while self.queue.pending() < EVENTS_IN_FLIGHT {
                self.queue.submit(self.transfer);
            }
            let Some(completion) = self.queue.wait_next(idle) else {
                return Ok(None);
            };
            match completion.status {
                Ok(()) => self.take(&completion.data),
                Err(U3vError::Disconnected) => return Err(U3vError::Disconnected),
                // Any other failure — a transfer the host cancelled too,
                // which the reader cancels only while recovering, below, and
                // an unplug on macOS does (UV-03) — is counted, and the
                // endpoint reset, as the stream does: cancellations that
                // never stop end the reader rather than spin it (Codex review
                // of #205).
                Err(error) => {
                    let error = unless_gone(error, || self.queue.is_present(), Duration::ZERO);
                    if matches!(error, U3vError::Disconnected) {
                        return Err(error);
                    }
                    self.failures += 1;
                    tracing::warn!(%error, "an event transfer failed; resetting the endpoint");
                    if self.failures > MAX_CONSECUTIVE_FAILURES {
                        return Err(unless_gone(error, || self.queue.is_present(), GONE_GRACE));
                    }
                    self.queue.cancel_all();
                    while self.queue.pending() > 0 && self.queue.wait_next(idle).is_some() {}
                    self.queue
                        .reset()
                        .map_err(|e| unless_gone(e, || self.queue.is_present(), GONE_GRACE))?;
                }
            }
        }
    }

    fn take(&mut self, packet: &[u8]) {
        match parse_event_command(packet) {
            Ok(events) => {
                self.failures = 0;
                // GenCP's RequestAck flag: the device wants an EVENT_ACK,
                // which the event interface, an IN endpoint, cannot carry
                // (backlog UV-06).
                let flags = u16::from_le_bytes([packet[4], packet[5]]);
                if flags & viva_gencp::ccd_flags::REQUEST_ACK != 0 && !self.warned_ack {
                    self.warned_ack = true;
                    tracing::warn!(
                        "the camera asks for event acknowledges, which are not sent: \
                         please report this camera (backlog UV-06)"
                    );
                }
                self.events.extend(events);
            }
            Err(error) => {
                self.skipped += 1;
                tracing::debug!(%error, bytes = packet.len(), "skipping an event transfer");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An event command written out by hand from GenCP 1.3.1 Table 17.
    fn command(flags: u16, scd: &[u8]) -> Vec<u8> {
        let mut packet = b"U3VE".to_vec();
        packet.extend_from_slice(&flags.to_le_bytes());
        packet.extend_from_slice(&0x0C00u16.to_le_bytes());
        packet.extend_from_slice(&(scd.len() as u16).to_le_bytes());
        packet.extend_from_slice(&7u16.to_le_bytes());
        packet.extend_from_slice(scd);
        packet
    }

    #[test]
    fn one_event_fills_a_command_without_multievent() {
        #[rustfmt::skip]
        let scd = [
            0x00, 0x00,                                     // event_size 0
            0x43, 0x9C,                                     // event_id 0x9C43
            0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11, // timestamp
            0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // frame id 5
        ];
        let events = parse_event_command(&command(0, &scd)).unwrap();
        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert_eq!(event.id, 0x9C43);
        assert_eq!(event.timestamp, 0x1122_3344_5566_7788);
        assert_eq!(&event.data[..], &scd[12..]);
        assert_eq!(&event.raw[..], &scd[..]);
    }

    #[test]
    fn multievent_commands_carry_sized_events() {
        let mut scd = Vec::new();
        for (id, extra) in [(0x9001u16, 8usize), (0x9002, 0)] {
            scd.extend_from_slice(&((12 + extra) as u16).to_le_bytes());
            scd.extend_from_slice(&id.to_le_bytes());
            scd.extend_from_slice(&u64::from(id).to_le_bytes());
            scd.extend(std::iter::repeat_n(0xAB, extra));
        }
        let events = parse_event_command(&command(0, &scd)).unwrap();
        assert_eq!(
            events
                .iter()
                .map(|e| (e.id, e.data.len()))
                .collect::<Vec<_>>(),
            [(0x9001, 8), (0x9002, 0)]
        );
        assert_eq!(events[1].raw.len(), 12);
    }

    #[test]
    fn malformed_commands_are_refused() {
        assert!(parse_event_command(b"U3VE").is_err());
        let mut wrong_prefix = command(0, &[0; 12]);
        wrong_prefix[3] = b'C';
        assert!(parse_event_command(&wrong_prefix).is_err());
        let mut too_long = vec![0u8; 12];
        too_long[0] = 40;
        assert!(parse_event_command(&command(0, &too_long)).is_err());
        let mut short_header = command(0, &[0; 12]);
        short_header.truncate(20);
        assert!(parse_event_command(&short_header).is_err());
    }

    /// UV-03 on macOS: an event transfer that fails with an unknown error on
    /// a device the operating system no longer lists ends the event stream
    /// with a disconnect; on a device still listed the stream goes on.
    #[test]
    fn an_unknown_failure_is_a_disconnect_only_once_the_device_is_unlisted() {
        use crate::usb::{MockUsbTransfer, UsbTransfer};
        use std::sync::Arc;
        const EP: u8 = 0x83;
        let not_responding = || U3vError::usb("unknown (error 0xe00002ed)");
        let idle = Duration::from_millis(1);

        let mock = Arc::new(MockUsbTransfer::new());
        mock.set_present(Some(true));
        mock.enqueue_error(EP, not_responding());
        mock.enqueue_read(EP, command(0, &[0, 0, 0x01, 0x90, 1, 0, 0, 0, 0, 0, 0, 0]));
        let mut events = U3vEventStream::new(mock.bulk_in(EP).unwrap(), 0, None);
        let event = events.next_event_within(idle).unwrap().expect("an event");
        assert_eq!(event.id, 0x9001);

        mock.set_present(Some(false));
        mock.enqueue_error(EP, not_responding());
        let result = events.next_event_within(idle);
        assert!(matches!(result, Err(U3vError::Disconnected)), "{result:?}");
    }

    /// Codex review of #205: a transfer the host cancelled, on a device still
    /// listed, is a failure like any other — counted, and the endpoint reset —
    /// so that cancellations that never stop end the reader instead of
    /// spinning it. A good event in between starts the count again.
    #[test]
    fn transfers_the_host_keeps_cancelling_end_the_event_reader() {
        use crate::usb::{MockUsbTransfer, UsbTransfer};
        use std::sync::Arc;
        const EP: u8 = 0x83;
        let idle = Duration::from_millis(1);

        let mock = Arc::new(MockUsbTransfer::new());
        mock.set_present(Some(true));
        mock.enqueue_error(EP, U3vError::Cancelled);
        mock.enqueue_read(EP, command(0, &[0, 0, 0x01, 0x90, 1, 0, 0, 0, 0, 0, 0, 0]));
        for _ in 0..=MAX_CONSECUTIVE_FAILURES {
            mock.enqueue_error(EP, U3vError::Cancelled);
        }
        let mut events = U3vEventStream::new(mock.bulk_in(EP).unwrap(), 0, None);
        let event = events.next_event_within(idle).unwrap().expect("an event");
        assert_eq!(event.id, 0x9001);
        let result = events.next_event_within(idle);
        assert!(matches!(result, Err(U3vError::Cancelled)), "{result:?}");
    }
}

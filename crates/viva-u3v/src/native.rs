//! The `usb` feature's transport: [`nusb`], a pure-Rust USB library
//! (ADR-0032).
//!
//! One camera has run it, on macOS over USB 2 (ADR-0032 §8); Linux,
//! Windows and SuperSpeed have not (backlog TC-27). What the code relies on
//! is `nusb`'s documented behaviour: transfers on one endpoint
//! complete in the order they were submitted, a cancelled transfer still
//! comes back, an IN transfer ends at a short packet, and its length must
//! be a multiple of the endpoint's maximum packet size.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nusb::transfer::{Buffer, Bulk, ControlOut, ControlType, In, Out, Recipient, TransferError};
use nusb::{Endpoint, Interface, MaybeFuture};

use crate::U3vError;
use crate::bootstrap::UsbSpeed;
use crate::descriptor::{U3vInterfaceInfo, parse_u3v_interfaces};
use crate::discovery::U3vDeviceInfo;
use crate::usb::{BulkIn, Completion, UsbTransfer};

/// The standard `SET_FEATURE` request, and the `ENDPOINT_HALT` feature
/// (USB 3.2 §9.4.9, Table 9-7).
const SET_FEATURE: u8 = 0x03;
const ENDPOINT_HALT: u16 = 0;

/// How long the control transfers of an endpoint reset may take.
const RESET_TIMEOUT: Duration = Duration::from_millis(1000);

/// [`UsbTransfer`] over a USB3 Vision device opened with `nusb`: its
/// control interface claimed, and its streaming and event interfaces when
/// it has them.
///
/// A streaming or event interface that cannot be claimed does not stop the
/// camera from opening — its features still work — but opening a stream or
/// events on it returns the claim's error (backlog UV-03: it used to be
/// discarded).
pub struct NusbTransfer {
    interfaces: U3vInterfaceInfo,
    control_in: Mutex<Endpoint<Bulk, In>>,
    control_out: Mutex<Endpoint<Bulk, Out>>,
    stream: Option<Claimed>,
    event: Option<Claimed>,
    packet_sizes: HashMap<u8, usize>,
    speed: Option<UsbSpeed>,
    presence: Arc<Presence>,
}

/// Whether the opened device is still listed by the operating system: what
/// tells an unplug apart on macOS, where the transfers of an unplugged
/// mvBlueFOX3-2004G failed with `kIOReturnNotResponding`,
/// `kIOReturnNotReady` and `kIOReturnNotAttached` — unknown errors to
/// `nusb`, which maps only `kIOReturnNoDevice` to a disconnect (UV-03).
struct Presence {
    /// `nusb`'s id for the device: the IORegistry entry on macOS, the bus
    /// and address on Linux, the device instance on Windows. A device
    /// plugged in again gets a new one.
    id: nusb::DeviceId,
    /// Once unlisted, gone for good: this handle cannot reach it again.
    gone: AtomicBool,
}

impl Presence {
    /// One look at the device list; `None` when it cannot be read.
    fn check(&self) -> Option<bool> {
        if self.gone.load(Ordering::Relaxed) {
            return Some(false);
        }
        let mut devices = nusb::list_devices().wait().ok()?;
        let present = devices.any(|device| device.id() == self.id);
        if !present {
            self.gone.store(true, Ordering::Relaxed);
        }
        Some(present)
    }
}

/// A data interface, claimed or not.
struct Claimed {
    number: u8,
    interface: Result<Interface, nusb::Error>,
}

impl NusbTransfer {
    /// Open the device enumeration reported as `info` and claim its
    /// USB3 Vision interfaces: control, and streaming and events where it
    /// has them. [`U3vDevice::open_device`](crate::U3vDevice::open_device)
    /// opens one this way.
    pub fn open(info: &U3vDeviceInfo) -> Result<Self, U3vError> {
        let found = nusb::list_devices()
            .wait()
            .map_err(nusb_error)?
            .find(|d| {
                d.bus_id() == info.bus
                    && d.device_address() == info.address
                    && d.vendor_id() == info.vendor_id
                    && d.product_id() == info.product_id
            })
            .ok_or_else(|| U3vError::NotFound {
                bus: info.bus.clone(),
                address: info.address,
            })?;
        let device = found.open().wait().map_err(nusb_error)?;
        let (interfaces, packet_sizes) = {
            let config = device.active_configuration().map_err(U3vError::usb)?;
            parse_u3v_interfaces(&config)
                .ok_or(U3vError::Device("has no USB3 Vision control interface"))?
        };

        let claim = |number: u8| device.claim_interface(number).wait();
        let control = claim(interfaces.control_iface).map_err(|source| U3vError::Claim {
            interface: "control",
            number: interfaces.control_iface,
            source: Box::new(source),
        })?;
        let control_in = control
            .endpoint::<Bulk, In>(interfaces.control_ep_in)
            .map_err(nusb_error)?;
        let control_out = control
            .endpoint::<Bulk, Out>(interfaces.control_ep_out)
            .map_err(nusb_error)?;
        let data_interface = |which: &'static str, number: Option<u8>| {
            number.map(|number| {
                let interface = claim(number);
                if let Err(error) = &interface {
                    tracing::warn!(
                        interface = which,
                        number,
                        %error,
                        "cannot claim the camera's {which} interface; its features work, \
                         but opening its {which} will fail"
                    );
                }
                Claimed { number, interface }
            })
        };
        let stream = data_interface("stream", interfaces.stream_iface);
        let event = data_interface("event", interfaces.event_iface);

        Ok(Self {
            interfaces,
            control_in: Mutex::new(control_in),
            control_out: Mutex::new(control_out),
            stream,
            event,
            packet_sizes: packet_sizes.into_iter().collect(),
            speed: found.speed().map(speed),
            presence: Arc::new(Presence {
                id: found.id(),
                gone: AtomicBool::new(false),
            }),
        })
    }

    /// The device's USB3 Vision interfaces and endpoints.
    pub fn interfaces(&self) -> &U3vInterfaceInfo {
        &self.interfaces
    }

    /// The claimed interface `endpoint` belongs to.
    fn data_interface(&self, endpoint: u8) -> Result<&Interface, U3vError> {
        let (which, claimed) = if Some(endpoint) == self.interfaces.stream_ep_in {
            ("stream", self.stream.as_ref())
        } else if Some(endpoint) == self.interfaces.event_ep_in {
            ("event", self.event.as_ref())
        } else {
            return Err(U3vError::Device("has no such bulk IN endpoint"));
        };
        let claimed = claimed.ok_or(U3vError::Device("has no such interface"))?;
        claimed
            .interface
            .as_ref()
            .map_err(|source| U3vError::Claim {
                interface: which,
                number: claimed.number,
                source: Box::new(source.clone()),
            })
    }

    fn packet_size(&self, endpoint: u8) -> usize {
        self.packet_sizes
            .get(&endpoint)
            .copied()
            .unwrap_or(1)
            .max(1)
    }
}

impl UsbTransfer for NusbTransfer {
    fn bulk_write(&self, endpoint: u8, data: &[u8], timeout: Duration) -> Result<usize, U3vError> {
        if endpoint != self.interfaces.control_ep_out {
            return Err(U3vError::Device("has no such bulk OUT endpoint"));
        }
        let mut out = self
            .control_out
            .lock()
            .map_err(|_| U3vError::Device("control endpoint lock poisoned"))?;
        let completion = out.transfer_blocking(Buffer::from(data), timeout);
        completion
            .status
            .map_err(|error| blocking_error(endpoint, error))?;
        Ok(completion.actual_len)
    }

    fn bulk_read(
        &self,
        endpoint: u8,
        buf: &mut [u8],
        timeout: Duration,
    ) -> Result<usize, U3vError> {
        let requested = buf.len().div_ceil(self.packet_size(endpoint)) * self.packet_size(endpoint);
        let completion = if endpoint == self.interfaces.control_ep_in {
            let mut control = self
                .control_in
                .lock()
                .map_err(|_| U3vError::Device("control endpoint lock poisoned"))?;
            control.transfer_blocking(Buffer::new(requested.max(1)), timeout)
        } else {
            let mut queue = self
                .data_interface(endpoint)?
                .endpoint::<Bulk, In>(endpoint)
                .map_err(nusb_error)?;
            queue.transfer_blocking(Buffer::new(requested.max(1)), timeout)
        };
        completion
            .status
            .map_err(|error| blocking_error(endpoint, error))?;
        let data = completion.buffer.into_vec();
        if data.len() > buf.len() {
            return Err(U3vError::Length {
                what: "bulk IN transfer",
                expected: buf.len(),
                got: data.len(),
            });
        }
        buf[..data.len()].copy_from_slice(&data);
        Ok(data.len())
    }

    fn bulk_in(self: &Arc<Self>, endpoint: u8) -> Result<Box<dyn BulkIn>, U3vError> {
        let interface = self.data_interface(endpoint)?.clone();
        let queue = interface
            .endpoint::<Bulk, In>(endpoint)
            .map_err(nusb_error)?;
        Ok(Box::new(NusbBulkIn {
            packet: queue.max_packet_size().max(1),
            queue,
            interface,
            endpoint,
            presence: Arc::clone(&self.presence),
        }))
    }

    fn max_packet_size(&self, endpoint: u8) -> Option<usize> {
        self.packet_sizes.get(&endpoint).copied()
    }

    fn link_speed(&self) -> Option<UsbSpeed> {
        self.speed
    }

    fn is_present(&self) -> Option<bool> {
        self.presence.check()
    }
}

/// A bulk IN endpoint with `nusb`'s own transfer queue.
struct NusbBulkIn {
    queue: Endpoint<Bulk, In>,
    /// For the control request that halts the endpoint.
    interface: Interface,
    endpoint: u8,
    packet: usize,
    presence: Arc<Presence>,
}

impl BulkIn for NusbBulkIn {
    fn submit(&mut self, len: usize) {
        // nusb refuses an IN transfer that is not a whole number of packets.
        let len = len.max(1).div_ceil(self.packet) * self.packet;
        self.queue.submit(Buffer::new(len));
    }

    fn pending(&self) -> usize {
        self.queue.pending()
    }

    fn wait_next(&mut self, timeout: Duration) -> Option<Completion> {
        if self.queue.pending() == 0 {
            return None;
        }
        let completion = self.queue.wait_next_complete(timeout)?;
        let status = completion
            .status
            .map_err(|error| transfer_error(self.endpoint, error));
        Some(Completion::new(completion.buffer.into_vec(), status))
    }

    fn cancel_all(&mut self) {
        self.queue.cancel_all();
    }

    fn reset(&mut self) -> Result<(), U3vError> {
        self.interface
            .control_out(
                ControlOut {
                    control_type: ControlType::Standard,
                    recipient: Recipient::Endpoint,
                    request: SET_FEATURE,
                    value: ENDPOINT_HALT,
                    index: u16::from(self.endpoint),
                    data: &[],
                },
                RESET_TIMEOUT,
            )
            .wait()
            .map_err(|error| transfer_error(self.endpoint, error))?;
        self.queue.clear_halt().wait().map_err(nusb_error)
    }

    fn is_present(&self) -> Option<bool> {
        self.presence.check()
    }
}

/// A transfer's failure as the stream tells them apart: a disconnect ends
/// it, a stall is recovered from, and the rest are recovered from as well
/// (backlog UV-05 keeps which ones a camera really sends open).
fn transfer_error(endpoint: u8, error: TransferError) -> U3vError {
    match error {
        TransferError::Disconnected => U3vError::Disconnected,
        TransferError::Stall => U3vError::Stalled { endpoint },
        TransferError::Cancelled => U3vError::Cancelled,
        TransferError::Unknown(code) => U3vError::usb(OsError(code)),
        other => U3vError::usb(other),
    }
}

/// An operating-system code `nusb` passes on without a meaning: named when
/// it is one of the IOKit codes an unplug on macOS produced, so a log says
/// what happened. None of them is taken as a disconnect on its own —
/// `kIOReturnNotResponding` can be transient — the device list decides
/// ([`U3vError::may_mean_gone`]).
#[derive(Debug)]
struct OsError(u32);

/// The IOKit codes an unplugged device's transfers fail with, as
/// `IOReturn.h` names them.
const IOKIT_CODES: [(u32, &str); 4] = [
    (0xe000_02c0, "kIOReturnNoDevice"),
    (0xe000_02d8, "kIOReturnNotReady"),
    (0xe000_02d9, "kIOReturnNotAttached"),
    (0xe000_02ed, "kIOReturnNotResponding"),
];

impl std::fmt::Display for OsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match IOKIT_CODES.iter().find(|(code, _)| *code == self.0) {
            Some((_, name)) => write!(f, "{name} (error {:#010x})", self.0),
            None => write!(f, "unknown (error {:#010x})", self.0),
        }
    }
}

impl std::error::Error for OsError {}

/// [`transfer_error`] for a blocking transfer, which `nusb` cancels when it
/// times out.
fn blocking_error(endpoint: u8, error: TransferError) -> U3vError {
    match error {
        TransferError::Cancelled => U3vError::Timeout,
        other => transfer_error(endpoint, other),
    }
}

/// A failure that is not a transfer's: opening, claiming, clearing a halt.
fn nusb_error(error: nusb::Error) -> U3vError {
    match error.kind() {
        nusb::ErrorKind::Disconnected => U3vError::Disconnected,
        _ => U3vError::usb(error),
    }
}

/// The speed `nusb` reports.
pub(crate) fn speed(speed: nusb::Speed) -> UsbSpeed {
    match speed {
        nusb::Speed::Low => UsbSpeed::Low,
        nusb::Speed::Full => UsbSpeed::Full,
        nusb::Speed::High => UsbSpeed::High,
        nusb::Speed::Super => UsbSpeed::Super,
        // `nusb::Speed` is non-exhaustive; anything newer is faster.
        _ => UsbSpeed::SuperPlus,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// UV-02/UV-03: a disconnect is its own error, a stall names its
    /// endpoint, a blocking transfer's cancellation is its timeout, and the
    /// rest keep the library's error as their source.
    #[test]
    fn transfer_failures_map_to_what_the_stream_tells_apart() {
        assert!(matches!(
            transfer_error(0x82, TransferError::Disconnected),
            U3vError::Disconnected
        ));
        assert!(matches!(
            transfer_error(0x82, TransferError::Stall),
            U3vError::Stalled { endpoint: 0x82 }
        ));
        assert!(matches!(
            transfer_error(0x82, TransferError::Cancelled),
            U3vError::Cancelled
        ));
        assert!(matches!(
            blocking_error(0x81, TransferError::Cancelled),
            U3vError::Timeout
        ));
        let fault = transfer_error(0x82, TransferError::Fault);
        let U3vError::Usb(source) = &fault else {
            panic!("{fault:?}");
        };
        assert!(source.downcast_ref::<TransferError>().is_some());
        assert!(matches!(
            blocking_error(0x81, TransferError::Disconnected),
            U3vError::Disconnected
        ));
    }

    /// The codes an unplug on macOS gave are named, and left to the device
    /// list: on their own they are not a disconnect (UV-03).
    #[test]
    fn iokit_codes_are_named_and_not_conclusive() {
        let error = transfer_error(0x83, TransferError::Unknown(0xe000_02ed));
        assert_eq!(
            error.to_string(),
            "usb: kIOReturnNotResponding (error 0xe00002ed)"
        );
        assert!(error.may_mean_gone());
        let other = transfer_error(0x83, TransferError::Unknown(0x47));
        assert_eq!(other.to_string(), "usb: unknown (error 0x00000047)");
    }

    #[test]
    fn nusb_speeds_map_to_ours() {
        assert_eq!(speed(nusb::Speed::High), UsbSpeed::High);
        assert_eq!(speed(nusb::Speed::Super), UsbSpeed::Super);
        assert!(speed(nusb::Speed::High).is_below_superspeed());
        assert!(!speed(nusb::Speed::SuperPlus).is_below_superspeed());
    }
}

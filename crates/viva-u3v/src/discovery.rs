//! USB3 Vision device enumeration.
//!
//! Lists the USB devices with a USB3 Vision interface, from what the
//! operating system already knows about them: no device is opened, so a
//! scan neither disturbs nor depends on a camera another handle — this
//! process's own, say — holds (backlog UV-04).

#[cfg(feature = "usb")]
use crate::U3vError;
use crate::bootstrap::UsbSpeed;

/// Information about a discovered USB3 Vision device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct U3vDeviceInfo {
    /// The USB bus — the host controller — as the operating system names it.
    pub bus: String,
    /// USB device address on the bus. It changes when the device is plugged
    /// in again; the serial does not.
    pub address: u8,
    /// USB vendor ID.
    pub vendor_id: u16,
    /// USB product ID.
    pub product_id: u16,
    /// Serial number, from the USB string descriptor the operating system
    /// caches; `None` when the device reports none.
    pub serial: Option<String>,
    /// Manufacturer name, likewise. Windows caches none.
    pub manufacturer: Option<String>,
    /// Product/model name, likewise.
    pub model: Option<String>,
    /// The speed the device enumerated at, when the operating system says.
    pub speed: Option<UsbSpeed>,
}

impl std::fmt::Display for U3vDeviceInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "USB3V {:04x}:{:04x} bus={} addr={} {}",
            self.vendor_id,
            self.product_id,
            self.bus,
            self.address,
            self.model.as_deref().unwrap_or("(unknown)"),
        )
    }
}

/// Enumerate all USB3 Vision devices currently connected to the system.
///
/// A device is one when an interface — or, on Windows, the interface
/// association grouping them — has class `0xEF` and subclass `0x05`. Its
/// strings come from the operating system's cache; a device is opened only
/// when it is connected to, which warns of a camera below SuperSpeed.
#[cfg(feature = "usb")]
pub fn discover() -> Result<Vec<U3vDeviceInfo>, U3vError> {
    use nusb::MaybeFuture;

    let devices = nusb::list_devices()
        .wait()
        .map_err(U3vError::usb)?
        .filter(is_u3v)
        .map(|d| U3vDeviceInfo {
            bus: d.bus_id().to_string(),
            address: d.device_address(),
            vendor_id: d.vendor_id(),
            product_id: d.product_id(),
            serial: d.serial_number().map(str::to_string),
            manufacturer: d.manufacturer_string().map(str::to_string),
            model: d.product_string().map(str::to_string),
            speed: d.speed().map(crate::native::speed),
        })
        .collect();
    Ok(devices)
}

/// Whether `device` has a USB3 Vision interface (class `0xEF`, subclass
/// `0x05`). A device bound whole to one driver lists no interfaces on
/// Windows; it counts when it declares the interface association triple
/// (`0xEF`/`0x02`/`0x01`), and opening it settles whether it is one.
#[cfg(feature = "usb")]
fn is_u3v(device: &nusb::DeviceInfo) -> bool {
    use crate::descriptor::{U3V_INTERFACE_CLASS, U3V_INTERFACE_SUBCLASS};
    let mut interfaces = device.interfaces().peekable();
    if interfaces.peek().is_none() {
        return (device.class(), device.subclass(), device.protocol()) == (0xEF, 0x02, 0x01);
    }
    interfaces.any(|i| i.class() == U3V_INTERFACE_CLASS && i.subclass() == U3V_INTERFACE_SUBCLASS)
}

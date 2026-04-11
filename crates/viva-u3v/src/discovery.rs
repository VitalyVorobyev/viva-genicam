//! USB3 Vision device enumeration.
//!
//! Scans all connected USB devices for USB3 Vision interfaces and returns
//! descriptive information for each discovered camera.

#[cfg(feature = "usb")]
use crate::U3vError;
use crate::descriptor::U3vInterfaceInfo;

/// Information about a discovered USB3 Vision device.
#[derive(Debug, Clone)]
pub struct U3vDeviceInfo {
    /// USB bus number.
    pub bus: u8,
    /// USB device address on the bus.
    pub address: u8,
    /// USB vendor ID.
    pub vendor_id: u16,
    /// USB product ID.
    pub product_id: u16,
    /// Device serial number (from USB string descriptor, if available).
    pub serial: Option<String>,
    /// Manufacturer name (from USB string descriptor, if available).
    pub manufacturer: Option<String>,
    /// Product/model name (from USB string descriptor, if available).
    pub model: Option<String>,
    /// Parsed U3V interface and endpoint information.
    pub interface_info: U3vInterfaceInfo,
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
/// Returns an empty list if no U3V cameras are found. Devices that cannot
/// be opened or whose descriptors fail to parse are silently skipped.
#[cfg(feature = "usb")]
pub fn discover() -> Result<Vec<U3vDeviceInfo>, U3vError> {
    use crate::descriptor::{is_likely_u3v_device, parse_u3v_interfaces};
    use rusb::UsbContext;

    let context = rusb::Context::new().map_err(|e| U3vError::Usb(e.to_string()))?;
    let devices = context
        .devices()
        .map_err(|e| U3vError::Usb(e.to_string()))?;

    let mut found = Vec::new();

    for device in devices.iter() {
        let desc = match device.device_descriptor() {
            Ok(d) => d,
            Err(_) => continue,
        };

        if !is_likely_u3v_device(&desc) {
            continue;
        }

        let config = match device.active_config_descriptor() {
            Ok(c) => c,
            Err(_) => continue,
        };

        let interface_info = match parse_u3v_interfaces(&config) {
            Some(info) => info,
            None => continue,
        };

        // Try to read string descriptors (may fail if device is busy).
        let (serial, manufacturer, model) = match device.open() {
            Ok(handle) => {
                let timeout = std::time::Duration::from_millis(500);
                let serial = desc
                    .serial_number_string_index()
                    .and_then(|i| handle.read_string_descriptor_ascii(i).ok());
                let manufacturer = desc
                    .manufacturer_string_index()
                    .and_then(|i| handle.read_string_descriptor_ascii(i).ok());
                let model = desc
                    .product_string_index()
                    .and_then(|i| handle.read_string_descriptor_ascii(i).ok());
                let _ = timeout;
                (serial, manufacturer, model)
            }
            Err(_) => (None, None, None),
        };

        found.push(U3vDeviceInfo {
            bus: device.bus_number(),
            address: device.address(),
            vendor_id: desc.vendor_id(),
            product_id: desc.product_id(),
            serial,
            manufacturer,
            model,
            interface_info,
        });
    }

    Ok(found)
}

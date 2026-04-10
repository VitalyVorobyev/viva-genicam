//! USB descriptor parsing for USB3 Vision devices.
//!
//! USB3 Vision devices are identified by their Interface Association Descriptor
//! (IAD) class codes and interface class/subclass values. This module extracts
//! endpoint addresses and interface numbers from USB descriptors.

/// USB3 Vision interface class code.
pub const U3V_INTERFACE_CLASS: u8 = 0xEF;
/// USB3 Vision interface subclass for IAD.
pub const U3V_INTERFACE_SUBCLASS: u8 = 0x05;

/// U3V control interface subclass.
pub const U3V_CONTROL_SUBCLASS: u8 = 0x00;
/// U3V event interface subclass.
pub const U3V_EVENT_SUBCLASS: u8 = 0x01;
/// U3V streaming interface subclass.
pub const U3V_STREAM_SUBCLASS: u8 = 0x02;

/// U3V interface protocol code.
pub const U3V_INTERFACE_PROTOCOL: u8 = 0x00;

/// Information about a USB3 Vision device's interfaces and endpoints,
/// extracted from USB descriptors.
#[derive(Debug, Clone)]
pub struct U3vInterfaceInfo {
    /// Interface number for the control channel.
    pub control_iface: u8,
    /// Bulk IN endpoint for control acks.
    pub control_ep_in: u8,
    /// Bulk OUT endpoint for control commands.
    pub control_ep_out: u8,
    /// Interface number for the streaming channel (if present).
    pub stream_iface: Option<u8>,
    /// Bulk IN endpoint for stream data (if present).
    pub stream_ep_in: Option<u8>,
    /// Interface number for the event channel (if present).
    pub event_iface: Option<u8>,
    /// Bulk IN endpoint for events (if present).
    pub event_ep_in: Option<u8>,
}

/// Check whether a USB device's interface descriptors indicate a USB3 Vision device
/// and extract endpoint information.
///
/// Returns `None` if no U3V control interface is found.
#[cfg(feature = "usb")]
pub fn parse_u3v_interfaces(config: &rusb::ConfigDescriptor) -> Option<U3vInterfaceInfo> {
    let mut control_iface = None;
    let mut control_ep_in = None;
    let mut control_ep_out = None;
    let mut stream_iface = None;
    let mut stream_ep_in = None;
    let mut event_iface = None;
    let mut event_ep_in = None;

    for interface in config.interfaces() {
        for desc in interface.descriptors() {
            if desc.class_code() != U3V_INTERFACE_CLASS {
                continue;
            }

            match desc.sub_class_code() {
                U3V_CONTROL_SUBCLASS => {
                    control_iface = Some(desc.interface_number());
                    for ep in desc.endpoint_descriptors() {
                        if ep.transfer_type() != rusb::TransferType::Bulk {
                            continue;
                        }
                        match ep.direction() {
                            rusb::Direction::In => control_ep_in = Some(ep.address()),
                            rusb::Direction::Out => control_ep_out = Some(ep.address()),
                        }
                    }
                }
                U3V_EVENT_SUBCLASS => {
                    event_iface = Some(desc.interface_number());
                    for ep in desc.endpoint_descriptors() {
                        if ep.transfer_type() == rusb::TransferType::Bulk
                            && ep.direction() == rusb::Direction::In
                        {
                            event_ep_in = Some(ep.address());
                        }
                    }
                }
                U3V_STREAM_SUBCLASS => {
                    stream_iface = Some(desc.interface_number());
                    for ep in desc.endpoint_descriptors() {
                        if ep.transfer_type() == rusb::TransferType::Bulk
                            && ep.direction() == rusb::Direction::In
                        {
                            stream_ep_in = Some(ep.address());
                        }
                    }
                }
                _ => {}
            }
        }
    }

    let control_iface = control_iface?;
    let control_ep_in = control_ep_in?;
    let control_ep_out = control_ep_out?;

    Some(U3vInterfaceInfo {
        control_iface,
        control_ep_in,
        control_ep_out,
        stream_iface,
        stream_ep_in,
        event_iface,
        event_ep_in,
    })
}

/// Check whether a device descriptor looks like it could be a U3V device.
///
/// USB3 Vision uses IAD class `0xEF`, subclass `0x02`, protocol `0x01`
/// at the device level, but some cameras use `0x00` (per-interface).
/// This is a coarse filter; full detection requires inspecting interface
/// descriptors via [`parse_u3v_interfaces`].
#[cfg(feature = "usb")]
pub fn is_likely_u3v_device(desc: &rusb::DeviceDescriptor) -> bool {
    // IAD class at device level
    if desc.class_code() == 0xEF && desc.sub_class_code() == 0x02 && desc.protocol_code() == 0x01 {
        return true;
    }
    // Per-interface class: device descriptor says 0x00, need to check interfaces
    desc.class_code() == 0x00
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u3v_class_constants_are_correct() {
        assert_eq!(U3V_INTERFACE_CLASS, 0xEF);
        assert_eq!(U3V_CONTROL_SUBCLASS, 0x00);
        assert_eq!(U3V_EVENT_SUBCLASS, 0x01);
        assert_eq!(U3V_STREAM_SUBCLASS, 0x02);
        assert_eq!(U3V_INTERFACE_PROTOCOL, 0x00);
        assert_eq!(U3V_INTERFACE_SUBCLASS, 0x05);
    }
}

//! USB descriptor parsing for USB3 Vision devices.
//!
//! USB3 Vision devices are identified by their interfaces' class, subclass
//! and protocol. This module extracts endpoint addresses and interface
//! numbers from USB descriptors.

#[cfg(any(feature = "usb", test))]
/// USB3 Vision interface class code: Miscellaneous.
pub(crate) const U3V_INTERFACE_CLASS: u8 = 0xEF;
#[cfg(any(feature = "usb", test))]
/// USB3 Vision interface subclass, the same on all three of its interfaces.
pub(crate) const U3V_INTERFACE_SUBCLASS: u8 = 0x05;

#[cfg(any(feature = "usb", test))]
/// Interface protocol of the control interface.
pub(crate) const U3V_CONTROL_PROTOCOL: u8 = 0x00;
#[cfg(any(feature = "usb", test))]
/// Interface protocol of the event interface.
pub(crate) const U3V_EVENT_PROTOCOL: u8 = 0x01;
#[cfg(any(feature = "usb", test))]
/// Interface protocol of the streaming interface.
pub(crate) const U3V_STREAM_PROTOCOL: u8 = 0x02;

/// What a USB3 Vision interface carries.
#[cfg(any(feature = "usb", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum U3vInterfaceKind {
    /// The control interface: GenCP commands and acknowledges.
    Control,
    /// The event interface.
    Event,
    /// The streaming interface.
    Stream,
}

#[cfg(any(feature = "usb", test))]
/// Which USB3 Vision interface, if any, an interface descriptor describes.
///
/// All three are class `0xEF`, subclass `0x05`; the protocol tells them apart
/// (aravis `ARV_UV_INTERFACE_*_PROTOCOL`, Wireshark `IF_SUBCLASS_MISC_U3V`).
/// Until backlog TC-27 discovery matched the *subclass* against 0, 1 and 2,
/// so it found no interface on a conformant camera.
pub(crate) fn classify_interface(
    class: u8,
    subclass: u8,
    protocol: u8,
) -> Option<U3vInterfaceKind> {
    if class != U3V_INTERFACE_CLASS || subclass != U3V_INTERFACE_SUBCLASS {
        return None;
    }
    match protocol {
        U3V_CONTROL_PROTOCOL => Some(U3vInterfaceKind::Control),
        U3V_EVENT_PROTOCOL => Some(U3vInterfaceKind::Event),
        U3V_STREAM_PROTOCOL => Some(U3vInterfaceKind::Stream),
        _ => None,
    }
}

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

/// The USB3 Vision interfaces and endpoints in a device's active
/// configuration, and each bulk endpoint's maximum packet size; `None` if
/// it has no USB3 Vision control interface with both bulk endpoints.
#[cfg(feature = "usb")]
pub(crate) fn parse_u3v_interfaces(
    config: &nusb::descriptors::ConfigurationDescriptor<'_>,
) -> Option<(U3vInterfaceInfo, Vec<(u8, usize)>)> {
    use nusb::descriptors::TransferType;
    use nusb::transfer::Direction;

    let mut control_iface = None;
    let mut control_ep_in = None;
    let mut control_ep_out = None;
    let mut stream_iface = None;
    let mut stream_ep_in = None;
    let mut event_iface = None;
    let mut event_ep_in = None;
    let mut packet_sizes = Vec::new();

    for desc in config.interface_alt_settings() {
        let Some(kind) = classify_interface(desc.class(), desc.subclass(), desc.protocol()) else {
            continue;
        };
        let number = desc.interface_number();
        for ep in desc
            .endpoints()
            .filter(|ep| ep.transfer_type() == TransferType::Bulk)
        {
            packet_sizes.push((ep.address(), ep.max_packet_size()));
            match (kind, ep.direction()) {
                (U3vInterfaceKind::Control, Direction::In) => control_ep_in = Some(ep.address()),
                (U3vInterfaceKind::Control, Direction::Out) => control_ep_out = Some(ep.address()),
                (U3vInterfaceKind::Event, Direction::In) => event_ep_in = Some(ep.address()),
                (U3vInterfaceKind::Stream, Direction::In) => stream_ep_in = Some(ep.address()),
                _ => {}
            }
        }
        match kind {
            U3vInterfaceKind::Control => control_iface = Some(number),
            U3vInterfaceKind::Event => event_iface = Some(number),
            U3vInterfaceKind::Stream => stream_iface = Some(number),
        }
    }

    Some((
        U3vInterfaceInfo {
            control_iface: control_iface?,
            control_ep_in: control_ep_in?,
            control_ep_out: control_ep_out?,
            stream_iface,
            stream_ep_in,
            event_iface,
            event_ep_in,
        },
        packet_sizes,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interfaces_are_told_apart_by_protocol() {
        assert_eq!(
            classify_interface(0xEF, 0x05, 0x00),
            Some(U3vInterfaceKind::Control)
        );
        assert_eq!(
            classify_interface(0xEF, 0x05, 0x01),
            Some(U3vInterfaceKind::Event)
        );
        assert_eq!(
            classify_interface(0xEF, 0x05, 0x02),
            Some(U3vInterfaceKind::Stream)
        );
        // What discovery used to look for: subclass 0, 1, 2.
        assert_eq!(classify_interface(0xEF, 0x00, 0x00), None);
        assert_eq!(classify_interface(0xEF, 0x02, 0x00), None);
        assert_eq!(classify_interface(0xFF, 0x05, 0x00), None);
    }
}

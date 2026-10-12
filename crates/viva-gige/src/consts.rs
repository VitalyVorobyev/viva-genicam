//! GigE Vision's vocabulary: GVCP opcodes and header fields, bootstrap
//! register addresses and bits, and the protocol's limits.
//!
//! Public as a namespace on purpose (ADR-0030): the fake camera imports these
//! rather than keeping copies. A constant is vocabulary; what a register
//! means, and how a packet is encoded, each side still derives from the
//! specification on its own.

use std::time::Duration;

/// The key byte every GVCP command starts with.
pub const GVCP_CMD_KEY: u8 = 0x42;
/// Size of a GVCP command or acknowledgement header.
pub const GVCP_HEADER_SIZE: usize = 8;
/// Command flag bit 0: the sender wants an acknowledgement.
pub const FLAG_ACK_REQUIRED: u8 = 0x01;
/// Command flag bit 4 on an event command: 64-bit block ids (GigE Vision 2.0).
pub const FLAG_EXTENDED_IDS: u8 = 0x10;

/// GVCP control port as defined by the GigE Vision specification (section 7.3).
pub const PORT: u16 = 3956;
/// Opcode of the discovery command.
pub const DISCOVERY_COMMAND: u16 = 0x0002;
/// Opcode of the discovery acknowledgement.
pub const DISCOVERY_ACK: u16 = 0x0003;
/// Opcode of the FORCEIP command.
pub const FORCEIP_COMMAND: u16 = 0x0004;
/// Opcode of the FORCEIP acknowledgement.
pub const FORCEIP_ACK: u16 = 0x0005;
/// Opcode of `PACKETRESEND_CMD`, which asks for no acknowledgement: the
/// answer is the packets themselves, on the stream channel. See
/// [`encode_packet_resend`](crate::encode_packet_resend).
pub const PACKET_RESEND_COMMAND: u16 = 0x0040;
/// Flag-byte bit 4 on `PACKETRESEND_CMD`: the payload carries a 64-bit
/// block id and 32-bit packet ids (GigE Vision 2.0 extended ids). The same
/// bit means "allow broadcast acknowledge" on `DISCOVERY_CMD`.
pub const PACKET_RESEND_EXTENDED_IDS: u8 = 0x10;
/// Opcode of the PENDING_ACK acknowledgement (GigE Vision 1.2, section 18.5).
///
/// A device that cannot complete a command within the controller's timeout
/// answers with this instead of the real acknowledgement, asking for more
/// time. GenCP models the same mechanism as its own command id `0x0805`
/// (`viva_gencp::PENDING_ACK_COMMAND`), never as a status (ADR-0020).
pub const PENDING_ACK: u16 = 0x0089;

// ── Event channel and action commands ───────────────────────────────
//
// These four opcodes are the ones a device sends *to* us on the message
// channel, plus the two we broadcast for actions. They live here rather
// than in `action`/`message` because keeping every GVCP opcode in one
// table is what makes a collision visible: `ACTION_COMMAND` was 0x0080 —
// `READ_REGISTER` — for as long as it had its own private constant.
// GigE Vision 2.0 section 18; corroborated by Wireshark's `packet-gvcp.c`
// (`GVCP_ACTION_CMD`, `GVCP_EVENT_CMD`, `GVCP_EVENTDATA_CMD`) and, for the
// register commands it shadowed, `../aravis/src/arvgvcpprivate.h`.

/// Opcode of the event command sent by a device on the message channel.
pub const EVENT_COMMAND: u16 = 0x00C0;
/// Opcode of the acknowledgement a controller returns for an `EVENT_CMD`.
pub const EVENT_ACK: u16 = 0x00C1;
/// Opcode of the event command that carries device-specific event data.
pub const EVENTDATA_COMMAND: u16 = 0x00C2;
/// Opcode of the acknowledgement returned for an `EVENTDATA_CMD`.
pub const EVENTDATA_ACK: u16 = 0x00C3;
/// Opcode of the action command.
pub const ACTION_COMMAND: u16 = 0x0100;
/// Opcode of the action acknowledgement.
pub const ACTION_ACK: u16 = 0x0101;

/// Size of one event entry in an `EVENT_CMD` with 16-bit block IDs.
pub const EVENT_ENTRY: usize = 16;
/// Size of one event entry in an `EVENT_CMD` with 64-bit block IDs.
///
/// GigE Vision 2.0 extended block IDs, signalled by bit 4 of the GVCP
/// flags byte.
pub const EVENT_ENTRY_EXTENDED: usize = 24;

/// Current IP configuration flags register.
///
/// Bit 2 = DHCP, bit 1 = persistent IP, bit 0 = LLA.
pub const CURRENT_IP_CONFIG: u64 = 0x0014;

/// Persistent IP address register (4 bytes at the end of a 16-byte block).
pub const PERSISTENT_IP_ADDRESS: u64 = 0x064C;
/// Persistent subnet mask register.
pub const PERSISTENT_SUBNET_MASK: u64 = 0x065C;
/// Persistent default gateway register.
pub const PERSISTENT_DEFAULT_GATEWAY: u64 = 0x066C;

/// Address of the Control Channel Privilege (CCP) register.
///
/// A controller must write `CONTROL_PRIVILEGE` to this register before the
/// device accepts stream configuration or acquisition commands.
pub const CONTROL_CHANNEL_PRIVILEGE: u64 = 0x0a00;
/// CCP value claiming exclusive control.
pub const CCP_CONTROL: u32 = 1 << 1;
/// CCP value indicating an exclusive owner.
pub const CCP_EXCLUSIVE: u32 = 1 << 0;
/// Bits of the CCP register that mean "this application is the controller".
pub const CCP_CONTROLLER_BITS: u32 = CCP_CONTROL | CCP_EXCLUSIVE;
/// Primary Application Port: the UDP port of the application holding
/// control, in the low half, where GVCP Capability bit `0x0800_0000` says
/// the device has it. Wireshark: `GVCP_PRIMARY_APPLICATION_PORT`.
pub const PRIMARY_APPLICATION_PORT: u64 = 0x0a04;
/// Primary Application IP address. Wireshark:
/// `GVCP_PRIMARY_APPLICATION_IP_ADDRESS`.
pub const PRIMARY_APPLICATION_IP: u64 = 0x0a14;

/// Heartbeat timeout register (`GevHeartbeatTimeout`), in milliseconds.
///
/// A device that has granted control privilege releases it again if it
/// receives no GVCP command from the controlling application within this
/// window. GVSP image traffic does not count — a stream can be running at
/// full rate while the control channel times out underneath it.
pub const HEARTBEAT_TIMEOUT: u64 = 0x0938;

/// Number of stream channels the device implements
/// (`GevNumberOfStreamChannels`). Wireshark: `GVCP_NUMBER_OF_STREAM_CHANNELS`.
pub const NUMBER_OF_STREAM_CHANNELS: u64 = 0x0904;
/// GVCP Capability: one bit per optional command or feature. Wireshark:
/// `GVCP_CAPABILITY`.
pub const GVCP_CAPABILITY: u64 = 0x0934;
/// Capability bit 29 — counting from the most significant bit as bit 0,
/// as the specification does — `PACKETRESEND_CMD` supported. Wireshark's
/// `hf_gvcp_capability_packetresend` masks `0x00000004`; aravis reads the
/// same bit (`ARV_GVBS_GVCP_CAPABILITY_PACKET_RESEND`, `1 << 2`).
pub const CAPABILITY_PACKET_RESEND: u32 = 0x0000_0004;
/// Capability bit 26 — `PENDING_ACK` supported. Wireshark:
/// `hf_gvcp_capability_pending`, mask `0x00000020`.
pub const CAPABILITY_PENDING_ACK: u32 = 0x0000_0020;
/// GVCP Configuration (`GevGVCPConfiguration`): optional behaviour the
/// controller switches on. Wireshark: `GVCP_CONFIGURATION`.
pub const GVCP_CONFIGURATION: u64 = 0x0954;
/// Configuration bit 30 — `PENDING_ACK` enabled; a device sends one only
/// once this is set. Wireshark: `hf_gvcp_configuration_pending_ack_enable`,
/// mask `0x00000002`.
pub const CONFIGURATION_PENDING_ACK_ENABLE: u32 = 0x0000_0002;
/// Timestamp tick frequency, high 32 bits (`GevTimestampTickFrequencyHigh`).
pub const TIMESTAMP_TICK_FREQUENCY_HIGH: u64 = 0x093C;
/// Timestamp tick frequency, low 32 bits (`GevTimestampTickFrequencyLow`).
/// `GevSCPD` counts in these ticks.
pub const TIMESTAMP_TICK_FREQUENCY_LOW: u64 = 0x0940;

// ── Message channel bootstrap registers ─────────────────────────────
//
// These sit in the 0x0B00 block, next to CCP at 0x0A00 and the stream
// channels at 0x0D00. They previously read 0x0900_0200 / 0x0900_0204,
// which is not a bootstrap address at all: 0x0900 is
// `GevNumberOfMessageChannels`, and the low half was being used as if it
// were a base. Every event-channel write therefore landed ~150 MB into
// the device's register space. Wireshark's `packet-gvcp.c`
// (`GVCP_MC_DESTINATION_PORT`, `GVCP_MC_DESTINATION_ADDRESS`) gives the
// real values, and the CCP and stream-channel addresses we already had
// right corroborate the scheme.

/// Number of message channels the device implements (`GevNumberOfMessageChannels`).
pub const NUMBER_OF_MESSAGE_CHANNELS: u64 = 0x0000_0900;
/// Message channel destination port register (`GevMCP`).
///
/// A 32-bit register; the UDP port occupies the low 16 bits.
pub const MESSAGE_DESTINATION_PORT: u64 = 0x0000_0B00;
/// Message channel destination address register (`GevMCDA`).
pub const MESSAGE_DESTINATION_ADDRESS: u64 = 0x0000_0B10;
/// Message channel transmission timeout in milliseconds (`GevMCTT`).
pub const MESSAGE_CHANNEL_TIMEOUT: u64 = 0x0000_0B14;
/// Message channel retry count (`GevMCRC`).
pub const MESSAGE_CHANNEL_RETRY_COUNT: u64 = 0x0000_0B18;

/// Maximum number of bytes we read per GenCP `ReadMem` operation.
pub const GENCP_MAX_BLOCK: usize = 512;
/// Additional bytes that accompany a GenCP `WriteMem` block.
pub const GENCP_WRITE_OVERHEAD: usize = 8;

/// Default timeout for control transactions.
pub const CONTROL_TIMEOUT: Duration = Duration::from_millis(500);
/// Maximum number of automatic retries for a control transaction.
pub const MAX_RETRIES: usize = 4;
/// Maximum number of consecutive PENDING_ACKs honoured for one command.
///
/// A device is free to keep asking for more time; this bounds a
/// misbehaving one that never finishes.
pub const MAX_PENDING_ACKS: usize = 100;
/// Ceiling on the extension a single PENDING_ACK may request.
///
/// The field is 16-bit milliseconds, so the wire maximum is ~65 s. Cap it
/// well below that: an honest device asks for tens or hundreds of
/// milliseconds, and this keeps a garbage value from hanging the caller.
pub const MAX_PENDING_ACK_WAIT: Duration = Duration::from_secs(10);
/// Base delay used for retry backoff.
pub const RETRY_BASE_DELAY: Duration = Duration::from_millis(20);
/// Upper bound for the random jitter added to the retry delay (inclusive).
pub const RETRY_JITTER: Duration = Duration::from_millis(10);

/// Maximum number of bytes captured while listening for discovery responses.
pub const DISCOVERY_BUFFER: usize = 2048;

/// Base register for stream channel 0 (GigE Vision bootstrap register map).
///
/// The GigE Vision specification defines stream channel bootstrap registers
/// starting at 0x0d00. Note: some cameras may use different offsets declared
/// in their GenICam XML (e.g. SFNC `GevSCDA` nodes). The bootstrap offsets
/// here match the aravis implementation and the GigE Vision 2.x standard.
pub const STREAM_CHANNEL_BASE: u64 = 0x0d00;
/// Stride in bytes between successive stream channel blocks.
pub const STREAM_CHANNEL_STRIDE: u64 = 0x40;
/// Offset for `GevSCPHostPort` within a stream channel block.
pub const STREAM_DESTINATION_PORT: u64 = 0x00;
/// Offset for `GevSCPSPacketSize` within a stream channel block.
pub const STREAM_PACKET_SIZE: u64 = 0x04;
/// Offset for `GevSCPD` (packet delay) within a stream channel block.
pub const STREAM_PACKET_DELAY: u64 = 0x08;
/// Offset for `GevSCDA` (stream destination IP address) within a stream channel block.
pub const STREAM_DESTINATION_ADDRESS: u64 = 0x18;

// ── Bootstrap registers (GigE Vision 2.x, the bootstrap register map) ────

/// `Version`: the GigE Vision version the device implements.
pub const VERSION: u64 = 0x0000;
/// `Device Mode`: endianness, character set, device class.
pub const DEVICE_MODE: u64 = 0x0004;
/// `Device MAC Address – High`: the MAC's top two bytes, right-aligned.
pub const DEVICE_MAC_ADDRESS_HIGH: u64 = 0x0008;
/// `Device MAC Address – Low`: the MAC's bottom four bytes.
pub const DEVICE_MAC_ADDRESS_LOW: u64 = 0x000C;
/// `Supported IP Configuration`: LLA, DHCP, persistent IP.
pub const SUPPORTED_IP_CONFIG: u64 = 0x0010;
/// `Current IP Address`.
pub const CURRENT_IP_ADDRESS: u64 = 0x0024;
/// `Current Subnet Mask`.
pub const CURRENT_SUBNET_MASK: u64 = 0x0034;
/// `Current Default Gateway`.
pub const CURRENT_DEFAULT_GATEWAY: u64 = 0x0044;
/// `First URL`: where the device's GenApi XML is (512 bytes, a string).
pub const FIRST_URL: u64 = 0x0200;
/// `Second URL`: an alternative to [`FIRST_URL`].
pub const SECOND_URL: u64 = 0x0400;
/// Length of [`FIRST_URL`] and [`SECOND_URL`] in bytes.
pub const URL_LEN: usize = 512;
/// `GVCP Capability` bit 31: several registers in one `READREG`/`WRITEREG`.
pub const CAPABILITY_CONCATENATION: u32 = 0x0000_0001;
/// `GVCP Capability` bit 30: `WRITEMEM` is supported.
pub const CAPABILITY_WRITEMEM: u32 = 0x0000_0002;
/// `GVCP Capability` bit 4: the primary application's port and address are
/// readable at [`PRIMARY_APPLICATION_PORT`] and [`PRIMARY_APPLICATION_IP`].
/// Mask from Wireshark's `hf_gvcp_capability_ccp_application_portip`.
pub const CAPABILITY_CCP_APPLICATION_PORT_IP: u32 = 0x0800_0000;

/// The bits of `GevSCPSPacketSize` that hold the packet size.
///
/// Bit 31 fires a test packet and bit 30 sets do-not-fragment; bits 29-16 are
/// reserved. Masking matters on the read side as much as the write side — a
/// device that leaves the do-not-fragment bit set would otherwise read back as
/// a packet size of over a billion.
pub const STREAM_PACKET_SIZE_MASK: u32 = 0xFFFF;

/// `GevSCPSPacketSize` bit 31: emit one test packet of the requested size.
///
/// The only way to discover that a size *both endpoints accept* cannot cross
/// the link between them — a register read reports what the device stored, not
/// what the network will carry
/// ([#112](https://github.com/VitalyVorobyev/viva-genicam/issues/112)).
pub const SCPS_FIRE_TEST_PACKET: u32 = 0x8000_0000;

/// `GevSCPSPacketSize` bit 30: set do-not-fragment on transmitted packets.
///
/// Paired with the test packet so a path that would *fragment* the datagram
/// drops it instead. Without it a probe can succeed on a link that then
/// delivers reassembled fragments, which is slower than the smaller size it
/// talked us out of.
pub const SCPS_DO_NOT_FRAGMENT: u32 = 0x4000_0000;

#[cfg(test)]
mod tests {
    use super::*;

    /// The values, written out as literals from the GigE Vision bootstrap
    /// register map and opcode table rather than from these constants
    /// (ADR-0019), corroborated by aravis's `arvgvcpprivate.h`
    /// (`ARV_GVBS_*_OFFSET`, `ArvGvcpCommand`). The fake cameras import the
    /// names; this table is what keeps a wrong value from passing in both.
    #[test]
    fn the_vocabulary_matches_the_specification() {
        let addresses: &[(&str, u64, u64)] = &[
            ("Version", VERSION, 0x0000),
            ("Device Mode", DEVICE_MODE, 0x0004),
            ("Device MAC Address High", DEVICE_MAC_ADDRESS_HIGH, 0x0008),
            ("Device MAC Address Low", DEVICE_MAC_ADDRESS_LOW, 0x000C),
            ("Supported IP Configuration", SUPPORTED_IP_CONFIG, 0x0010),
            ("Current IP Configuration", CURRENT_IP_CONFIG, 0x0014),
            ("Current IP Address", CURRENT_IP_ADDRESS, 0x0024),
            ("Current Subnet Mask", CURRENT_SUBNET_MASK, 0x0034),
            ("Current Default Gateway", CURRENT_DEFAULT_GATEWAY, 0x0044),
            ("First URL", FIRST_URL, 0x0200),
            ("Second URL", SECOND_URL, 0x0400),
            ("Persistent IP Address", PERSISTENT_IP_ADDRESS, 0x064C),
            ("Persistent Subnet Mask", PERSISTENT_SUBNET_MASK, 0x065C),
            (
                "Persistent Default Gateway",
                PERSISTENT_DEFAULT_GATEWAY,
                0x066C,
            ),
            (
                "Number of Message Channels",
                NUMBER_OF_MESSAGE_CHANNELS,
                0x0900,
            ),
            (
                "Number of Stream Channels",
                NUMBER_OF_STREAM_CHANNELS,
                0x0904,
            ),
            ("GVCP Capability", GVCP_CAPABILITY, 0x0934),
            ("Heartbeat Timeout", HEARTBEAT_TIMEOUT, 0x0938),
            (
                "Timestamp Tick Frequency High",
                TIMESTAMP_TICK_FREQUENCY_HIGH,
                0x093C,
            ),
            (
                "Timestamp Tick Frequency Low",
                TIMESTAMP_TICK_FREQUENCY_LOW,
                0x0940,
            ),
            ("GVCP Configuration", GVCP_CONFIGURATION, 0x0954),
            (
                "Control Channel Privilege",
                CONTROL_CHANNEL_PRIVILEGE,
                0x0A00,
            ),
            ("Primary Application Port", PRIMARY_APPLICATION_PORT, 0x0A04),
            ("Primary Application IP", PRIMARY_APPLICATION_IP, 0x0A14),
            ("Message Channel Port", MESSAGE_DESTINATION_PORT, 0x0B00),
            (
                "Message Channel Destination",
                MESSAGE_DESTINATION_ADDRESS,
                0x0B10,
            ),
            (
                "Stream Channel 0 Port",
                STREAM_CHANNEL_BASE + STREAM_DESTINATION_PORT,
                0x0D00,
            ),
            (
                "Stream Channel 0 Packet Size",
                STREAM_CHANNEL_BASE + STREAM_PACKET_SIZE,
                0x0D04,
            ),
            (
                "Stream Channel 0 Packet Delay",
                STREAM_CHANNEL_BASE + STREAM_PACKET_DELAY,
                0x0D08,
            ),
            (
                "Stream Channel 0 Destination",
                STREAM_CHANNEL_BASE + STREAM_DESTINATION_ADDRESS,
                0x0D18,
            ),
        ];
        for &(name, actual, spec) in addresses {
            assert_eq!(actual, spec, "{name}");
        }
        let opcodes: &[(&str, u16, u16)] = &[
            ("DISCOVERY_CMD", DISCOVERY_COMMAND, 0x0002),
            ("DISCOVERY_ACK", DISCOVERY_ACK, 0x0003),
            ("FORCEIP_CMD", FORCEIP_COMMAND, 0x0004),
            ("FORCEIP_ACK", FORCEIP_ACK, 0x0005),
            ("PACKETRESEND_CMD", PACKET_RESEND_COMMAND, 0x0040),
            ("PENDING_ACK", PENDING_ACK, 0x0089),
            ("EVENT_CMD", EVENT_COMMAND, 0x00C0),
            ("EVENT_ACK", EVENT_ACK, 0x00C1),
            ("EVENTDATA_CMD", EVENTDATA_COMMAND, 0x00C2),
            ("EVENTDATA_ACK", EVENTDATA_ACK, 0x00C3),
            ("ACTION_CMD", ACTION_COMMAND, 0x0100),
            ("ACTION_ACK", ACTION_ACK, 0x0101),
        ];
        for &(name, actual, spec) in opcodes {
            assert_eq!(actual, spec, "{name}");
        }
        assert_eq!(GVCP_CMD_KEY, 0x42);
        assert_eq!((CCP_EXCLUSIVE, CCP_CONTROL), (1 << 0, 1 << 1));
        assert_eq!(
            (
                CAPABILITY_CONCATENATION,
                CAPABILITY_WRITEMEM,
                CAPABILITY_PACKET_RESEND
            ),
            (1 << 0, 1 << 1, 1 << 2)
        );
        assert_eq!(CAPABILITY_PENDING_ACK, 1 << 5);
        assert_eq!(
            (SCPS_FIRE_TEST_PACKET, SCPS_DO_NOT_FRAGMENT),
            (1 << 31, 1 << 30)
        );
    }
}

//! GVCP control plane utilities.

use std::collections::HashMap;
use std::io::Cursor;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use bytes::{Buf, BufMut, Bytes, BytesMut};
use fastrand::Rng;
use if_addrs::{IfAddr, get_if_addrs};
use thiserror::Error;
use tokio::net::UdpSocket;
use tokio::task::JoinSet;
use tokio::time;
use tracing::{debug, info, trace, warn};
use viva_gencp::{AckHeader, CommandFlags, GenCpAck, OpCode, StatusCode, decode_ack};

use crate::consts::{
    self, GVCP_CMD_KEY, SCPS_DO_NOT_FRAGMENT, SCPS_FIRE_TEST_PACKET, STREAM_PACKET_SIZE_MASK,
};
use crate::nic::Iface;
use crate::status::GigeStatus;

/// Public alias for the GVCP well-known port.
pub use consts::PORT as GVCP_PORT;

/// Encode a `PACKETRESEND_CMD` asking the device to send packets
/// `first_packet..=last_packet` of block `block_id` on stream channel
/// `channel` again.
///
/// The command asks for **no acknowledgement**: the answer is the packets
/// themselves, on the stream channel. With 16-bit block ids the payload is
/// 12 bytes; with extended ids (`extended_ids`, flag `0x10`) it is 20:
///
/// | Bytes | Standard ids                   | Extended ids                |
/// |-------|--------------------------------|-----------------------------|
/// | 0–1   | stream channel index           | stream channel index        |
/// | 2–3   | block id (16 bits)             | reserved, 0                 |
/// | 4–7   | first packet id (24 bits)      | first packet id (32 bits)   |
/// | 8–11  | last packet id (24 bits)       | last packet id (32 bits)    |
/// | 12–19 | —                              | block id (64 bits)          |
///
/// The layout is the field table Wireshark's `packet-gvcp.c` dissects
/// (`dissect_packetresend_cmd`, which reads the standard packet ids as the
/// low 24 bits of each word). aravis builds the same bytes for channel 0
/// (`arv_gvcp_packet_new_packet_resend_cmd`) — corroboration, not the rule.
/// With standard ids the block id is truncated to its 16 bits and the packet
/// ids to their 24, which is all those fields hold.
pub fn encode_packet_resend(
    request_id: u16,
    channel: u16,
    block_id: u64,
    first_packet: u32,
    last_packet: u32,
    extended_ids: bool,
) -> Bytes {
    const PACKET_ID_24: u32 = 0x00FF_FFFF;
    let length: u16 = if extended_ids { 20 } else { 12 };
    let mut buf = BytesMut::with_capacity(viva_gencp::HEADER_SIZE + length as usize);
    buf.put_u8(GVCP_CMD_KEY);
    buf.put_u8(if extended_ids {
        consts::PACKET_RESEND_EXTENDED_IDS
    } else {
        0
    });
    buf.put_u16(consts::PACKET_RESEND_COMMAND);
    buf.put_u16(length);
    buf.put_u16(request_id);
    buf.put_u16(channel);
    if extended_ids {
        buf.put_u16(0);
        buf.put_u32(first_packet);
        buf.put_u32(last_packet);
        buf.put_u64(block_id);
    } else {
        buf.put_u16(block_id as u16);
        buf.put_u32(first_packet & PACKET_ID_24);
        buf.put_u32(last_packet & PACKET_ID_24);
    }
    buf.freeze()
}

/// GVCP request header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GvcpRequestHeader {
    /// Request flags (acknowledgement, broadcast).
    pub flags: CommandFlags,
    /// Raw command/opcode value.
    pub command: u16,
    /// Payload length in bytes.
    pub length: u16,
    /// Request identifier.
    pub request_id: u16,
}

impl GvcpRequestHeader {
    /// Encode the header into a `Bytes` buffer ready to be transmitted.
    ///
    /// Uses proper GVCP wire format: byte 0 = `0x42` (command key),
    /// byte 1 = flags byte (bit 0 = ACK_REQUIRED, bit 4 = BROADCAST).
    pub(crate) fn encode(self, payload: &[u8]) -> Bytes {
        let mut buf = BytesMut::with_capacity(viva_gencp::HEADER_SIZE + payload.len());
        // GVCP command header: key byte + flags byte (not a u16 flags field).
        buf.put_u8(GVCP_CMD_KEY);
        buf.put_u8(self.gvcp_flags_byte());
        buf.put_u16(self.command);
        buf.put_u16(self.length);
        buf.put_u16(self.request_id);
        buf.extend_from_slice(payload);
        buf.freeze()
    }

    /// Convert `CommandFlags` to the single-byte GVCP flag field.
    ///
    /// Bit 4 is overloaded by the specification: it means "allow broadcast
    /// acknowledge" on `DISCOVERY_CMD` and "64-bit block IDs" on
    /// `PACKETRESEND_CMD`/`EVENT_CMD`/`EVENTDATA_CMD`. This header sets it only
    /// as the former; [`encode_packet_resend`] writes its own flag byte.
    fn gvcp_flags_byte(&self) -> u8 {
        let mut byte = 0u8;
        if self.flags.contains(CommandFlags::ACK_REQUIRED) {
            byte |= 0x01;
        }
        if self.flags.contains(CommandFlags::BROADCAST) {
            byte |= 0x10;
        }
        if self.flags.contains(CommandFlags::SCHEDULED_ACTION) {
            byte |= 0x80;
        }
        byte
    }
}

/// GVCP acknowledgement header wrapper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GvcpAckHeader {
    /// Status reported by the device.
    pub status: GigeStatus,
    /// Raw command/opcode value.
    pub command: u16,
    /// Payload length in bytes.
    pub length: u16,
    /// Identifier of the answered request.
    pub request_id: u16,
}

impl From<AckHeader> for GvcpAckHeader {
    fn from(value: AckHeader) -> Self {
        Self {
            status: GigeStatus::from_raw(value.status),
            command: value.opcode.ack_code(),
            length: value.length,
            request_id: value.request_id,
        }
    }
}

/// Errors that can occur when operating the GVCP control path.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum GigeError {
    /// A socket operation failed.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// No acknowledgement arrived within the control timeout, retries
    /// included.
    #[error("timeout waiting for acknowledgement")]
    Timeout,
    /// An acknowledgement's header could not be decoded.
    #[error("GenCP: {0}")]
    GenCp(#[from] viva_gencp::GenCpError),
    /// The device answered with an error status, decoded with GigE Vision's
    /// table (ADR-0020).
    #[error("device reported status {0}")]
    Status(GigeStatus),
    /// An acknowledgement does not have the shape GVCP gives it.
    #[error("malformed {command} acknowledgement: {problem}")]
    MalformedAck {
        /// The command it answers, such as `"READREG"`.
        command: &'static str,
        /// What is wrong, such as `"answers another command"`.
        problem: &'static str,
    },
    /// An acknowledgement carries another payload length than its command
    /// asked for.
    #[error("{command} acknowledgement carries {got} bytes; expected {expected}")]
    AckLength {
        /// The command it answers.
        command: &'static str,
        /// The bytes asked for.
        expected: usize,
        /// The bytes it carries.
        got: usize,
    },
    /// GigE Vision control runs over IPv4, and the address or interface
    /// given has none.
    #[error("GigE Vision control runs over IPv4, and the {0} has no IPv4 address")]
    NotIpv4(&'static str),
    /// A GVSP packet size wider than `GevSCPSPacketSize`'s 16-bit field.
    #[error("GVSP packet size {0} does not fit GevSCPSPacketSize's 16 bits")]
    PacketSize(u32),
}

/// Information returned by GVCP discovery packets.
///
/// Build one with [`DeviceInfo::from_ip`] and the fields you have
/// (`DeviceInfo { model, ..DeviceInfo::from_ip(ip) }`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// The camera's current IPv4 address.
    pub ip: Ipv4Addr,
    /// The camera's MAC address.
    pub mac: [u8; 6],
    /// Model name (Discovery ACK offset 104).
    pub model: Option<String>,
    /// Manufacturer name (Discovery ACK offset 72).
    pub manufacturer: Option<String>,
    /// Device version string (Discovery ACK offset 136).
    pub version: Option<String>,
    /// Serial number as printed on the device (Discovery ACK offset 216).
    pub serial: Option<String>,
    /// User-programmable device name (Discovery ACK offset 232).
    pub user_name: Option<String>,
    /// The host address the Discovery ACK arrived at: the local IPv4 of the
    /// discovery socket, which names both the interface and, on a NIC with
    /// several addresses, which of them reached the device. `None` for a
    /// record discovery did not make. A broadcast meant for this device, such
    /// as [`force_ip`], has to leave from it (`Iface::from_ipv4`).
    pub local_ip: Option<Ipv4Addr>,
    /// The camera's current subnet mask (Discovery ACK offset 52).
    pub subnet: Option<Ipv4Addr>,
    /// The camera's current default gateway (Discovery ACK offset 68).
    pub gateway: Option<Ipv4Addr>,
    /// The netmask of the host address in [`local_ip`](Self::local_ip).
    pub local_netmask: Option<Ipv4Addr>,
}

impl DeviceInfo {
    /// A minimal record for a device addressed directly by IP.
    ///
    /// Used when the caller names a camera by address and there is no
    /// Discovery ACK to populate the identity fields.
    pub fn from_ip(ip: Ipv4Addr) -> Self {
        Self {
            ip,
            mac: [0; 6],
            model: None,
            manufacturer: None,
            version: None,
            serial: None,
            user_name: None,
            local_ip: None,
            subnet: None,
            gateway: None,
            local_netmask: None,
        }
    }

    /// Whether the camera and the host address that heard it are on one
    /// subnet, by both masks: the camera's address inside the host's network,
    /// and the host's inside the camera's. `None` when either side's address
    /// or mask is unknown.
    ///
    /// A camera that answers discovery but fails this is configured for
    /// another network. It answered a broadcast, but cannot be reached by
    /// unicast — every connection to it fails — until its address is changed
    /// (`force_ip`) or the host joins its network.
    pub fn shares_subnet_with_host(&self) -> Option<bool> {
        let (camera, camera_mask) = (u32::from(self.ip), u32::from(self.subnet?));
        let host = u32::from(self.local_ip?);
        let host_mask = u32::from(self.local_netmask?);
        Some(camera & host_mask == host & host_mask && host & camera_mask == camera & camera_mask)
    }

    /// Format the MAC address as `AA:BB:CC:DD:EE:FF`.
    pub fn mac_string(&self) -> String {
        self.mac
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<Vec<_>>()
            .join(":")
    }
}

/// How [`discover_with`] looks for cameras.
///
/// The default: every IPv4 interface but loopback, and acknowledgements
/// unicast to the host.
#[derive(Debug, Clone, Default)]
pub struct DiscoveryOptions {
    interface: Option<String>,
    include_loopback: bool,
    allow_broadcast_ack: bool,
}

impl DiscoveryOptions {
    /// The defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask on this interface only, by its OS name. A named interface is
    /// asked even when it is loopback.
    pub fn interface(mut self, name: impl Into<String>) -> Self {
        self.interface = Some(name.into());
        self
    }

    /// Ask on loopback too, for simulated cameras bound to `127.0.0.1`.
    pub fn include_loopback(mut self, include: bool) -> Self {
        self.include_loopback = include;
        self
    }

    /// Set the command's *allow broadcast acknowledge* flag (`0x10`): a
    /// camera that cannot unicast its answer to the host — one configured
    /// for another subnet, typically — may broadcast it instead (default:
    /// off).
    ///
    /// Off, such a camera may stay silent. On, every listener on the subnet
    /// receives its answer, which is why it is not the default.
    pub fn allow_broadcast_ack(mut self, allow: bool) -> Self {
        self.allow_broadcast_ack = allow;
        self
    }
}

/// Discover GigE Vision devices on the local network by broadcasting a GVCP discovery command.
pub async fn discover(timeout: Duration) -> Result<Vec<DeviceInfo>, GigeError> {
    discover_with(timeout, &DiscoveryOptions::new()).await
}

/// Discover devices only on the specified interface name.
///
/// When a user explicitly names an interface (including loopback like `lo0`),
/// it is always included — the loopback filter only applies to the unfiltered
/// [`discover`] call.
pub async fn discover_on_interface(
    timeout: Duration,
    interface: &str,
) -> Result<Vec<DeviceInfo>, GigeError> {
    discover_with(timeout, &DiscoveryOptions::new().interface(interface)).await
}

/// Discover devices on all interfaces including loopback.
///
/// This is useful for testing with simulated cameras (e.g. `arv-fake-gv-camera`)
/// bound to `127.0.0.1`.
pub async fn discover_all(timeout: Duration) -> Result<Vec<DeviceInfo>, GigeError> {
    discover_with(timeout, &DiscoveryOptions::new().include_loopback(true)).await
}

/// Discover devices as `options` say.
///
/// Each interface asks from a socket bound to its own address, sending to
/// its subnet-directed broadcast and to the limited broadcast
/// `255.255.255.255`: only the second reaches a camera configured for
/// another subnet, which is the camera a user most needs to find.
pub async fn discover_with(
    timeout: Duration,
    options: &DiscoveryOptions,
) -> Result<Vec<DeviceInfo>, GigeError> {
    discover_impl(timeout, options).await
}

/// Ask the camera at `ip` directly: a unicast `DISCOVERY_CMD` to its GVCP
/// port, for a camera that broadcast discovery does not reach — across a
/// router, say. `None` when nothing answers within `timeout`.
pub async fn discover_ip(ip: Ipv4Addr, timeout: Duration) -> Result<Option<DeviceInfo>, GigeError> {
    let socket = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0)).await?;
    socket
        .connect(SocketAddr::new(IpAddr::V4(ip), consts::PORT))
        .await?;
    let local_ip = match socket.local_addr()?.ip() {
        IpAddr::V4(local) => Some(local),
        IpAddr::V6(_) => None,
    };
    let request_id = 0x0100;
    let header = GvcpRequestHeader {
        flags: discovery_flags(false),
        command: consts::DISCOVERY_COMMAND,
        length: 0,
        request_id,
    };
    info!(%ip, "sending GVCP discovery by unicast");
    socket.send(&header.encode(&[])).await?;
    let mut buffer = vec![0u8; consts::DISCOVERY_BUFFER];
    let deadline = time::Instant::now() + timeout;
    loop {
        let len = match time::timeout_at(deadline, socket.recv(&mut buffer)).await {
            Err(_) => return Ok(None),
            Ok(Ok(len)) => len,
            // Nothing listens at that address: an answer, not a failure.
            Ok(Err(err)) if is_icmp_error(&err) => return Ok(None),
            Ok(Err(err)) => return Err(err.into()),
        };
        if let Some(mut info) = parse_discovery_ack(&buffer[..len], request_id) {
            info.local_ip = local_ip;
            info.local_netmask = local_ip.and_then(netmask_of);
            return Ok(Some(info));
        }
    }
}

/// The netmask of this host's address `ip`, if an interface has it.
fn netmask_of(ip: Ipv4Addr) -> Option<Ipv4Addr> {
    get_if_addrs()
        .ok()?
        .into_iter()
        .find_map(|iface| match iface.addr {
            IfAddr::V4(v4) if v4.ip == ip => Some(v4.netmask),
            _ => None,
        })
}

/// Send a FORCEIP command to temporarily assign an IP address to a device.
///
/// FORCEIP is a broadcast command that targets a device by its MAC address.
/// The assigned IP is temporary — it does not survive a power cycle. Use
/// [`GigeDevice::write_persistent_ip`] + [`GigeDevice::enable_persistent_ip`]
/// for permanent configuration.
///
/// FORCEIP payload layout (56 bytes, big-endian):
/// ```text
/// [0..2]   reserved
/// [2..8]   target MAC address (6 bytes)
/// [8..20]  reserved
/// [20..24] static IP address
/// [24..36] reserved
/// [36..40] subnet mask
/// [40..52] reserved
/// [52..56] gateway
/// ```
pub async fn force_ip(
    mac: [u8; 6],
    ip: Ipv4Addr,
    subnet: Ipv4Addr,
    gateway: Ipv4Addr,
    iface: Option<&Iface>,
) -> Result<(), GigeError> {
    // Build the 56-byte FORCEIP payload.
    let payload = encode_forceip_payload(mac, ip, subnet, gateway);

    let local_ip = match iface {
        Some(iface) => iface.ipv4().ok_or(GigeError::NotIpv4("interface"))?,
        None => Ipv4Addr::UNSPECIFIED,
    };

    let socket = UdpSocket::bind(SocketAddr::new(IpAddr::V4(local_ip), 0)).await?;
    socket.set_broadcast(true)?;
    let dest = SocketAddr::new(IpAddr::V4(Ipv4Addr::BROADCAST), consts::PORT);

    let header = GvcpRequestHeader {
        flags: CommandFlags::ACK_REQUIRED | CommandFlags::BROADCAST,
        command: consts::FORCEIP_COMMAND,
        length: payload.len() as u16,
        request_id: 1,
    };
    let packet = header.encode(&payload);
    info!(mac = ?mac, %ip, %subnet, %gateway, "sending FORCEIP command");
    socket.send_to(&packet, dest).await?;

    // Wait for FORCEIP_ACK.
    let mut buf = vec![0u8; consts::DISCOVERY_BUFFER];
    match time::timeout(consts::CONTROL_TIMEOUT, socket.recv_from(&mut buf)).await {
        Ok(Ok((len, _src))) => {
            if len < viva_gencp::HEADER_SIZE {
                return Err(GigeError::MalformedAck {
                    command: "FORCEIP",
                    problem: "is shorter than a GVCP header",
                });
            }
            let mut cursor = &buf[..];
            let status = cursor.get_u16();
            let command = cursor.get_u16();
            if command != consts::FORCEIP_ACK {
                debug!(
                    command = format_args!("{command:#06x}"),
                    "FORCEIP answered by another acknowledgement"
                );
                return Err(GigeError::MalformedAck {
                    command: "FORCEIP",
                    problem: "answers another command",
                });
            }
            let status = GigeStatus::from_raw(status);
            if !status.is_success() {
                return Err(GigeError::Status(status));
            }
            info!(%ip, "FORCEIP accepted");
            Ok(())
        }
        Ok(Err(e)) => Err(e.into()),
        Err(_) => Err(GigeError::Timeout),
    }
}

/// Encode the 56-byte FORCEIP payload.
fn encode_forceip_payload(
    mac: [u8; 6],
    ip: Ipv4Addr,
    subnet: Ipv4Addr,
    gateway: Ipv4Addr,
) -> Vec<u8> {
    let mut buf = vec![0u8; 56];
    // [0..2]   reserved
    // [2..8]   MAC address
    buf[2..8].copy_from_slice(&mac);
    // [8..20]  reserved
    // [20..24] IP address
    buf[20..24].copy_from_slice(&ip.octets());
    // [24..36] reserved
    // [36..40] subnet mask
    buf[36..40].copy_from_slice(&subnet.octets());
    // [40..52] reserved
    // [52..56] gateway
    buf[52..56].copy_from_slice(&gateway.octets());
    buf
}

async fn discover_impl(
    timeout: Duration,
    options: &DiscoveryOptions,
) -> Result<Vec<DeviceInfo>, GigeError> {
    let include_loopback = options.include_loopback || options.interface.is_some();
    let mut interfaces = Vec::new();
    for iface in get_if_addrs()? {
        let IfAddr::V4(v4) = iface.addr else {
            continue;
        };
        if !include_loopback && v4.ip.is_loopback() {
            continue;
        }
        if let Some(filter) = &options.interface
            && iface.name != *filter
        {
            continue;
        }
        interfaces.push((iface.name, iface.index, v4));
    }
    let flags = discovery_flags(options.allow_broadcast_ack);

    if interfaces.is_empty() {
        return Ok(Vec::new());
    }

    let mut join_set = JoinSet::new();
    for (idx, (name, index, v4)) in interfaces.into_iter().enumerate() {
        let request_id = 0x0100u16.wrapping_add(idx as u16);
        let interface_name = name.clone();
        join_set.spawn(async move {
            let local_addr = SocketAddr::new(IpAddr::V4(v4.ip), 0);
            let socket = match UdpSocket::bind(local_addr).await {
                Ok(socket) => socket,
                Err(err) => {
                    warn!(%interface_name, local = %v4.ip, error = %err,
                          "skipping interface: bind failed");
                    return Vec::new();
                }
            };
            if !v4.ip.is_loopback() {
                if let Err(err) = socket.set_broadcast(true) {
                    warn!(%interface_name, local = %v4.ip, error = %err,
                          "skipping interface: SO_BROADCAST failed");
                    return Vec::new();
                }
                bind_to_interface(&socket, index, &interface_name);
            }

            let header = GvcpRequestHeader {
                flags,
                command: consts::DISCOVERY_COMMAND,
                length: 0,
                request_id,
            };
            let packet = header.encode(&[]);
            let mut sent = 0;
            for dest in discovery_destinations(v4.ip, v4.netmask) {
                let destination = SocketAddr::new(IpAddr::V4(dest), consts::PORT);
                info!(%interface_name, local = %v4.ip, dest = %destination, "sending GVCP discovery");
                match socket.send_to(&packet, destination).await {
                    Ok(_) => sent += 1,
                    Err(err) => warn!(%interface_name, dest = %destination, error = %err,
                                      "discovery send failed"),
                }
            }
            if sent == 0 {
                warn!(%interface_name, "skipping interface: no discovery could be sent");
                return Vec::new();
            }

            let mut responses = Vec::new();
            let mut buffer = vec![0u8; consts::DISCOVERY_BUFFER];
            let timer = time::sleep(timeout);
            tokio::pin!(timer);
            loop {
                tokio::select! {
                    _ = &mut timer => break,
                    recv = socket.recv_from(&mut buffer) => {
                        // A receive error must not discard the cameras we have
                        // already found on this interface. Windows in particular
                        // reports WSAECONNRESET (10054) here when an earlier
                        // broadcast drew an ICMP port-unreachable (#57).
                        let (len, src) = match recv {
                            Ok(v) => v,
                            // An ICMP reply to one of the broadcasts says
                            // nothing about the cameras still answering.
                            Err(err) if is_icmp_error(&err) => {
                                debug!(%interface_name, error = %err,
                                       "ignoring an ICMP error on the discovery socket");
                                continue;
                            }
                            Err(err) => {
                                debug!(%interface_name, error = %err,
                                       "discovery receive failed; keeping results so far");
                                break;
                            }
                        };
                        info!(%interface_name, %src, "received GVCP response");
                        trace!(%interface_name, bytes = len, "GVCP response length");
                        if let Some(mut info) = parse_discovery_ack(&buffer[..len], request_id) {
                            trace!(ip = %info.ip, mac = %info.mac_string(), "parsed discovery ack");
                            info.local_ip = Some(v4.ip);
                            info.local_netmask = Some(v4.netmask);
                            responses.push(info);
                        }
                    }
                }
            }
            responses
        });
    }

    // One interface failing must never fail the whole call: a host commonly has
    // a down NIC, a Hyper-V switch, or a VPN adapter alongside the one the
    // camera is on.
    let mut seen = HashMap::new();
    while let Some(res) = join_set.join_next().await {
        match res {
            Ok(devices) => {
                for dev in devices {
                    seen.entry((dev.ip, dev.mac)).or_insert(dev);
                }
            }
            Err(err) => warn!(error = %err, "discovery task failed"),
        }
    }

    let mut devices: Vec<_> = seen.into_values().collect();
    devices.sort_by_key(|d| d.ip);
    Ok(devices)
}

/// Where an interface sends `DISCOVERY_CMD`.
///
/// On loopback, broadcast is not supported on some platforms (macOS), so the
/// command goes to the local address itself.
///
/// Elsewhere it goes to the subnet-directed broadcast, which routers may
/// forward, and to the limited broadcast `255.255.255.255`, which reaches
/// every device on the link whatever its address — the camera configured for
/// another subnet among them (aravis sends only the latter,
/// `arvgvinterface.c`). A camera that hears both answers twice; discovery
/// keeps one record per address and MAC.
fn discovery_destinations(ip: Ipv4Addr, netmask: Ipv4Addr) -> Vec<Ipv4Addr> {
    if ip.is_loopback() {
        return vec![ip];
    }
    let directed = directed_broadcast(ip, netmask);
    if directed == Ipv4Addr::BROADCAST {
        return vec![directed];
    }
    vec![directed, Ipv4Addr::BROADCAST]
}

/// The flag byte of a `DISCOVERY_CMD`: an acknowledgement asked for, and
/// *allow broadcast acknowledge* (`0x10`) only when the caller allows it.
fn discovery_flags(allow_broadcast_ack: bool) -> CommandFlags {
    if allow_broadcast_ack {
        CommandFlags::ACK_REQUIRED | CommandFlags::BROADCAST
    } else {
        CommandFlags::ACK_REQUIRED
    }
}

/// Whether `err` only reflects an ICMP reply to something the socket sent:
/// `ECONNREFUSED` on Unix, `WSAECONNRESET` on Windows.
fn is_icmp_error(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::ConnectionReset
    )
}

/// Keep a discovery socket's broadcasts on its own interface.
///
/// Linux sends a limited broadcast from the interface owning the socket's
/// source address. macOS routes it like any other packet — out of the primary
/// interface — unless the socket is bound to one (`IP_BOUND_IF`).
fn bind_to_interface(socket: &UdpSocket, index: Option<u32>, name: &str) {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        let index = index.and_then(std::num::NonZeroU32::new);
        if let Err(err) = socket2::SockRef::from(socket).bind_device_by_index_v4(index) {
            debug!(name, error = %err, "IP_BOUND_IF failed; the limited broadcast may leave by another interface");
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    let _ = (socket, index, name);
}

/// Derive an IPv4 directed-broadcast address from an interface address and
/// netmask.
///
/// On Linux an address added without an explicit `brd` value can make
/// `getifaddrs(3)` report the address itself through the broadcast union. That
/// turns discovery into a unicast request, notably for a manually configured
/// `169.254.0.0/16` address. The netmask remains authoritative, so calculate
/// the broadcast address ourselves instead of trusting that optional field.
fn directed_broadcast(ip: Ipv4Addr, netmask: Ipv4Addr) -> Ipv4Addr {
    Ipv4Addr::from(u32::from(ip) | !u32::from(netmask))
}

/// Decode one datagram received on the discovery socket.
///
/// Returns `None` — never an error — for anything that is not a usable
/// Discovery ACK for us. The socket is bound to an ephemeral port on a
/// broadcast network, so unrelated GVCP traffic is expected; treating it as
/// fatal would discard the cameras already found on this interface (#57).
fn parse_discovery_ack(buf: &[u8], expected_request: u16) -> Option<DeviceInfo> {
    if buf.len() < viva_gencp::HEADER_SIZE {
        trace!(len = buf.len(), "ignoring short GVCP datagram");
        return None;
    }
    let mut header = buf;
    let status = header.get_u16();
    let command = header.get_u16();
    let length = header.get_u16() as usize;
    let request_id = header.get_u16();
    if request_id != expected_request {
        return None;
    }
    if command != consts::DISCOVERY_ACK {
        debug!(
            opcode = format_args!("{command:#06x}"),
            "ignoring non-discovery GVCP ack"
        );
        return None;
    }
    if status != 0 {
        debug!(
            status = format_args!("{status:#06x}"),
            "discovery ack reported a non-zero status"
        );
        return None;
    }
    if buf.len() < viva_gencp::HEADER_SIZE + length {
        debug!(
            declared = length,
            actual = buf.len() - viva_gencp::HEADER_SIZE,
            "ignoring truncated discovery payload"
        );
        return None;
    }
    let payload = &buf[viva_gencp::HEADER_SIZE..viva_gencp::HEADER_SIZE + length];
    match parse_discovery_payload(payload) {
        Ok(info) => Some(info),
        Err(err) => {
            debug!(error = %err, "ignoring unparsable discovery payload");
            None
        }
    }
}

/// Parse a GigE Vision Discovery ACK payload (248 bytes).
///
/// The payload mirrors the device's bootstrap register block, so the MAC is
/// split across `DeviceMACAddressHigh` (0x08, whose low half holds the top two
/// octets) and `DeviceMACAddressLow` (0x0C) — six contiguous bytes at offset
/// **10**, not 12. Cross-checked against Wireshark's `dissect_discovery_ack()`,
/// which reads the MAC from `offset + 10` and the IP, manufacturer and model
/// from 36, 72 and 104. Reported in #57 against a JAI FS-3200T-10GE-NNC, whose
/// `00:0C:DF:06:5B:2F` was read as `DF:06:5B:2F:C0:00`.
///
/// | Offset | Size | Field                        |
/// |--------|------|------------------------------|
/// |      0 |    2 | Spec version major           |
/// |      2 |    2 | Spec version minor           |
/// |      4 |    4 | Device mode                  |
/// |      8 |    2 | Reserved (MAC-high padding)  |
/// |     10 |    6 | MAC address                  |
/// |     16 |    4 | Supported IP config          |
/// |     20 |    4 | Current IP config            |
/// |     24 |   12 | Reserved                     |
/// |     36 |    4 | Current IP address           |
/// |     40 |   12 | Reserved                     |
/// |     52 |    4 | Current subnet mask          |
/// |     56 |   12 | Reserved                     |
/// |     68 |    4 | Default gateway              |
/// |     72 |   32 | Manufacturer name            |
/// |    104 |   32 | Model name                   |
/// |    136 |   32 | Device version               |
/// |    168 |   48 | Manufacturer specific info   |
/// |    216 |   16 | Serial number                |
/// |    232 |   16 | User defined name            |
fn parse_discovery_payload(payload: &[u8]) -> Result<DeviceInfo, GigeError> {
    // Minimum size to reach past the current IP field. Everything after it is
    // read leniently: a short-but-valid ACK should still yield a usable device
    // rather than failing discovery on that interface.
    if payload.len() < 40 {
        return Err(GigeError::AckLength {
            command: "DISCOVERY",
            expected: 40,
            got: payload.len(),
        });
    }
    let mut cursor = Cursor::new(payload);
    let _spec_major = cursor.get_u16(); // 0
    let _spec_minor = cursor.get_u16(); // 2
    let _device_mode = cursor.get_u32(); // 4

    // Only the two padding bytes of the MAC-high register precede the address.
    cursor.advance(2); // 8..10

    // MAC: 2 bytes from the high register + 4 from the low register.
    let mut mac = [0u8; 6];
    cursor.copy_to_slice(&mut mac); // 10..16

    let _supported_ip_config = cursor.get_u32(); // 16
    let _current_ip_config = cursor.get_u32(); // 20

    // 12 bytes reserved before current IP.
    cursor.advance(12); // 24..36
    let ip = Ipv4Addr::from(cursor.get_u32()); // 36

    // Everything past the IP is optional, so every step from here on has to
    // tolerate the payload ending.
    skip(&mut cursor, 12); // 40..52 reserved
    let subnet = read_ipv4(&mut cursor); // 52
    skip(&mut cursor, 12); // 56..68 reserved
    let gateway = read_ipv4(&mut cursor); // 68

    // String fields. All optional: a device that truncates the payload still
    // gives us an addressable camera.
    let manufacturer = read_fixed_string(&mut cursor, 32); // 72
    let model = read_fixed_string(&mut cursor, 32); // 104
    let version = read_fixed_string(&mut cursor, 32); // 136
    skip(&mut cursor, 48); // 168 manufacturer-specific info
    let serial = read_fixed_string(&mut cursor, 16); // 216
    let user_name = read_fixed_string(&mut cursor, 16); // 232

    Ok(DeviceInfo {
        ip,
        mac,
        manufacturer,
        model,
        version,
        serial,
        user_name,
        local_ip: None,
        subnet,
        gateway,
        local_netmask: None,
    })
}

/// Read an IPv4 address, or `None` if the payload ends early.
fn read_ipv4(cursor: &mut Cursor<&[u8]>) -> Option<Ipv4Addr> {
    (cursor.remaining() >= 4).then(|| Ipv4Addr::from(cursor.get_u32()))
}

/// Read a NUL-padded fixed-width string, or `None` if the payload ends early.
fn read_fixed_string(cursor: &mut Cursor<&[u8]>, len: usize) -> Option<String> {
    if cursor.remaining() < len {
        return None;
    }
    let mut buf = vec![0u8; len];
    cursor.copy_to_slice(&mut buf);
    parse_string(&buf)
}

/// Advance past a field, stopping at the end of a truncated payload.
fn skip(cursor: &mut Cursor<&[u8]>, len: usize) {
    let n = len.min(cursor.remaining());
    cursor.advance(n);
}

fn parse_string(bytes: &[u8]) -> Option<String> {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    let slice = &bytes[..end];
    let s = String::from_utf8_lossy(slice).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// Outcome of waiting for one acknowledgement.
///
/// Mirrors the three cases the caller already distinguishes — a datagram, a
/// socket error, and a deadline — so that PENDING_ACK handling can be factored
/// out without changing the retry policy around it.
enum AckRecv {
    Received(usize),
    Io(std::io::Error),
    TimedOut,
}

/// Decode a GVCP PENDING_ACK, returning the request id it refers to and the
/// extra time the device is asking for.
///
/// Returns `None` for anything that is not a PENDING_ACK, so the caller can
/// hand the datagram to the normal GenCP decoder.
///
/// Layout per GigE Vision 1.2 section 18.5: the 8-byte GVCP acknowledgement
/// header, then two reserved bytes and a 16-bit `time_to_completion` in
/// milliseconds.
///
/// This diverges deliberately from aravis, whose
/// `arv_gvcp_packet_get_pending_ack_timeout` reads all four payload bytes as a
/// big-endian `u32`. The two agree whenever the reserved field is zero, which
/// is what the specification requires of the device; reading the `u16` the
/// specification actually defines is the safer of the two, because a device
/// that leaves junk in the reserved field cannot then talk us into a wait
/// three orders of magnitude too long. Callers clamp the result regardless.
fn parse_pending_ack(buf: &[u8]) -> Option<(u16, Duration)> {
    if buf.len() < viva_gencp::HEADER_SIZE {
        return None;
    }
    if u16::from_be_bytes([buf[2], buf[3]]) != consts::PENDING_ACK {
        return None;
    }
    let request_id = u16::from_be_bytes([buf[6], buf[7]]);
    let payload = &buf[viva_gencp::HEADER_SIZE..];
    // A truncated PENDING_ACK is still an unambiguous request for more time;
    // grant the default rather than discarding it and resending the command.
    let millis = match payload {
        [_, _, hi, lo, ..] => u16::from_be_bytes([*hi, *lo]),
        _ => return Some((request_id, consts::CONTROL_TIMEOUT)),
    };
    Some((request_id, Duration::from_millis(u64::from(millis))))
}

/// Environment variable that starts every [`GigeDevice`] on READMEM/WRITEMEM.
///
/// Any non-empty value other than `0` counts as set. The escape hatch for a
/// device that mishandles READREG/WRITEREG in a way the automatic fallback
/// cannot detect — answering with a wrong value rather than refusing — so a
/// user can get back to memory access without a rebuild.
pub const FORCE_READMEM_ENV: &str = "VIVA_GIGE_FORCE_READMEM";

/// Whether a [`FORCE_READMEM_ENV`] value asks for memory access.
fn force_readmem_requested(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|v| !v.is_empty() && v != "0")
}

/// The GVCP name of a register or memory command, for errors.
fn command_name(opcode: OpCode) -> &'static str {
    match opcode {
        OpCode::ReadRegister => "READREG",
        OpCode::WriteRegister => "WRITEREG",
        OpCode::ReadMem => "READMEM",
        OpCode::WriteMem => "WRITEMEM",
    }
}

/// The READREG/WRITEREG address for an access that is exactly one register.
///
/// A GVCP register is a 32-bit word on a 32-bit boundary in the low 4 GiB, so
/// anything else stays READMEM/WRITEMEM. In particular an 8-byte access is
/// *not* split into two registers: a multi-register READREG has no defined
/// atomicity, so a latched 64-bit timestamp could tear silently, and a
/// WRITEREG acknowledgement carries only an index placeholder, so a partial
/// multi-register write could not even be reported (backlog TC-22).
fn single_register_address(addr: u64, len: usize) -> Option<u32> {
    if len != 4 || !addr.is_multiple_of(4) {
        return None;
    }
    u32::try_from(addr).ok()
}

/// GVCP Capability (`0x0934`): which optional commands and behaviours the
/// device implements, one bit each.
///
/// Masks from Wireshark's `hf_gvcp_capability_*` fields; the specification
/// numbers bits from the most significant, so `0x20` is its bit 26. Only the
/// bits something here acts on, or a report shows, have a method; the rest
/// are in [`bits`](Self::bits).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GvcpCapability(u32);

impl GvcpCapability {
    /// The capability word as read.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// The raw word.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// `PENDING_ACK` can be enabled in GVCP Configuration (`0x20`).
    pub const fn pending_ack(self) -> bool {
        self.0 & consts::CAPABILITY_PENDING_ACK != 0
    }

    /// `PACKETRESEND_CMD` is answered (`0x04`).
    pub const fn packet_resend(self) -> bool {
        self.0 & consts::CAPABILITY_PACKET_RESEND != 0
    }

    /// `WRITEMEM_CMD` is implemented (`0x02`).
    pub const fn writemem(self) -> bool {
        self.0 & 0x0000_0002 != 0
    }

    /// Several registers in one READREG/WRITEREG (`0x01`).
    pub const fn concatenation(self) -> bool {
        self.0 & 0x0000_0001 != 0
    }

    /// `EVENT_CMD` is sent (`0x08`).
    pub const fn event(self) -> bool {
        self.0 & 0x0000_0008 != 0
    }

    /// `EVENTDATA_CMD` is sent (`0x10`).
    pub const fn event_data(self) -> bool {
        self.0 & 0x0000_0010 != 0
    }

    /// `ACTION_CMD` is answered (`0x40`).
    pub const fn action(self) -> bool {
        self.0 & 0x0000_0040 != 0
    }

    /// The heartbeat can be disabled in GVCP Configuration (`0x2000_0000`).
    pub const fn heartbeat_disable(self) -> bool {
        self.0 & 0x2000_0000 != 0
    }

    /// The primary application's IP and port are readable (`0x0800_0000`).
    pub const fn ccp_application_port_ip(self) -> bool {
        self.0 & 0x0800_0000 != 0
    }

    /// Control can be handed over with a switchover key (`0x0020_0000`).
    pub const fn primary_application_switchover(self) -> bool {
        self.0 & 0x0020_0000 != 0
    }
}

/// GVCP device handle.
pub struct GigeDevice {
    socket: UdpSocket,
    remote: SocketAddr,
    control_timeout: Duration,
    inflight_keepalive_period: Option<Duration>,
    last_command_sent: time::Instant,
    request_id: u16,
    rng: Rng,
    /// Single-register access goes out as READMEM/WRITEMEM instead of
    /// READREG/WRITEREG. Set by [`FORCE_READMEM_ENV`] at open, or latched for
    /// the rest of the session once the device refuses a register command.
    memory_access_only: bool,
    /// Whether the device names its primary application (GVCP Capability
    /// `0x0800_0000`), read once when first needed.
    names_primary_application: Option<bool>,
}

/// Stream negotiation outcome describing the values written to the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamParams {
    /// Selected GVSP packet size (bytes).
    pub packet_size: u32,
    /// `GevSCPD` written, in timestamp ticks; `None` when the stream left the
    /// camera's own value alone.
    pub packet_delay: Option<u32>,
    /// Link MTU used to derive the packet size.
    pub mtu: u32,
    /// Host IPv4 address configured on the device.
    pub host: Ipv4Addr,
    /// Host port configured on the device.
    pub port: u16,
    /// Stream channel index.
    pub channel: u32,
    /// The camera's GVCP address, where packet resend requests go; `None`
    /// for a stream received on a socket of the caller's.
    pub camera: Option<SocketAddr>,
}

impl GigeDevice {
    /// Connect to a device GVCP endpoint.
    ///
    /// The connection is ready for register read/write but does not claim
    /// control privilege. Call [`Self::claim_control`] before configuring streaming
    /// or starting acquisition.
    pub async fn open(addr: SocketAddr) -> Result<Self, GigeError> {
        let local_ip = match addr.ip() {
            IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            IpAddr::V6(_) => {
                return Err(GigeError::NotIpv4("device"));
            }
        };
        let socket = UdpSocket::bind(SocketAddr::new(local_ip, 0)).await?;
        socket.connect(addr).await?;
        let memory_access_only =
            force_readmem_requested(std::env::var_os(FORCE_READMEM_ENV).as_deref());
        if memory_access_only {
            info!(%addr, "{FORCE_READMEM_ENV} is set; using READMEM/WRITEMEM for every register");
        }
        Ok(Self {
            socket,
            remote: addr,
            control_timeout: consts::CONTROL_TIMEOUT,
            inflight_keepalive_period: None,
            last_command_sent: time::Instant::now(),
            request_id: 1,
            rng: Rng::new(),
            memory_access_only,
            names_primary_application: None,
        })
    }

    /// Set the acknowledgement timeout per control transaction attempt.
    ///
    /// Applies to register/memory access, control privilege and packet resend.
    /// PENDING_ACK may extend this wait, but never shorten it. Automatic retries
    /// retain their existing limit. Zero requests an immediate timeout.
    pub fn set_control_timeout(&mut self, timeout: Duration) {
        self.control_timeout = timeout;
    }

    /// Return the acknowledgement timeout per control transaction attempt.
    pub fn control_timeout(&self) -> Duration {
        self.control_timeout
    }

    /// Configure control-channel probes while a transaction waits for its ACK.
    ///
    /// After this much silence on the socket, send an idempotent CCP read using
    /// the same source port and a separate request ID. Probe replies are drained
    /// without extending the original transaction's deadline. Use a period
    /// comfortably shorter than the device's heartbeat timeout.
    ///
    /// `None` or a zero period disables the probes (the default). This handles
    /// in-flight requests only; an idle connection still needs a keepalive.
    pub fn set_inflight_keepalive_period(&mut self, period: Option<Duration>) {
        self.inflight_keepalive_period = period.filter(|period| !period.is_zero());
    }

    /// The in-flight heartbeat period, `None` while they are off.
    pub fn inflight_keepalive_period(&self) -> Option<Duration> {
        self.inflight_keepalive_period
    }

    /// Send every single-register access as READMEM/WRITEMEM from now on.
    ///
    /// What [`FORCE_READMEM_ENV`] does at open, for a caller that knows its
    /// device better than the environment does. There is no way back within
    /// the session, as there is none from the automatic fallback.
    pub fn use_memory_access(&mut self) {
        self.memory_access_only = true;
    }

    /// Whether single-register access currently goes out as READMEM/WRITEMEM.
    pub fn memory_access_only(&self) -> bool {
        self.memory_access_only
    }

    /// Claim control channel privilege (CCP).
    ///
    /// Required by the GigE Vision specification before the device accepts
    /// stream configuration or acquisition commands.
    pub async fn claim_control(&mut self) -> Result<(), GigeError> {
        self.write_u32(consts::CONTROL_CHANNEL_PRIVILEGE, consts::CCP_CONTROL)
            .await?;
        debug!(addr = %self.remote, "claimed control channel privilege");
        Ok(())
    }

    /// Release control channel privilege.
    pub async fn release_control(&mut self) -> Result<(), GigeError> {
        self.write_u32(consts::CONTROL_CHANNEL_PRIVILEGE, 0).await
    }

    /// Send the release of control privilege (CCP ← 0) without waiting for
    /// its acknowledgement, and without a runtime to wait on.
    ///
    /// For a caller that cannot block — a `Drop` on a current-thread runtime's
    /// own thread. Best effort: a lost datagram leaves the device to time the
    /// privilege out. Prefer [`Self::release_control`].
    ///
    /// Sent as WRITEMEM once the session uses memory access
    /// ([`Self::memory_access_only`]), as every other register write is: a
    /// device that refuses WRITEREG would refuse the release too. Both
    /// commands carry the same eight bytes, the address and then the value.
    pub fn try_send_release(&mut self) -> std::io::Result<()> {
        let request_id = self.next_request_id();
        let mut payload = BytesMut::with_capacity(8);
        payload.put_u32(consts::CONTROL_CHANNEL_PRIVILEGE as u32);
        payload.put_u32(0);
        let opcode = if self.memory_access_only {
            OpCode::WriteMem
        } else {
            OpCode::WriteRegister
        };
        let header = GvcpRequestHeader {
            flags: CommandFlags::ACK_REQUIRED,
            command: opcode.command_code(),
            length: payload.len() as u16,
            request_id,
        };
        self.socket.try_send(&header.encode(&payload)).map(drop)
    }

    /// Read `GevHeartbeatTimeout` (milliseconds).
    ///
    /// This is how long the device will keep control privilege granted with no
    /// GVCP command from the controlling application.
    pub async fn heartbeat_timeout_ms(&mut self) -> Result<u32, GigeError> {
        self.read_u32(consts::HEARTBEAT_TIMEOUT).await
    }

    /// Refresh the device's heartbeat timer, and report whether this
    /// application still holds control privilege.
    ///
    /// Any GVCP command resets the timer, so this reads CCP rather than writing
    /// it: a read is idempotent, and its value doubles as the answer to "does
    /// the device still consider us the controller?". A device that revoked
    /// privilege — because an earlier heartbeat was lost, or because another
    /// application took control — is reported as `Ok(false)` here instead of
    /// failing the next configuration write with `ACCESS_DENIED`.
    ///
    /// Losing privilege is not a transport failure, so it is not an `Err`: the
    /// register read succeeded and the answer is simply "no". `Err` means the
    /// command itself did not complete.
    ///
    /// CCP alone cannot tell this application from another that took control
    /// after an expired heartbeat: both read as controlled. Where GVCP
    /// Capability says the device names its primary application (bit
    /// `0x0800_0000`), its IP and port are compared with this socket's too
    /// (backlog TC-38).
    pub async fn ping_control_channel(&mut self) -> Result<bool, GigeError> {
        let privilege = self.read_u32(consts::CONTROL_CHANNEL_PRIVILEGE).await?;
        if privilege & consts::CCP_CONTROLLER_BITS == 0 {
            return Ok(false);
        }
        self.primary_application_is_us().await
    }

    /// Whether the device's primary application is this socket, where the
    /// device says (`0x0A04` port, `0x0A14` IP); `true` where it does not.
    async fn primary_application_is_us(&mut self) -> Result<bool, GigeError> {
        let names_owner = match self.names_primary_application {
            Some(known) => known,
            None => {
                let known = self
                    .gvcp_capability()
                    .await
                    .map(GvcpCapability::ccp_application_port_ip)
                    .unwrap_or(false);
                self.names_primary_application = Some(known);
                known
            }
        };
        if !names_owner {
            return Ok(true);
        }
        let SocketAddr::V4(us) = self.socket.local_addr()? else {
            return Ok(true);
        };
        let port = self.read_u32(consts::PRIMARY_APPLICATION_PORT).await? & 0xFFFF;
        let ip = Ipv4Addr::from(self.read_u32(consts::PRIMARY_APPLICATION_IP).await?);
        Ok(port == u32::from(us.port()) && ip == *us.ip())
    }

    /// Return the remote GVCP socket address associated with this device.
    pub fn remote_addr(&self) -> SocketAddr {
        self.remote
    }

    /// The local address and port commands leave from: what the device
    /// knows this application by.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    fn next_request_id(&mut self) -> u16 {
        let id = self.request_id;
        self.request_id = self.request_id.wrapping_add(1);
        if self.request_id == 0 {
            self.request_id = 1;
        }
        id
    }

    async fn transact_with_retry(
        &mut self,
        opcode: OpCode,
        payload: BytesMut,
    ) -> Result<GenCpAck, GigeError> {
        // Retries resend the same transaction. A device may finish a slow,
        // non-idempotent command after our first receive deadline; assigning a
        // new ID would turn its delayed acknowledgement into a mismatch and
        // could execute the command again.
        let request_id = self.next_request_id();
        let payload_bytes = payload.freeze();
        let header = GvcpRequestHeader {
            flags: CommandFlags::ACK_REQUIRED,
            command: opcode.command_code(),
            length: payload_bytes.len() as u16,
            request_id,
        };
        let encoded = header.encode(&payload_bytes);
        let mut attempt = 0usize;
        'retry: loop {
            attempt += 1;
            trace!(request_id, opcode = ?opcode, bytes = encoded.len(), attempt, "sending GVCP command");
            if let Err(err) = self.socket.send(&encoded).await {
                if attempt >= consts::MAX_RETRIES {
                    return Err(err.into());
                }
                warn!(request_id, ?opcode, attempt, "send failed, retrying");
                self.backoff(attempt).await;
                continue 'retry;
            }
            self.last_command_sent = time::Instant::now();

            let mut buf = vec![
                0u8;
                viva_gencp::HEADER_SIZE
                    + consts::GENCP_MAX_BLOCK
                    + consts::GENCP_WRITE_OVERHEAD
            ];
            // Only this request's datagram comes back: `recv_for_request` drains
            // replies to other request IDs — a delayed ACK for an earlier
            // command, an in-flight heartbeat's reply — against this attempt's
            // deadline, so they can neither be mistaken for it nor extend it.
            match self.recv_absorbing_pending(&mut buf, request_id).await {
                AckRecv::Received(len) => {
                    trace!(request_id, bytes = len, attempt, "received GenCP ack");
                    let ack = decode_ack(&buf[..len])?;
                    if ack.header.opcode != opcode {
                        return Err(GigeError::MalformedAck {
                            command: command_name(opcode),
                            problem: "answers another command",
                        });
                    }
                    match GigeStatus::from_raw(ack.header.status) {
                        status if status.is_success() => return Ok(ack),
                        // Only `BUSY` (0x8007) is congestion. This used to
                        // match `DeviceBusy`, which was mapped to 0x8004 —
                        // `WRITE_PROTECT` — so the retry loop burned its
                        // budget on a read-only register that could never
                        // accept the write, and gave up immediately on the
                        // one status retrying is for.
                        status if status.is_retryable() && attempt < consts::MAX_RETRIES => {
                            warn!(request_id, attempt, %status, "device busy, retrying");
                            self.backoff(attempt).await;
                            continue 'retry;
                        }
                        other => return Err(GigeError::Status(other)),
                    }
                }
                AckRecv::Io(err) => {
                    if attempt >= consts::MAX_RETRIES {
                        return Err(err.into());
                    }
                    warn!(request_id, ?opcode, attempt, "receive error, retrying");
                    self.backoff(attempt).await;
                    continue 'retry;
                }
                AckRecv::TimedOut => {
                    if attempt >= consts::MAX_RETRIES {
                        return Err(GigeError::Timeout);
                    }
                    warn!(request_id, ?opcode, attempt, "command timeout, retrying");
                    self.backoff(attempt).await;
                    continue 'retry;
                }
            }
        }
    }

    /// Send a heartbeat without recursively entering the active transaction.
    async fn send_inflight_keepalive(&mut self, active_request_id: u16) -> Result<(), GigeError> {
        let mut request_id = self.next_request_id();
        // A long transaction may outlive the 16-bit request-ID counter.
        if request_id == active_request_id {
            request_id = self.next_request_id();
        }
        let mut payload = BytesMut::with_capacity(8);
        payload.put_u32(consts::CONTROL_CHANNEL_PRIVILEGE as u32);
        let opcode = if self.memory_access_only {
            payload.put_u16(0);
            payload.put_u16(4);
            OpCode::ReadMem
        } else {
            OpCode::ReadRegister
        };
        let header = GvcpRequestHeader {
            flags: CommandFlags::ACK_REQUIRED,
            command: opcode.command_code(),
            length: payload.len() as u16,
            request_id,
        };
        self.socket.send(&header.encode(&payload)).await?;
        self.last_command_sent = time::Instant::now();
        trace!(
            request_id,
            active_request_id, "sent in-flight control keepalive"
        );
        Ok(())
    }

    /// One socket reader owns both the active request and its heartbeat replies.
    /// Keep an absolute deadline so unrelated or duplicate ACKs cannot postpone
    /// a timeout, and never resend the active command merely to send a heartbeat.
    ///
    /// `committed` says the device already has the command — it answered with
    /// `PENDING_ACK` — so nothing on the socket may end the wait early: a
    /// resend could run it twice. An ICMP error after an in-flight heartbeat
    /// is ignored for the same reason (TC-26): it may be the heartbeat's, and
    /// the command it interrupts may be executing. Only an error the active
    /// command alone can have drawn ends the wait, so an absent device still
    /// fails fast.
    async fn recv_for_request(
        &mut self,
        buf: &mut [u8],
        request_id: u16,
        deadline: time::Instant,
        committed: bool,
    ) -> AckRecv {
        let mut tolerate_icmp = committed;
        loop {
            let heartbeat_deadline = self
                .inflight_keepalive_period
                .and_then(|period| self.last_command_sent.checked_add(period));
            tokio::select! {
                _ = time::sleep_until(deadline) => return AckRecv::TimedOut,
                _ = async {
                    if let Some(deadline) = heartbeat_deadline {
                        time::sleep_until(deadline).await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {
                    tolerate_icmp = true;
                    if let Err(err) = self.send_inflight_keepalive(request_id).await {
                        // The command is still waiting for its answer; a lost
                        // heartbeat costs at most this period's refresh.
                        debug!(request_id, error = %err, "in-flight heartbeat not sent; still waiting");
                        self.last_command_sent = time::Instant::now();
                    }
                }
                received = self.socket.recv(buf) => {
                    let len = match received {
                        Ok(len) => len,
                        Err(err) if tolerate_icmp && is_icmp_error(&err) => {
                            debug!(request_id, error = %err, "ignoring an ICMP error while the command is pending");
                            continue;
                        }
                        Err(err) => return AckRecv::Io(err),
                    };
                    let ack_id = buf.get(..len)
                        .and_then(|packet| packet.get(6..8))
                        .and_then(|bytes| bytes.try_into().ok())
                        .map(u16::from_be_bytes);
                    if let Some(ack_id) = ack_id
                        && ack_id != request_id
                    {
                        trace!(request_id, ack_id, "drained keepalive or delayed acknowledgement");
                        continue;
                    }
                    // Preserve existing validation of malformed matching ACKs.
                    return AckRecv::Received(len);
                }
            }
        }
    }

    /// Receive one acknowledgement, granting the device the extra time it asks
    /// for via PENDING_ACK.
    ///
    /// A PENDING_ACK is not a failure and must not be retried: the command is
    /// still executing on the device, so resending it risks running it twice —
    /// which for a `WriteMem` to flash is exactly the operation you least want
    /// duplicated. The GigE Vision specification instead has the controller
    /// extend its own deadline by the time the device requests and keep
    /// waiting on the same request id.
    async fn recv_absorbing_pending(&mut self, buf: &mut [u8], request_id: u16) -> AckRecv {
        let mut wait = self.control_timeout;
        let mut pending_seen = 0usize;
        loop {
            let deadline = time::Instant::now() + wait;
            match self
                .recv_for_request(buf, request_id, deadline, pending_seen > 0)
                .await
            {
                AckRecv::Received(len) => {
                    // `recv_for_request` has already drained PENDING_ACKs for
                    // other request IDs, so this one is ours.
                    let Some((_, requested)) = parse_pending_ack(&buf[..len]) else {
                        return AckRecv::Received(len);
                    };
                    pending_seen += 1;
                    if pending_seen > consts::MAX_PENDING_ACKS {
                        warn!(
                            request_id,
                            pending_seen, "device kept requesting more time, giving up"
                        );
                        return AckRecv::TimedOut;
                    }
                    wait = requested
                        .clamp(consts::CONTROL_TIMEOUT, consts::MAX_PENDING_ACK_WAIT)
                        .max(self.control_timeout);
                    debug!(
                        request_id,
                        requested_ms = requested.as_millis() as u64,
                        waiting_ms = wait.as_millis() as u64,
                        pending_seen,
                        "device requested more time (PENDING_ACK)"
                    );
                }
                other => return other,
            }
        }
    }

    async fn backoff(&mut self, attempt: usize) {
        let multiplier = 1u32 << (attempt.saturating_sub(1)).min(3);
        let base_ms = consts::RETRY_BASE_DELAY.as_millis() as u64;
        let base = Duration::from_millis(base_ms.saturating_mul(multiplier as u64).max(base_ms));
        let jitter_ms = self.rng.u64(..=consts::RETRY_JITTER.as_millis() as u64);
        let jitter = Duration::from_millis(jitter_ms);
        let delay = base + jitter;
        debug!(attempt, delay = ?delay, "gvcp retry backoff");
        time::sleep(delay).await;
    }

    /// Read `len` bytes at `addr`, as READREG when that is exactly one
    /// register and READMEM otherwise.
    ///
    /// The READREG value is returned big-endian, which is byte-for-byte what
    /// READMEM returns for the same word: GVCP carries both in network order,
    /// and interpreting the bytes is GenApi's job, not the transport's.
    ///
    /// A device that answers READREG with `NOT_IMPLEMENTED`, or does not
    /// answer it at all, is switched to READMEM for the rest of the session
    /// and this access is retried there, so the caller still gets its value.
    /// Any other refusal — `ACCESS_DENIED`, `BAD_ALIGNMENT` — is the device's
    /// real answer about this register and is returned as it is.
    pub async fn read_register_or_mem(
        &mut self,
        addr: u64,
        len: usize,
    ) -> Result<Vec<u8>, GigeError> {
        let register = match single_register_address(addr, len) {
            Some(register) if !self.memory_access_only => register,
            _ => return self.read_mem(addr, len).await,
        };
        match self.read_register(register).await {
            Ok(value) => Ok(value.to_be_bytes().to_vec()),
            Err(GigeError::Status(GigeStatus::Common(StatusCode::NotImplemented))) => {
                self.latch_memory_access("READREG", addr, "NOT_IMPLEMENTED");
                self.read_mem(addr, len).await
            }
            Err(GigeError::Timeout) => {
                // Silence is ambiguous: the device may ignore READREG, or may
                // be gone. Latch only once READMEM proves it is still there,
                // so a cable pull does not cost the session its READREG.
                let data = self.read_mem(addr, len).await?;
                self.latch_memory_access("READREG", addr, "no reply");
                Ok(data)
            }
            Err(err) => Err(err),
        }
    }

    /// Write `data` at `addr`, as WRITEREG when that is exactly one register
    /// and WRITEMEM otherwise.
    ///
    /// Falls back to WRITEMEM exactly as [`Self::read_register_or_mem`] falls
    /// back to READMEM. After a WRITEREG that got no reply the write is sent
    /// again as WRITEMEM; a device that did execute the first one but lost
    /// the acknowledgement sees it twice, which is the same exposure the
    /// command retry loop already has.
    pub async fn write_register_or_mem(&mut self, addr: u64, data: &[u8]) -> Result<(), GigeError> {
        let (register, value) = match (single_register_address(addr, data.len()), data) {
            (Some(register), &[b0, b1, b2, b3]) if !self.memory_access_only => {
                (register, u32::from_be_bytes([b0, b1, b2, b3]))
            }
            _ => return self.write_mem(addr, data).await,
        };
        match self.write_register(register, value).await {
            Ok(()) => Ok(()),
            Err(GigeError::Status(GigeStatus::Common(StatusCode::NotImplemented))) => {
                self.latch_memory_access("WRITEREG", addr, "NOT_IMPLEMENTED");
                self.write_mem(addr, data).await
            }
            Err(GigeError::Timeout) => {
                self.write_mem(addr, data).await?;
                self.latch_memory_access("WRITEREG", addr, "no reply");
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    /// Switch this session to memory access, warning the first time only.
    fn latch_memory_access(&mut self, command: &str, addr: u64, why: &str) {
        if self.memory_access_only {
            return;
        }
        self.memory_access_only = true;
        warn!(
            device = %self.remote,
            addr = format!("0x{addr:08x}"),
            "device refused {command} ({why}); using READMEM/WRITEMEM for the rest of \
             this session (set {FORCE_READMEM_ENV}=1 to start that way)"
        );
    }

    /// Read one 32-bit register through [`Self::read_register_or_mem`].
    async fn read_u32(&mut self, addr: u64) -> Result<u32, GigeError> {
        let data = self.read_register_or_mem(addr, 4).await?;
        let word: [u8; 4] = data
            .as_slice()
            .try_into()
            .map_err(|_| GigeError::AckLength {
                command: "READREG",
                expected: 4,
                got: data.len(),
            })?;
        Ok(u32::from_be_bytes(word))
    }

    /// Write one 32-bit register through [`Self::write_register_or_mem`].
    async fn write_u32(&mut self, addr: u64, value: u32) -> Result<(), GigeError> {
        self.write_register_or_mem(addr, &value.to_be_bytes()).await
    }

    /// Read a single 32-bit register with READREG, unconditionally.
    ///
    /// Uses GVCP READREG format: 4-byte register address.
    /// The acknowledgement carries the 4-byte register value.
    ///
    /// No fallback: prefer [`Self::read_register_or_mem`], which picks the
    /// command and honours a device that does not implement this one.
    pub async fn read_register(&mut self, addr: u32) -> Result<u32, GigeError> {
        let mut payload = BytesMut::with_capacity(4);
        payload.put_u32(addr);
        let ack = self
            .transact_with_retry(OpCode::ReadRegister, payload)
            .await?;
        if ack.payload.len() != 4 {
            return Err(GigeError::AckLength {
                command: "READREG",
                expected: 4,
                got: ack.payload.len(),
            });
        }
        let mut cursor = &ack.payload[..];
        Ok(cursor.get_u32())
    }

    /// Write a single 32-bit register with WRITEREG, unconditionally.
    ///
    /// Uses GVCP WRITEREG format: 4-byte register address + 4-byte value.
    /// The acknowledgement carries a 4-byte data index placeholder.
    ///
    /// No fallback: prefer [`Self::write_register_or_mem`].
    pub async fn write_register(&mut self, addr: u32, value: u32) -> Result<(), GigeError> {
        let mut payload = BytesMut::with_capacity(8);
        payload.put_u32(addr);
        payload.put_u32(value);
        let ack = self
            .transact_with_retry(OpCode::WriteRegister, payload)
            .await?;
        if ack.payload.len() != 4 {
            return Err(GigeError::AckLength {
                command: "WRITEREG",
                expected: 4,
                got: ack.payload.len(),
            });
        }
        Ok(())
    }

    /// Read a block of memory from the remote device with chunking and retries.
    ///
    /// Uses GVCP READMEM format: 4-byte address + 2-byte reserved + 2-byte count.
    /// The acknowledgement carries: 4-byte address echo + data bytes.
    pub async fn read_mem(&mut self, addr: u64, len: usize) -> Result<Vec<u8>, GigeError> {
        let mut remaining = len;
        let mut offset = 0usize;
        let mut data = Vec::with_capacity(len);
        while remaining > 0 {
            let chunk = remaining.min(consts::GENCP_MAX_BLOCK);
            // GVCP requires the READMEM byte count to be a multiple of 4.
            // Strict cameras (e.g. Hikrobot) reject unaligned counts with
            // InvalidParameter, so request the aligned size and drop the
            // padding bytes below. Device memory regions are 4-byte aligned,
            // so reading past the end of e.g. the XML blob is safe.
            let request = chunk.next_multiple_of(4);
            let mut payload = BytesMut::with_capacity(8);
            payload.put_u32((addr + offset as u64) as u32);
            payload.put_u16(0); // reserved
            payload.put_u16(request as u16);
            let ack = self.transact_with_retry(OpCode::ReadMem, payload).await?;
            // GVCP READMEM_ACK: 4-byte address prefix + data.
            let ack_data = if ack.payload.len() >= 4 + request {
                &ack.payload[4..4 + request]
            } else if ack.payload.len() == request {
                // Some devices omit the address echo.
                &ack.payload[..request]
            } else {
                return Err(GigeError::AckLength {
                    command: "READMEM",
                    expected: request,
                    got: ack.payload.len(),
                });
            };
            data.extend_from_slice(&ack_data[..chunk]);
            remaining -= chunk;
            offset += chunk;
        }
        Ok(data)
    }

    /// Write a block of memory to the remote device with chunking and retries.
    ///
    /// Uses GVCP WRITEMEM format: 4-byte address + data bytes.
    /// The acknowledgement carries: 4-byte reserved (index).
    ///
    /// GVCP carries whole 32-bit words at 32-bit aligned addresses, as
    /// READMEM does (#35). A write that starts or ends inside a word — a
    /// one-byte register, a two-byte one at an odd half-word — goes out as
    /// the words around it, the bytes it does not cover read from the device
    /// first, so neighbouring registers keep their values (TC-32). aravis
    /// pads such a write with whatever its buffer held
    /// (`arv_gvcp_packet_new_write_memory_cmd`). If the read is refused, the
    /// write goes out as given, for the device to judge.
    pub async fn write_mem(&mut self, addr: u64, data: &[u8]) -> Result<(), GigeError> {
        let start = addr & !3;
        let end = (addr + data.len() as u64).next_multiple_of(4);
        if data.is_empty() || (start == addr && end == addr + data.len() as u64) {
            return self.write_mem_words(addr, data).await;
        }
        let mut words = match self.read_mem(start, (end - start) as usize).await {
            Ok(words) => words,
            Err(err) => {
                debug!(
                    addr = format!("0x{addr:08x}"),
                    len = data.len(),
                    error = %err,
                    "cannot read the words around an unaligned write; sending it as given"
                );
                return self.write_mem_words(addr, data).await;
            }
        };
        let offset = (addr - start) as usize;
        words[offset..offset + data.len()].copy_from_slice(data);
        self.write_mem_words(start, &words).await
    }

    /// WRITEMEM `data` at `addr` as it is, in chunks of at most 508 bytes.
    async fn write_mem_words(&mut self, addr: u64, data: &[u8]) -> Result<(), GigeError> {
        /// GVCP WRITEMEM overhead: 4-byte address prefix.
        const GVCP_WRITE_OVERHEAD: usize = 4;

        let mut offset = 0usize;
        while offset < data.len() {
            // Never zero: the loop runs while bytes remain, and a block holds
            // more than the address prefix.
            const _: () = assert!(consts::GENCP_MAX_BLOCK > GVCP_WRITE_OVERHEAD);
            let chunk = (data.len() - offset).min(consts::GENCP_MAX_BLOCK - GVCP_WRITE_OVERHEAD);
            let mut payload = BytesMut::with_capacity(GVCP_WRITE_OVERHEAD + chunk);
            payload.put_u32((addr + offset as u64) as u32);
            payload.extend_from_slice(&data[offset..offset + chunk]);
            let ack = self.transact_with_retry(OpCode::WriteMem, payload).await?;
            // GVCP WRITEMEM_ACK: 4-byte reserved payload.
            if ack.payload.len() > 4 {
                return Err(GigeError::AckLength {
                    command: "WRITEMEM",
                    expected: 4,
                    got: ack.payload.len(),
                });
            }
            offset += chunk;
        }
        Ok(())
    }

    /// Configure the message channel destination address/port.
    pub async fn set_message_destination(
        &mut self,
        ip: Ipv4Addr,
        port: u16,
    ) -> Result<(), GigeError> {
        info!(%ip, port, "configuring message channel destination");
        self.write_u32(consts::MESSAGE_DESTINATION_ADDRESS, u32::from(ip))
            .await?;
        // GevMCP is a 32-bit register with the port in the low half. Writing
        // only the two port bytes lands them in the *high* half.
        self.write_u32(consts::MESSAGE_DESTINATION_PORT, u32::from(port))
            .await?;
        Ok(())
    }

    fn stream_reg(channel: u32, offset: u64) -> u64 {
        consts::STREAM_CHANNEL_BASE + channel as u64 * consts::STREAM_CHANNEL_STRIDE + offset
    }

    /// Configure the GVSP host destination for the provided channel.
    pub async fn set_stream_destination(
        &mut self,
        channel: u32,
        ip: Ipv4Addr,
        port: u16,
    ) -> Result<(), GigeError> {
        info!(channel, %ip, port, "configuring stream destination");
        let addr = Self::stream_reg(channel, consts::STREAM_DESTINATION_ADDRESS);
        self.write_u32(addr, u32::from(ip)).await?;
        let addr = Self::stream_reg(channel, consts::STREAM_DESTINATION_PORT);
        self.write_u32(addr, u32::from(port)).await?;
        Ok(())
    }

    /// Read where stream channel `channel` sends: `GevSCDA` and the port in
    /// the low half of `GevSCPHostPort`. A multicast group here is how a
    /// camera streams to several hosts at once.
    pub async fn stream_destination(&mut self, channel: u32) -> Result<(Ipv4Addr, u16), GigeError> {
        let addr = Self::stream_reg(channel, consts::STREAM_DESTINATION_ADDRESS);
        let ip = Ipv4Addr::from(self.read_u32(addr).await?);
        let addr = Self::stream_reg(channel, consts::STREAM_DESTINATION_PORT);
        let port = (self.read_u32(addr).await? & 0xFFFF) as u16;
        Ok((ip, port))
    }

    /// Configure the packet size for the stream channel.
    ///
    /// Only bits 0-15 of `GevSCPSPacketSize` hold the size; the high bits are
    /// reserved for the fire-test-packet and do-not-fragment flags. A larger
    /// value is refused rather than truncated, because truncation is silent and
    /// arrives as a stream that never completes a frame — writing 70 000 would
    /// configure 4 464.
    pub async fn set_stream_packet_size(
        &mut self,
        channel: u32,
        packet_size: u32,
    ) -> Result<(), GigeError> {
        if packet_size > STREAM_PACKET_SIZE_MASK {
            return Err(GigeError::PacketSize(packet_size));
        }
        info!(channel, packet_size, "configuring stream packet size");
        let addr = Self::stream_reg(channel, consts::STREAM_PACKET_SIZE);
        self.write_u32(addr, packet_size).await
    }

    /// Ask the device to emit one GVSP test packet of `packet_size` bytes.
    ///
    /// Writes `GevSCPSPacketSize` with bit 31 (fire test packet) and bit 30
    /// (do not fragment) set. The size lands in the register as usual; the
    /// flags do not, and a device that echoed them back would report a packet
    /// size of over a billion, so [`GigeDevice::get_stream_packet_size`] masks.
    ///
    /// This does not wait for the packet — it arrives on the stream channel,
    /// which this type does not own. The caller listens.
    pub async fn request_test_packet(
        &mut self,
        channel: u32,
        packet_size: u32,
    ) -> Result<(), GigeError> {
        if packet_size > STREAM_PACKET_SIZE_MASK {
            return Err(GigeError::PacketSize(packet_size));
        }
        let addr = Self::stream_reg(channel, consts::STREAM_PACKET_SIZE);
        let word = packet_size | SCPS_FIRE_TEST_PACKET | SCPS_DO_NOT_FRAGMENT;
        debug!(channel, packet_size, "requesting GVSP test packet");
        self.write_u32(addr, word).await
    }

    /// Read `GevSCPSPacketSize` back and return the size the device holds.
    ///
    /// A device is free to clamp a requested packet size to what it supports,
    /// and the request succeeds when it does — nothing on the wire distinguishes
    /// "accepted" from "accepted and reduced". Until the receive path follows
    /// the *effective* value it reassembles at the wrong pitch and no frame ever
    /// completes ([#112](https://github.com/VitalyVorobyev/viva-genicam/issues/112),
    /// backlog SR-02).
    ///
    /// Deliberately a GVCP register read rather than a GenApi node read: the
    /// write side bypasses the `NodeMap`, so a cached `GevSCPSPacketSize` node
    /// can report the pre-write value and turn this check into a second bug.
    pub async fn get_stream_packet_size(&mut self, channel: u32) -> Result<u32, GigeError> {
        let addr = Self::stream_reg(channel, consts::STREAM_PACKET_SIZE);
        let raw = self.read_u32(addr).await?;
        Ok(raw & STREAM_PACKET_SIZE_MASK)
    }

    /// Read GVCP Capability (`0x0934`): which optional commands the device
    /// implements.
    pub async fn gvcp_capability(&mut self) -> Result<GvcpCapability, GigeError> {
        self.read_u32(consts::GVCP_CAPABILITY)
            .await
            .map(GvcpCapability::from_bits)
    }

    /// Switch `PENDING_ACK` on, if GVCP Capability says the device has it:
    /// set its enable bit (`0x2`) in GVCP Configuration (`0x0954`), keeping
    /// the others. Returns whether it is on.
    ///
    /// A device sends `PENDING_ACK` only once enabled. Without it, a command
    /// that takes longer than the acknowledgement timeout — a flash write, a
    /// mode change — is retried as though lost, and may run twice. A write,
    /// so it needs control privilege.
    pub async fn enable_pending_ack(&mut self) -> Result<bool, GigeError> {
        if !self.gvcp_capability().await?.pending_ack() {
            return Ok(false);
        }
        let config = self.read_u32(consts::GVCP_CONFIGURATION).await?;
        if config & consts::CONFIGURATION_PENDING_ACK_ENABLE == 0 {
            self.write_u32(
                consts::GVCP_CONFIGURATION,
                config | consts::CONFIGURATION_PENDING_ACK_ENABLE,
            )
            .await?;
        }
        debug!(addr = %self.remote, "PENDING_ACK enabled");
        Ok(true)
    }

    /// Read `GevNumberOfStreamChannels` (`0x0904`).
    pub async fn stream_channel_count(&mut self) -> Result<u32, GigeError> {
        self.read_u32(consts::NUMBER_OF_STREAM_CHANNELS).await
    }

    /// Read the timestamp tick frequency in hertz (`0x093C` high, `0x0940`
    /// low): the unit of device timestamps and of `GevSCPD`. A device without
    /// a timestamp clock reports 0.
    pub async fn timestamp_tick_frequency(&mut self) -> Result<u64, GigeError> {
        let high = self.read_u32(consts::TIMESTAMP_TICK_FREQUENCY_HIGH).await?;
        let low = self.read_u32(consts::TIMESTAMP_TICK_FREQUENCY_LOW).await?;
        Ok((u64::from(high) << 32) | u64::from(low))
    }

    /// Write `GevSCPD`, the delay between stream packets, in timestamp ticks
    /// (see [`GigeDevice::timestamp_tick_frequency`]).
    pub async fn set_stream_packet_delay(
        &mut self,
        channel: u32,
        packet_delay: u32,
    ) -> Result<(), GigeError> {
        debug!(channel, packet_delay, "configuring stream packet delay");
        let addr = Self::stream_reg(channel, consts::STREAM_PACKET_DELAY);
        self.write_u32(addr, packet_delay).await
    }

    /// Read the persistent IP configuration from the device.
    ///
    /// Returns `(ip, subnet, gateway)`.
    pub async fn read_persistent_ip(
        &mut self,
    ) -> Result<(Ipv4Addr, Ipv4Addr, Ipv4Addr), GigeError> {
        let ip = Ipv4Addr::from(self.read_u32(consts::PERSISTENT_IP_ADDRESS).await?);
        let subnet = Ipv4Addr::from(self.read_u32(consts::PERSISTENT_SUBNET_MASK).await?);
        let gateway = Ipv4Addr::from(self.read_u32(consts::PERSISTENT_DEFAULT_GATEWAY).await?);
        Ok((ip, subnet, gateway))
    }

    /// Write the persistent IP configuration to the device.
    pub async fn write_persistent_ip(
        &mut self,
        ip: Ipv4Addr,
        subnet: Ipv4Addr,
        gateway: Ipv4Addr,
    ) -> Result<(), GigeError> {
        self.write_u32(consts::PERSISTENT_IP_ADDRESS, u32::from(ip))
            .await?;
        self.write_u32(consts::PERSISTENT_SUBNET_MASK, u32::from(subnet))
            .await?;
        self.write_u32(consts::PERSISTENT_DEFAULT_GATEWAY, u32::from(gateway))
            .await?;
        info!(%ip, %subnet, %gateway, "wrote persistent IP configuration");
        Ok(())
    }

    /// Enable persistent IP mode in the device configuration flags.
    ///
    /// Sets bit 1 (persistent IP) in the `CurrentIPConfiguration` register.
    pub async fn enable_persistent_ip(&mut self) -> Result<(), GigeError> {
        let current = self.read_u32(consts::CURRENT_IP_CONFIG).await?;
        let updated = current | 0x02; // bit 1 = persistent IP
        self.write_u32(consts::CURRENT_IP_CONFIG, updated).await?;
        info!(config = format!("0x{updated:08x}"), "enabled persistent IP");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reply after the upstream default deadline and count queued retries.
    async fn delayed_control_ack(
        opcode: u16,
        payload: Vec<u8>,
        pending: bool,
    ) -> (SocketAddr, tokio::task::JoinHandle<usize>) {
        let socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
        let address = socket.local_addr().expect("address");
        let task = tokio::spawn(async move {
            let mut buffer = [0u8; 2048];
            let (_, peer) = socket.recv_from(&mut buffer).await.expect("request");
            assert_eq!(u16::from_be_bytes([buffer[2], buffer[3]]), opcode);
            let request_id = u16::from_be_bytes([buffer[6], buffer[7]]);
            if pending {
                socket
                    .send_to(&golden_pending_ack(request_id, 50), peer)
                    .await
                    .expect("pending ack");
            }
            time::sleep(Duration::from_millis(650)).await;
            let mut ack = Vec::new();
            ack.extend_from_slice(&0u16.to_be_bytes());
            ack.extend_from_slice(&(opcode + 1).to_be_bytes());
            ack.extend_from_slice(&(payload.len() as u16).to_be_bytes());
            ack.extend_from_slice(&request_id.to_be_bytes());
            ack.extend_from_slice(&payload);
            socket.send_to(&ack, peer).await.expect("ack");
            let mut requests = 1;
            while time::timeout(Duration::from_millis(50), socket.recv_from(&mut buffer))
                .await
                .is_ok()
            {
                requests += 1;
            }
            requests
        });
        (address, task)
    }

    #[tokio::test]
    async fn configured_control_timeout_covers_register_and_memory_reads_and_writes() {
        for opcode in [0x0080, 0x0082, 0x0084, 0x0086] {
            let value = 0xCAFEBABEu32.to_be_bytes();
            let payload = match opcode {
                0x0080 => value.to_vec(),
                0x0084 => [0x2000u32.to_be_bytes(), value].concat(),
                _ => 0u32.to_be_bytes().to_vec(),
            };
            // A short PENDING_ACK must not shorten the configured timeout.
            let (address, server) = delayed_control_ack(opcode, payload, opcode == 0x0086).await;
            let mut device = GigeDevice::open(address).await.expect("open");
            assert_eq!(device.control_timeout(), consts::CONTROL_TIMEOUT);
            device.set_control_timeout(Duration::from_secs(2));
            match opcode {
                0x0080 => assert_eq!(
                    device.read_register(0x2000).await.expect("read"),
                    0xCAFEBABE
                ),
                0x0082 => device.write_register(0x2000, 1).await.expect("write"),
                0x0084 => assert_eq!(device.read_mem(0x2000, 4).await.expect("read"), value),
                0x0086 => device.write_mem(0x2000, &value).await.expect("write"),
                _ => unreachable!(),
            }
            assert_eq!(server.await.expect("server"), 1, "request was resent");
        }
    }

    #[tokio::test]
    async fn a_short_configured_timeout_exhausts_the_existing_retry_budget() {
        // Keep the peer bound, but never acknowledge the command.
        let socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
        let mut device = GigeDevice::open(socket.local_addr().expect("address"))
            .await
            .expect("open");
        device.set_control_timeout(Duration::from_millis(20));
        let result = time::timeout(Duration::from_secs(1), device.read_register(0x2000))
            .await
            .expect("configured timeout was ignored");
        assert!(matches!(result, Err(GigeError::Timeout)));
        let mut buffer = [0u8; 2048];
        for _ in 0..consts::MAX_RETRIES {
            socket.try_recv(&mut buffer).expect("retry");
        }
        assert!(socket.try_recv(&mut buffer).is_err());
    }

    /// A Discovery ACK payload written out from the specification's field
    /// table, with a distinct recognisable value in every field.
    ///
    /// Built here rather than by round-tripping our own encoder: the point of
    /// this fixture is to disagree with the parser if the parser is wrong. See
    /// [ADR-0019]. The offsets are corroborated by Wireshark's
    /// `dissect_discovery_ack()`, which reads the MAC from `offset + 10` and
    /// the IP, manufacturer and model from 36, 72 and 104.
    ///
    /// [ADR-0019]: https://github.com/VitalyVorobyev/viva-genicam/blob/main/docs/adrs/adr0019-transport-conformance-and-spec-derived-fakes.md
    fn golden_discovery_payload() -> Vec<u8> {
        let mut p = vec![0u8; 248];
        p[0..2].copy_from_slice(&2u16.to_be_bytes()); // spec major
        p[2..4].copy_from_slice(&1u16.to_be_bytes()); // spec minor
        p[4..8].copy_from_slice(&0u32.to_be_bytes()); // device mode
        // 8..10 is the padding half of the MAC-high register.
        p[10..16].copy_from_slice(&[0x00, 0x0C, 0xDF, 0x06, 0x5B, 0x2F]); // MAC
        p[16..20].copy_from_slice(&7u32.to_be_bytes()); // supported IP config
        p[20..24].copy_from_slice(&5u32.to_be_bytes()); // current IP config
        p[36..40].copy_from_slice(&[169, 254, 78, 62]); // current IP
        p[52..56].copy_from_slice(&[255, 255, 0, 0]); // subnet
        p[68..72].copy_from_slice(&[169, 254, 0, 1]); // gateway
        let put =
            |p: &mut [u8], at: usize, s: &str| p[at..at + s.len()].copy_from_slice(s.as_bytes());
        put(&mut p, 72, "JAI Corporation"); // manufacturer
        put(&mut p, 104, "FS-3200T-10GE-NNC"); // model
        put(&mut p, 136, "1.2.3"); // device version
        put(&mut p, 168, "mfr-specific"); // manufacturer info
        put(&mut p, 216, "SN-12345"); // serial
        put(&mut p, 232, "left-camera"); // user-defined name
        p
    }

    #[test]
    fn discovery_payload_matches_spec_offsets() {
        let info = parse_discovery_payload(&golden_discovery_payload()).expect("parse");

        // The MAC begins at offset 10. Reading it at 12 — as we did before
        // #57 — yields DF:06:5B:2F:00:07, silently folding two bytes of
        // SupportedIPConfiguration into the address.
        assert_eq!(info.mac, [0x00, 0x0C, 0xDF, 0x06, 0x5B, 0x2F]);
        assert_eq!(info.mac_string(), "00:0C:DF:06:5B:2F");
        assert_eq!(info.ip, Ipv4Addr::new(169, 254, 78, 62));
        assert_eq!(info.manufacturer.as_deref(), Some("JAI Corporation"));
        assert_eq!(info.model.as_deref(), Some("FS-3200T-10GE-NNC"));
        assert_eq!(info.version.as_deref(), Some("1.2.3"));
        assert_eq!(info.serial.as_deref(), Some("SN-12345"));
        assert_eq!(info.user_name.as_deref(), Some("left-camera"));
        // TC-31: subnet at 52 and gateway at 68, which a camera on the wrong
        // network is diagnosed by.
        assert_eq!(info.subnet, Some(Ipv4Addr::new(255, 255, 0, 0)));
        assert_eq!(info.gateway, Some(Ipv4Addr::new(169, 254, 0, 1)));
    }

    /// TC-31: an interface asks its own subnet and the limited broadcast;
    /// only the second reaches a camera configured for another network.
    #[test]
    fn discovery_goes_to_the_subnet_and_to_the_limited_broadcast() {
        assert_eq!(
            discovery_destinations(
                Ipv4Addr::new(192, 168, 1, 5),
                Ipv4Addr::new(255, 255, 255, 0)
            ),
            vec![Ipv4Addr::new(192, 168, 1, 255), Ipv4Addr::BROADCAST]
        );
        assert_eq!(
            discovery_destinations(Ipv4Addr::LOCALHOST, Ipv4Addr::new(255, 0, 0, 0)),
            vec![Ipv4Addr::LOCALHOST],
            "loopback is asked by unicast"
        );
    }

    /// Subnet and gateway are compared both ways: the camera's address in the
    /// host's network and the host's in the camera's.
    #[test]
    fn a_camera_shares_the_host_subnet_only_by_both_masks() {
        let mut info = DeviceInfo::from_ip(Ipv4Addr::new(192, 168, 1, 20));
        assert_eq!(info.shares_subnet_with_host(), None, "nothing known");
        info.subnet = Some(Ipv4Addr::new(255, 255, 255, 0));
        info.local_ip = Some(Ipv4Addr::new(192, 168, 1, 5));
        info.local_netmask = Some(Ipv4Addr::new(255, 255, 255, 0));
        assert_eq!(info.shares_subnet_with_host(), Some(true));
        info.local_ip = Some(Ipv4Addr::new(169, 254, 3, 4));
        info.local_netmask = Some(Ipv4Addr::new(255, 255, 0, 0));
        assert_eq!(info.shares_subnet_with_host(), Some(false));
        // A /16 host holds a /24 camera's address, but the camera cannot
        // answer outside its /24.
        info.local_ip = Some(Ipv4Addr::new(192, 168, 9, 5));
        assert_eq!(info.shares_subnet_with_host(), Some(false));
    }

    #[test]
    fn discovery_payload_tolerates_truncation() {
        // A device that stops after the IP field still yields an addressable
        // camera rather than failing discovery on that interface.
        let short = golden_discovery_payload()[..40].to_vec();
        let info = parse_discovery_payload(&short).expect("short payload should parse");
        assert_eq!(info.ip, Ipv4Addr::new(169, 254, 78, 62));
        assert_eq!(info.mac, [0x00, 0x0C, 0xDF, 0x06, 0x5B, 0x2F]);
        assert_eq!(info.manufacturer, None);
        assert_eq!(info.serial, None);

        // Below the IP field there is nothing usable.
        assert!(parse_discovery_payload(&[0u8; 12]).is_err());
    }

    #[test]
    fn discovery_ack_ignores_foreign_traffic() {
        let payload = golden_discovery_payload();
        let ack = |status: u16, command: u16, request_id: u16| {
            let mut buf = Vec::new();
            buf.extend_from_slice(&status.to_be_bytes());
            buf.extend_from_slice(&command.to_be_bytes());
            buf.extend_from_slice(&(payload.len() as u16).to_be_bytes());
            buf.extend_from_slice(&request_id.to_be_bytes());
            buf.extend_from_slice(&payload);
            buf
        };

        // The real thing.
        assert!(parse_discovery_ack(&ack(0, consts::DISCOVERY_ACK, 0x0100), 0x0100).is_some());
        // Someone else's request id, a READREG ack that landed on our socket,
        // an error status, and a runt datagram must all be ignored rather than
        // failing discovery for every camera on the interface (#57).
        assert!(parse_discovery_ack(&ack(0, consts::DISCOVERY_ACK, 0x0999), 0x0100).is_none());
        assert!(parse_discovery_ack(&ack(0, 0x0081, 0x0100), 0x0100).is_none());
        assert!(parse_discovery_ack(&ack(0x8002, consts::DISCOVERY_ACK, 0x0100), 0x0100).is_none());
        assert!(parse_discovery_ack(&[0u8; 4], 0x0100).is_none());
    }

    #[test]
    fn directed_broadcast_uses_netmask_for_link_local_address() {
        // Linux reports no `brd` field for an address added as
        // `169.254.1.10/16`; `if-addrs` then exposes the local address as the
        // broadcast destination. Deriving it from the netmask must still
        // reach every APIPA peer on the link.
        assert_eq!(
            directed_broadcast(
                Ipv4Addr::new(169, 254, 1, 10),
                Ipv4Addr::new(255, 255, 0, 0),
            ),
            Ipv4Addr::new(169, 254, 255, 255)
        );
    }

    /// A PENDING_ACK written out from the specification's field table:
    /// the 8-byte acknowledgement header, two reserved bytes, then a 16-bit
    /// `time_to_completion` in milliseconds.
    fn golden_pending_ack(request_id: u16, millis: u16) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&0u16.to_be_bytes()); // status: success
        buf.extend_from_slice(&consts::PENDING_ACK.to_be_bytes()); // 0x0089
        buf.extend_from_slice(&4u16.to_be_bytes()); // payload length
        buf.extend_from_slice(&request_id.to_be_bytes());
        buf.extend_from_slice(&0u16.to_be_bytes()); // reserved
        buf.extend_from_slice(&millis.to_be_bytes()); // time to completion
        buf
    }

    #[test]
    fn pending_ack_matches_spec_offsets() {
        let (id, wait) = parse_pending_ack(&golden_pending_ack(0x1234, 750)).expect("pending ack");
        assert_eq!(id, 0x1234);
        assert_eq!(wait, Duration::from_millis(750));

        // The time is the u16 at payload offset 2, not a u32 over the whole
        // payload. The two readings agree only while the reserved field is
        // zero; a device that leaves junk there would ask aravis for
        // 0xDEAD_02EE ms — around 41 days — where we read 750.
        let mut junk = golden_pending_ack(0x1234, 750);
        junk[8..10].copy_from_slice(&0xDEADu16.to_be_bytes());
        assert_eq!(
            parse_pending_ack(&junk).expect("pending ack").1.as_millis(),
            750
        );
    }

    #[test]
    fn pending_ack_is_distinguished_from_real_acks() {
        // A genuine READREG ack must not be mistaken for a request for time,
        // or every register read would hang until the retry budget ran out.
        let mut readreg = Vec::new();
        readreg.extend_from_slice(&0u16.to_be_bytes());
        readreg.extend_from_slice(&0x0081u16.to_be_bytes());
        readreg.extend_from_slice(&4u16.to_be_bytes());
        readreg.extend_from_slice(&0x1234u16.to_be_bytes());
        readreg.extend_from_slice(&0u32.to_be_bytes());
        assert!(parse_pending_ack(&readreg).is_none());
        assert!(parse_pending_ack(&[0u8; 4]).is_none());

        // A device that truncates the payload is still asking for time.
        let truncated = &golden_pending_ack(0x1234, 750)[..viva_gencp::HEADER_SIZE];
        let (id, wait) = parse_pending_ack(truncated).expect("truncated pending ack");
        assert_eq!(id, 0x1234);
        assert_eq!(wait, consts::CONTROL_TIMEOUT);
    }

    /// A minimal GVCP device that answers `pending` PENDING_ACKs before the
    /// real acknowledgement, counting how many commands it was actually sent.
    ///
    /// The count is the point: a controller that treats PENDING_ACK as a
    /// failure and retries would execute the command more than once.
    async fn pending_ack_device(
        pending: usize,
        wait_ms: u16,
        stale_ack: bool,
    ) -> (SocketAddr, tokio::task::JoinHandle<usize>) {
        let sock = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
        let addr = sock.local_addr().expect("addr");
        let handle = tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            let mut commands = 0usize;
            let (len, peer) = sock.recv_from(&mut buf).await.expect("recv");
            commands += 1;
            let request_id = u16::from_be_bytes([buf[6], buf[7]]);
            debug_assert!(len >= viva_gencp::HEADER_SIZE);
            for _ in 0..pending {
                let ack = golden_pending_ack(request_id, wait_ms);
                sock.send_to(&ack, peer).await.expect("send pending");
            }
            if stale_ack {
                let mut ack = Vec::new();
                ack.extend_from_slice(&0u16.to_be_bytes());
                ack.extend_from_slice(&0x0081u16.to_be_bytes());
                ack.extend_from_slice(&4u16.to_be_bytes());
                ack.extend_from_slice(&request_id.wrapping_add(1).to_be_bytes());
                ack.extend_from_slice(&0xDEADBEEFu32.to_be_bytes());
                sock.send_to(&ack, peer).await.expect("send stale ack");
            }
            // The real READREG ack: one 4-byte register value.
            let mut ack = Vec::new();
            ack.extend_from_slice(&0u16.to_be_bytes());
            ack.extend_from_slice(&0x0081u16.to_be_bytes());
            ack.extend_from_slice(&4u16.to_be_bytes());
            ack.extend_from_slice(&request_id.to_be_bytes());
            ack.extend_from_slice(&0xCAFEBABEu32.to_be_bytes());
            sock.send_to(&ack, peer).await.expect("send ack");
            // Drain any retry the controller wrongly sent, so the count is
            // observable rather than lost in the socket buffer.
            let drain = time::timeout(Duration::from_millis(200), sock.recv_from(&mut buf)).await;
            if drain.is_ok() {
                commands += 1;
            }
            commands
        });
        (addr, handle)
    }

    #[tokio::test]
    async fn pending_ack_extends_the_deadline_without_resending() {
        let (addr, server) = pending_ack_device(1, 300, false).await;
        let mut device = GigeDevice::open(addr).await.expect("open");
        let value = device.read_register(0x0a00).await.expect("read register");
        assert_eq!(value, 0xCAFEBABE);
        assert_eq!(server.await.expect("join"), 1, "command must not be resent");
    }

    #[tokio::test]
    async fn repeated_pending_acks_are_all_honoured() {
        // A flash write can take several rounds. Each one restarts the clock;
        // the command is still sent exactly once.
        let (addr, server) = pending_ack_device(3, 200, false).await;
        let mut device = GigeDevice::open(addr).await.expect("open");
        let value = device.read_register(0x0a00).await.expect("read register");
        assert_eq!(value, 0xCAFEBABE);
        assert_eq!(server.await.expect("join"), 1, "command must not be resent");
    }

    #[tokio::test]
    async fn stale_acknowledgement_is_ignored_without_resending() {
        let (addr, server) = pending_ack_device(0, 0, true).await;
        let mut device = GigeDevice::open(addr).await.expect("open");
        let value = device.read_register(0x0a00).await.expect("read register");
        assert_eq!(value, 0xCAFEBABE);
        assert_eq!(server.await.expect("join"), 1, "command must not be resent");
    }

    /// A device that answers `error_replies` commands with `status_raw` and
    /// then, if it is asked again, succeeds. Returns the number of commands it
    /// received, which is what distinguishes "retried" from "reported".
    /// TC-26: while a `PENDING_ACK`ed write waits, the camera is briefly
    /// unreachable, so heartbeats draw ICMP port-unreachable and the socket
    /// reports errors. The write is still pending on the device: it must not
    /// be sent again, and the wait goes on to its acknowledgement.
    #[tokio::test]
    async fn an_unreachable_moment_during_a_pending_wait_does_not_resend_the_command() {
        let sock = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
        let addr = sock.local_addr().expect("addr");
        let server = tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            let (_, peer) = sock.recv_from(&mut buf).await.expect("the write");
            let request_id = u16::from_be_bytes([buf[6], buf[7]]);
            sock.send_to(&golden_pending_ack(request_id, 1500), peer)
                .await
                .expect("send pending");
            drop(sock);
            time::sleep(Duration::from_millis(200)).await;
            let sock = UdpSocket::bind(addr).await.expect("rebind");
            // Longer than an attempt's 500 ms timeout: a controller that
            // gave up waiting on the PENDING_ACK resends within it.
            let mut resent = 0usize;
            let listen_until = time::Instant::now() + Duration::from_millis(700);
            while let Ok(Ok((len, from))) =
                time::timeout_at(listen_until, sock.recv_from(&mut buf)).await
            {
                let opcode = u16::from_be_bytes([buf[2], buf[3]]);
                let id = u16::from_be_bytes([buf[6], buf[7]]);
                if opcode == 0x0082 && id == request_id {
                    resent += 1;
                } else if opcode == 0x0080 && len >= 12 {
                    // A heartbeat: answer it, as the device would.
                    let mut ack = vec![0, 0, 0, 0x81, 0, 4];
                    ack.extend_from_slice(&id.to_be_bytes());
                    ack.extend_from_slice(&2u32.to_be_bytes());
                    sock.send_to(&ack, from).await.expect("heartbeat ack");
                }
            }
            let mut ack = vec![0, 0, 0, 0x83, 0, 4];
            ack.extend_from_slice(&request_id.to_be_bytes());
            ack.extend_from_slice(&[0, 0, 0, 0]);
            sock.send_to(&ack, peer).await.expect("write ack");
            resent
        });
        let mut device = GigeDevice::open(addr).await.expect("open");
        device.set_inflight_keepalive_period(Some(Duration::from_millis(30)));
        device
            .write_register(0x2000, 1)
            .await
            .expect("the pending write completes");
        assert_eq!(server.await.unwrap(), 0, "the pending write was sent again");
    }

    async fn status_device(
        status_raw: u16,
        error_replies: usize,
    ) -> (SocketAddr, tokio::task::JoinHandle<usize>) {
        let sock = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
        let addr = sock.local_addr().expect("addr");
        let handle = tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            let mut commands = 0usize;
            loop {
                let recv =
                    time::timeout(Duration::from_millis(600), sock.recv_from(&mut buf)).await;
                let Ok(Ok((_len, peer))) = recv else { break };
                commands += 1;
                let request_id = u16::from_be_bytes([buf[6], buf[7]]);
                let mut ack = Vec::new();
                if commands <= error_replies {
                    ack.extend_from_slice(&status_raw.to_be_bytes());
                    ack.extend_from_slice(&0x0081u16.to_be_bytes());
                    ack.extend_from_slice(&0u16.to_be_bytes());
                    ack.extend_from_slice(&request_id.to_be_bytes());
                } else {
                    ack.extend_from_slice(&0u16.to_be_bytes());
                    ack.extend_from_slice(&0x0081u16.to_be_bytes());
                    ack.extend_from_slice(&4u16.to_be_bytes());
                    ack.extend_from_slice(&request_id.to_be_bytes());
                    ack.extend_from_slice(&0xCAFEBABEu32.to_be_bytes());
                }
                sock.send_to(&ack, peer).await.expect("send ack");
            }
            commands
        });
        (addr, handle)
    }

    #[tokio::test]
    async fn busy_is_retried() {
        // 0x8007 BUSY is congestion: the command can succeed if asked again.
        let (addr, server) = status_device(0x8007, 1).await;
        let mut device = GigeDevice::open(addr).await.expect("open");
        let value = device.read_register(0x0a00).await.expect("read register");
        assert_eq!(value, 0xCAFEBABE);
        assert_eq!(
            server.await.expect("join"),
            2,
            "BUSY must be retried, so the device sees a second command"
        );
    }

    #[tokio::test]
    async fn write_protect_is_reported_not_retried() {
        // 0x8004 WRITE_PROTECT is permanent. It used to be decoded as
        // `DeviceBusy`, so the retry loop spent its whole budget on a register
        // that could never accept the write.
        let (addr, server) = status_device(0x8004, usize::MAX).await;
        let mut device = GigeDevice::open(addr).await.expect("open");
        let err = device
            .read_register(0x0a00)
            .await
            .expect_err("write protect must surface");
        assert!(
            matches!(
                err,
                GigeError::Status(GigeStatus::Common(StatusCode::WriteProtect))
            ),
            "expected WRITE_PROTECT, got {err:?}"
        );
        assert_eq!(
            server.await.expect("join"),
            1,
            "a permanent refusal must not be retried"
        );
    }

    #[tokio::test]
    async fn access_denied_names_itself_in_the_error() {
        // The #45 case: the user saw `Unknown(32774)` and could tell us nothing.
        let (addr, server) = status_device(0x8006, usize::MAX).await;
        let mut device = GigeDevice::open(addr).await.expect("open");
        let err = device
            .read_register(0x0a00)
            .await
            .expect_err("access denied must surface");
        assert_eq!(
            err.to_string(),
            "device reported status ACCESS_DENIED (0x8006)"
        );
        assert_eq!(server.await.expect("join"), 1);
    }

    #[test]
    fn only_one_aligned_32_bit_word_is_a_register() {
        assert_eq!(single_register_address(0x0a00, 4), Some(0x0a00));
        assert_eq!(single_register_address(0xFFFF_FFFC, 4), Some(0xFFFF_FFFC));
        // Unaligned: the address stays READMEM, where TC-21 lives.
        assert_eq!(single_register_address(0x0a02, 4), None);
        // Not one register wide. Eight bytes is two registers, and is not
        // split: a multi-register READREG could tear a 64-bit value.
        assert_eq!(single_register_address(0x0a00, 8), None);
        assert_eq!(single_register_address(0x0a00, 2), None);
        assert_eq!(single_register_address(0x0a00, 0), None);
        // Beyond the 32-bit address a READREG can carry.
        assert_eq!(single_register_address(0x1_0000_0000, 4), None);
    }

    #[test]
    fn force_readmem_env_accepts_anything_but_empty_and_zero() {
        use std::ffi::OsStr;
        assert!(!force_readmem_requested(None));
        assert!(!force_readmem_requested(Some(OsStr::new(""))));
        assert!(!force_readmem_requested(Some(OsStr::new("0"))));
        assert!(force_readmem_requested(Some(OsStr::new("1"))));
        assert!(force_readmem_requested(Some(OsStr::new("true"))));
        // Only the exact string "0" means off; this is not a number parser.
        assert!(force_readmem_requested(Some(OsStr::new("00"))));
    }

    #[test]
    fn request_header_roundtrip() {
        let header = GvcpRequestHeader {
            flags: CommandFlags::ACK_REQUIRED,
            command: 0x1234,
            length: 4,
            request_id: 0xBEEF,
        };
        let payload = [1u8, 2, 3, 4];
        let encoded = header.encode(&payload);
        assert_eq!(encoded.len(), viva_gencp::HEADER_SIZE + payload.len());
        // GVCP wire format: byte 0 = 0x42 key, byte 1 = flags byte.
        assert_eq!(encoded[0], GVCP_CMD_KEY);
        assert_eq!(encoded[1], 0x01); // ACK_REQUIRED
        assert_eq!(&encoded[2..4], &header.command.to_be_bytes());
        assert_eq!(&encoded[4..6], &header.length.to_be_bytes());
        assert_eq!(&encoded[6..8], &header.request_id.to_be_bytes());
        assert_eq!(&encoded[8..], &payload);
    }

    #[test]
    fn forceip_payload_encoding() {
        let mac = [0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE];
        let ip = Ipv4Addr::new(192, 168, 1, 100);
        let subnet = Ipv4Addr::new(255, 255, 255, 0);
        let gateway = Ipv4Addr::new(192, 168, 1, 1);
        let payload = encode_forceip_payload(mac, ip, subnet, gateway);
        assert_eq!(payload.len(), 56);
        // MAC at offset 2..8
        assert_eq!(&payload[2..8], &mac);
        // IP at offset 20..24
        assert_eq!(&payload[20..24], &ip.octets());
        // Subnet at offset 36..40
        assert_eq!(&payload[36..40], &subnet.octets());
        // Gateway at offset 52..56
        assert_eq!(&payload[52..56], &gateway.octets());
        // Reserved bytes should be zero
        assert_eq!(&payload[0..2], &[0, 0]);
        assert_eq!(&payload[8..20], &[0u8; 12]);
        assert_eq!(&payload[24..36], &[0u8; 12]);
        assert_eq!(&payload[40..52], &[0u8; 12]);
    }

    #[test]
    fn ack_header_conversion() {
        let ack = AckHeader {
            status: 0x8007,
            opcode: OpCode::ReadMem,
            length: 12,
            request_id: 0x44,
        };
        let converted = GvcpAckHeader::from(ack);
        assert_eq!(converted.status, GigeStatus::Common(StatusCode::Busy));
        assert_eq!(converted.command, OpCode::ReadMem.ack_code());
        assert_eq!(converted.length, 12);
        assert_eq!(converted.request_id, 0x44);
    }

    /// The standard `PACKETRESEND_CMD`, written out byte by byte from the
    /// field table (Wireshark `dissect_packetresend_cmd`), not from a round
    /// trip through any parser of ours.
    #[test]
    fn packet_resend_has_the_12_byte_standard_layout() {
        #[rustfmt::skip]
        let expected: &[u8] = &[
            0x42, 0x00,             // key, flags: no acknowledgement, 16-bit ids
            0x00, 0x40,             // PACKETRESEND_CMD
            0x00, 0x0C,             // 12 bytes
            0x12, 0x34,             // request id
            0x00, 0x02,             // stream channel 2
            0xBE, 0xEF,             // block id
            0x00, 0x00, 0x00, 0x05, // first packet id
            0x00, 0x12, 0x34, 0x56, // last packet id
        ];
        assert_eq!(
            encode_packet_resend(0x1234, 2, 0xBEEF, 5, 0x12_3456, false).as_ref(),
            expected
        );
        // The fields hold 16 and 24 bits; nothing spills into the neighbours.
        let truncated = encode_packet_resend(1, 0, 0x1_0002, 0xFF00_0001, 0xFF00_0002, false);
        assert_eq!(&truncated[10..], &[0x00, 0x02, 0, 0, 0, 1, 0, 0, 0, 2]);
    }

    #[test]
    fn packet_resend_has_the_20_byte_extended_layout() {
        #[rustfmt::skip]
        let expected: &[u8] = &[
            0x42, 0x10,             // key, flags: extended ids, no acknowledgement
            0x00, 0x40,             // PACKETRESEND_CMD
            0x00, 0x14,             // 20 bytes
            0x00, 0x07,             // request id
            0x00, 0x01,             // stream channel 1
            0x00, 0x00,             // reserved
            0x01, 0x00, 0x00, 0x05, // first packet id, all 32 bits
            0x01, 0x00, 0x00, 0x09, // last packet id
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x02, // block id
        ];
        assert_eq!(
            encode_packet_resend(7, 1, (1 << 32) | 2, 0x0100_0005, 0x0100_0009, true).as_ref(),
            expected
        );
    }
}

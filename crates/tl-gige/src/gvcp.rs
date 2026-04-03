//! GVCP control plane utilities.

use std::collections::HashMap;
use std::io::Cursor;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use bytes::{Buf, BufMut, Bytes, BytesMut};
use fastrand::Rng;
use genicp::{decode_ack, AckHeader, CommandFlags, GenCpAck, OpCode, StatusCode};
use if_addrs::{get_if_addrs, IfAddr};
use thiserror::Error;
use tokio::net::UdpSocket;
use tokio::task::JoinSet;
use tokio::time;
use tracing::{debug, info, trace, warn};

use crate::nic::{self, Iface};

/// GVCP protocol constants grouped by semantic area.
pub mod consts {
    use std::time::Duration;

    /// GVCP control port as defined by the GigE Vision specification (section 7.3).
    pub const PORT: u16 = 3956;
    /// Opcode of the discovery command.
    pub const DISCOVERY_COMMAND: u16 = 0x0002;
    /// Opcode of the discovery acknowledgement.
    pub const DISCOVERY_ACK: u16 = 0x0003;
    /// Opcode for requesting packet resends.
    pub const PACKET_RESEND_COMMAND: u16 = 0x0040;
    /// Opcode of the packet resend acknowledgement.
    pub const PACKET_RESEND_ACK: u16 = 0x0041;

    /// Address of the Control Channel Privilege (CCP) register.
    ///
    /// A controller must write `CONTROL_PRIVILEGE` to this register before the
    /// device accepts stream configuration or acquisition commands.
    pub const CONTROL_CHANNEL_PRIVILEGE: u64 = 0x0a00;
    /// CCP value claiming exclusive control.
    pub const CCP_CONTROL: u32 = 1 << 1;

    /// Address of the SFNC `GevMessageChannel0DestinationAddress` register.
    pub const MESSAGE_DESTINATION_ADDRESS: u64 = 0x0900_0200;
    /// Address of the SFNC `GevMessageChannel0DestinationPort` register.
    pub const MESSAGE_DESTINATION_PORT: u64 = 0x0900_0204;
    /// Base address of the event notification mask (`GevEventNotificationAll`).
    pub const EVENT_NOTIFICATION_BASE: u64 = 0x0900_0300;
    /// Stride between successive event notification mask registers (bytes).
    pub const EVENT_NOTIFICATION_STRIDE: u64 = 4;

    /// Maximum number of bytes we read per GenCP `ReadMem` operation.
    pub const GENCP_MAX_BLOCK: usize = 512;
    /// Additional bytes that accompany a GenCP `WriteMem` block.
    pub const GENCP_WRITE_OVERHEAD: usize = 8;

    /// Default timeout for control transactions.
    pub const CONTROL_TIMEOUT: Duration = Duration::from_millis(500);
    /// Maximum number of automatic retries for a control transaction.
    pub const MAX_RETRIES: usize = 4;
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
}

/// Public alias for the GVCP well-known port.
pub use consts::PORT as GVCP_PORT;

/// GVCP request header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GvcpRequestHeader {
    /// Request flags (acknowledgement, broadcast).
    pub flags: CommandFlags,
    /// Raw command/opcode value.
    pub command: u16,
    /// Payload length in bytes.
    pub length: u16,
    /// Request identifier.
    pub request_id: u16,
}

/// GVCP command message key value (first byte of every GVCP command packet).
const GVCP_CMD_KEY: u8 = 0x42;

impl GvcpRequestHeader {
    /// Encode the header into a `Bytes` buffer ready to be transmitted.
    ///
    /// Uses proper GVCP wire format: byte 0 = `0x42` (command key),
    /// byte 1 = flags byte (bit 0 = ACK_REQUIRED, bit 4 = BROADCAST).
    pub fn encode(self, payload: &[u8]) -> Bytes {
        let mut buf = BytesMut::with_capacity(genicp::HEADER_SIZE + payload.len());
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
    fn gvcp_flags_byte(&self) -> u8 {
        let mut byte = 0u8;
        if self.flags.contains(CommandFlags::ACK_REQUIRED) {
            byte |= 0x01;
        }
        if self.flags.contains(CommandFlags::BROADCAST) {
            byte |= 0x10;
        }
        byte
    }
}

/// GVCP acknowledgement header wrapper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GvcpAckHeader {
    /// Status reported by the device.
    pub status: StatusCode,
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
            status: value.status,
            command: value.opcode.ack_code(),
            length: value.length,
            request_id: value.request_id,
        }
    }
}

/// Errors that can occur when operating the GVCP control path.
#[derive(Debug, Error)]
pub enum GigeError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("protocol: {0}")]
    Protocol(String),
    #[error("timeout waiting for acknowledgement")]
    Timeout,
    #[error("GenCP: {0}")]
    GenCp(#[from] genicp::GenCpError),
    #[error("device reported status {0:?}")]
    Status(StatusCode),
}

/// Information returned by GVCP discovery packets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub ip: Ipv4Addr,
    pub mac: [u8; 6],
    pub model: Option<String>,
    pub manufacturer: Option<String>,
}

impl DeviceInfo {
    fn mac_string(&self) -> String {
        self.mac
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<Vec<_>>()
            .join(":")
    }
}

/// Discover GigE Vision devices on the local network by broadcasting a GVCP discovery command.
pub async fn discover(timeout: Duration) -> Result<Vec<DeviceInfo>, GigeError> {
    discover_impl(timeout, None, false).await
}

/// Discover devices only on the specified interface name.
pub async fn discover_on_interface(
    timeout: Duration,
    interface: &str,
) -> Result<Vec<DeviceInfo>, GigeError> {
    discover_impl(timeout, Some(interface), false).await
}

/// Discover devices on all interfaces including loopback.
///
/// This is useful for testing with simulated cameras (e.g. `arv-fake-gv-camera`)
/// bound to `127.0.0.1`.
pub async fn discover_all(timeout: Duration) -> Result<Vec<DeviceInfo>, GigeError> {
    discover_impl(timeout, None, true).await
}

async fn discover_impl(
    timeout: Duration,
    iface_filter: Option<&str>,
    include_loopback: bool,
) -> Result<Vec<DeviceInfo>, GigeError> {
    let mut interfaces = Vec::new();
    for iface in get_if_addrs()? {
        let IfAddr::V4(v4) = iface.addr else {
            continue;
        };
        if !include_loopback && v4.ip.is_loopback() {
            continue;
        }
        if let Some(filter) = iface_filter {
            if iface.name != filter {
                continue;
            }
        }
        interfaces.push((iface.name, v4));
    }

    if interfaces.is_empty() {
        return Ok(Vec::new());
    }

    let mut join_set = JoinSet::new();
    for (idx, (name, v4)) in interfaces.into_iter().enumerate() {
        let request_id = 0x0100u16.wrapping_add(idx as u16);
        let interface_name = name.clone();
        join_set.spawn(async move {
            let local_addr = SocketAddr::new(IpAddr::V4(v4.ip), 0);
            let socket = UdpSocket::bind(local_addr).await?;
            // On loopback, broadcast is not supported on some platforms (macOS).
            // Send unicast discovery directly to the local address instead.
            let destination = if v4.ip.is_loopback() {
                SocketAddr::new(IpAddr::V4(v4.ip), consts::PORT)
            } else {
                socket.set_broadcast(true)?;
                let broadcast = v4.broadcast.unwrap_or(Ipv4Addr::BROADCAST);
                SocketAddr::new(IpAddr::V4(broadcast), consts::PORT)
            };

            let header = GvcpRequestHeader {
                flags: CommandFlags::ACK_REQUIRED | CommandFlags::BROADCAST,
                command: consts::DISCOVERY_COMMAND,
                length: 0,
                request_id,
            };
            let packet = header.encode(&[]);
            info!(%interface_name, local = %v4.ip, dest = %destination, "sending GVCP discovery");
            trace!(%interface_name, bytes = packet.len(), "GVCP discovery payload size");
            socket.send_to(&packet, destination).await?;

            let mut responses = Vec::new();
            let mut buffer = vec![0u8; consts::DISCOVERY_BUFFER];
            let timer = time::sleep(timeout);
            tokio::pin!(timer);
            loop {
                tokio::select! {
                    _ = &mut timer => break,
                    recv = socket.recv_from(&mut buffer) => {
                        let (len, src) = recv?;
                        info!(%interface_name, %src, "received GVCP response");
                        trace!(%interface_name, bytes = len, "GVCP response length");
                        if let Some(info) = parse_discovery_ack(&buffer[..len], request_id)? {
                            trace!(ip = %info.ip, mac = %info.mac_string(), "parsed discovery ack");
                            responses.push(info);
                        }
                    }
                }
            }
            Ok::<_, GigeError>(responses)
        });
    }

    let mut seen = HashMap::new();
    while let Some(res) = join_set.join_next().await {
        let devices =
            res.map_err(|e| GigeError::Protocol(format!("discovery task failed: {e}")))??;
        for dev in devices {
            seen.entry((dev.ip, dev.mac)).or_insert(dev);
        }
    }

    let mut devices: Vec<_> = seen.into_values().collect();
    devices.sort_by_key(|d| d.ip);
    Ok(devices)
}

fn parse_discovery_ack(buf: &[u8], expected_request: u16) -> Result<Option<DeviceInfo>, GigeError> {
    if buf.len() < genicp::HEADER_SIZE {
        return Err(GigeError::Protocol("GVCP ack too short".into()));
    }
    let mut header = buf;
    let status = header.get_u16();
    let command = header.get_u16();
    let length = header.get_u16() as usize;
    let request_id = header.get_u16();
    if request_id != expected_request {
        return Ok(None);
    }
    if command != consts::DISCOVERY_ACK {
        return Err(GigeError::Protocol(format!(
            "unexpected discovery opcode {command:#06x}"
        )));
    }
    if status != 0 {
        return Err(GigeError::Protocol(format!(
            "discovery returned status {status:#06x}"
        )));
    }
    if buf.len() < genicp::HEADER_SIZE + length {
        return Err(GigeError::Protocol("discovery payload truncated".into()));
    }
    let payload = &buf[genicp::HEADER_SIZE..genicp::HEADER_SIZE + length];
    let info = parse_discovery_payload(payload)?;
    Ok(Some(info))
}

/// Parse a GigE Vision Discovery ACK payload (248 bytes).
///
/// Field layout per GigE Vision specification (table 7-4):
///
/// | Offset | Size | Field                        |
/// |--------|------|------------------------------|
/// |      0 |    2 | Spec version major           |
/// |      2 |    2 | Spec version minor           |
/// |      4 |    4 | Device mode                  |
/// |      8 |    4 | Reserved                     |
/// |     12 |    2 | MAC address high             |
/// |     14 |    4 | MAC address low              |
/// |     18 |    4 | Supported IP config          |
/// |     22 |    4 | Current IP config            |
/// |     26 |   10 | Reserved                     |
/// |     36 |    4 | Current IP address           |
/// |     40 |   12 | Reserved                     |
/// |     52 |    4 | Current subnet mask          |
/// |     56 |   12 | Reserved                     |
/// |     68 |    4 | Default gateway              |
/// |     72 |   32 | Manufacturer name            |
/// |    106 |   32 | Model name                   |
/// |    138 |   32 | Device version               |
/// |    170 |   48 | Manufacturer specific info   |
/// |    218 |   16 | Serial number                |
/// |    234 |   16 | User defined name            |
fn parse_discovery_payload(payload: &[u8]) -> Result<DeviceInfo, GigeError> {
    // Minimum size to reach past the current IP field.
    if payload.len() < 40 {
        return Err(GigeError::Protocol("discovery payload too small".into()));
    }
    let mut cursor = Cursor::new(payload);
    let _spec_major = cursor.get_u16(); // 0
    let _spec_minor = cursor.get_u16(); // 2
    let _device_mode = cursor.get_u32(); // 4
    let _reserved = cursor.get_u32(); // 8

    // MAC: 2 bytes high + 4 bytes low = 6 bytes at offset 12.
    let mut mac = [0u8; 6];
    cursor.copy_to_slice(&mut mac); // 12..18

    let _supported_ip_config = cursor.get_u32(); // 18
    let _current_ip_config = cursor.get_u32(); // 22

    // 10 bytes reserved before current IP.
    cursor.advance(10); // 26..36
    let ip = Ipv4Addr::from(cursor.get_u32()); // 36

    // 12 bytes reserved before subnet.
    cursor.advance(12); // 40..52
    let _subnet = cursor.get_u32(); // 52

    // 12 bytes reserved before gateway.
    cursor.advance(12); // 56..68
    let _gateway = cursor.get_u32(); // 68

    // String fields.
    let manufacturer = read_fixed_string(&mut cursor, 32)?; // 72
    let model = read_fixed_string(&mut cursor, 32)?; // 104
                                                     // Remaining fields (version, info, serial, user name) are optional.

    Ok(DeviceInfo {
        ip,
        mac,
        manufacturer,
        model,
    })
}

fn read_fixed_string(cursor: &mut Cursor<&[u8]>, len: usize) -> Result<Option<String>, GigeError> {
    if cursor.remaining() < len {
        return Err(GigeError::Protocol("discovery string truncated".into()));
    }
    let mut buf = vec![0u8; len];
    cursor.copy_to_slice(&mut buf);
    Ok(parse_string(&buf))
}

fn parse_string(bytes: &[u8]) -> Option<String> {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    let slice = &bytes[..end];
    let s = String::from_utf8_lossy(slice).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// GVCP device handle.
pub struct GigeDevice {
    socket: UdpSocket,
    remote: SocketAddr,
    request_id: u16,
    rng: Rng,
}

/// Stream negotiation outcome describing the values written to the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamParams {
    /// Selected GVSP packet size (bytes).
    pub packet_size: u32,
    /// Packet delay expressed in GVSP clock ticks (80 ns units).
    pub packet_delay: u32,
    /// Link MTU used to derive the packet size.
    pub mtu: u32,
    /// Host IPv4 address configured on the device.
    pub host: Ipv4Addr,
    /// Host port configured on the device.
    pub port: u16,
}

impl GigeDevice {
    /// Connect to a device GVCP endpoint.
    ///
    /// The connection is ready for register read/write but does not claim
    /// control privilege. Call [`claim_control`] before configuring streaming
    /// or starting acquisition.
    pub async fn open(addr: SocketAddr) -> Result<Self, GigeError> {
        let local_ip = match addr.ip() {
            IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            IpAddr::V6(_) => {
                return Err(GigeError::Protocol("IPv6 GVCP is not supported".into()));
            }
        };
        let socket = UdpSocket::bind(SocketAddr::new(local_ip, 0)).await?;
        socket.connect(addr).await?;
        Ok(Self {
            socket,
            remote: addr,
            request_id: 1,
            rng: Rng::new(),
        })
    }

    /// Claim control channel privilege (CCP).
    ///
    /// Required by the GigE Vision specification before the device accepts
    /// stream configuration or acquisition commands.
    pub async fn claim_control(&mut self) -> Result<(), GigeError> {
        self.write_mem(
            consts::CONTROL_CHANNEL_PRIVILEGE,
            &consts::CCP_CONTROL.to_be_bytes(),
        )
        .await?;
        debug!(addr = %self.remote, "claimed control channel privilege");
        Ok(())
    }

    /// Release control channel privilege.
    pub async fn release_control(&mut self) -> Result<(), GigeError> {
        self.write_mem(consts::CONTROL_CHANNEL_PRIVILEGE, &0u32.to_be_bytes())
            .await
    }

    /// Return the remote GVCP socket address associated with this device.
    pub fn remote_addr(&self) -> SocketAddr {
        self.remote
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
        let mut attempt = 0usize;
        let mut payload = payload;
        loop {
            attempt += 1;
            let request_id = self.next_request_id();
            let payload_bytes = payload.clone().freeze();
            let header = GvcpRequestHeader {
                flags: CommandFlags::ACK_REQUIRED,
                command: opcode.command_code(),
                length: payload_bytes.len() as u16,
                request_id,
            };
            let encoded = header.encode(&payload_bytes);
            trace!(request_id, opcode = ?opcode, bytes = encoded.len(), attempt, "sending GVCP command");
            if let Err(err) = self.socket.send(&encoded).await {
                if attempt >= consts::MAX_RETRIES {
                    return Err(err.into());
                }
                warn!(request_id, ?opcode, attempt, "send failed, retrying");
                self.backoff(attempt).await;
                payload = BytesMut::from(&payload_bytes[..]);
                continue;
            }

            let mut buf =
                vec![
                    0u8;
                    genicp::HEADER_SIZE + consts::GENCP_MAX_BLOCK + consts::GENCP_WRITE_OVERHEAD
                ];
            match time::timeout(consts::CONTROL_TIMEOUT, self.socket.recv(&mut buf)).await {
                Ok(Ok(len)) => {
                    trace!(request_id, bytes = len, attempt, "received GenCP ack");
                    let ack = decode_ack(&buf[..len])?;
                    if ack.header.request_id != request_id {
                        debug!(
                            request_id,
                            got = ack.header.request_id,
                            attempt,
                            "acknowledgement id mismatch"
                        );
                        if attempt >= consts::MAX_RETRIES {
                            return Err(GigeError::Protocol("acknowledgement id mismatch".into()));
                        }
                        self.backoff(attempt).await;
                        payload = BytesMut::from(&payload_bytes[..]);
                        continue;
                    }
                    if ack.header.opcode != opcode {
                        return Err(GigeError::Protocol(
                            "unexpected opcode in acknowledgement".into(),
                        ));
                    }
                    match ack.header.status {
                        StatusCode::Success => return Ok(ack),
                        StatusCode::DeviceBusy if attempt < consts::MAX_RETRIES => {
                            warn!(request_id, attempt, "device busy, retrying");
                            self.backoff(attempt).await;
                            payload = BytesMut::from(&payload_bytes[..]);
                            continue;
                        }
                        other => return Err(GigeError::Status(other)),
                    }
                }
                Ok(Err(err)) => {
                    if attempt >= consts::MAX_RETRIES {
                        return Err(err.into());
                    }
                    warn!(request_id, ?opcode, attempt, "receive error, retrying");
                    self.backoff(attempt).await;
                    payload = BytesMut::from(&payload_bytes[..]);
                }
                Err(_) => {
                    if attempt >= consts::MAX_RETRIES {
                        return Err(GigeError::Timeout);
                    }
                    warn!(request_id, ?opcode, attempt, "command timeout, retrying");
                    self.backoff(attempt).await;
                    payload = BytesMut::from(&payload_bytes[..]);
                }
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
            let mut payload = BytesMut::with_capacity(8);
            payload.put_u32((addr + offset as u64) as u32);
            payload.put_u16(0); // reserved
            payload.put_u16(chunk as u16);
            let ack = self.transact_with_retry(OpCode::ReadMem, payload).await?;
            // GVCP READMEM_ACK: 4-byte address prefix + data.
            let ack_data = if ack.payload.len() >= 4 + chunk {
                &ack.payload[4..4 + chunk]
            } else if ack.payload.len() == chunk {
                // Some devices omit the address echo.
                &ack.payload[..chunk]
            } else {
                return Err(GigeError::Protocol(format!(
                    "expected {} bytes but device returned {}",
                    chunk,
                    ack.payload.len()
                )));
            };
            data.extend_from_slice(ack_data);
            remaining -= chunk;
            offset += chunk;
        }
        Ok(data)
    }

    /// Write a block of memory to the remote device with chunking and retries.
    ///
    /// Uses GVCP WRITEMEM format: 4-byte address + data bytes.
    /// The acknowledgement carries: 4-byte reserved (index).
    pub async fn write_mem(&mut self, addr: u64, data: &[u8]) -> Result<(), GigeError> {
        /// GVCP WRITEMEM overhead: 4-byte address prefix.
        const GVCP_WRITE_OVERHEAD: usize = 4;

        let mut offset = 0usize;
        while offset < data.len() {
            let chunk = (data.len() - offset).min(consts::GENCP_MAX_BLOCK - GVCP_WRITE_OVERHEAD);
            if chunk == 0 {
                return Err(GigeError::Protocol("write chunk size is zero".into()));
            }
            let mut payload = BytesMut::with_capacity(GVCP_WRITE_OVERHEAD + chunk);
            payload.put_u32((addr + offset as u64) as u32);
            payload.extend_from_slice(&data[offset..offset + chunk]);
            let ack = self.transact_with_retry(OpCode::WriteMem, payload).await?;
            // GVCP WRITEMEM_ACK: 4-byte reserved payload.
            if ack.payload.len() > 4 {
                return Err(GigeError::Protocol(
                    "write acknowledgement carried unexpected payload".into(),
                ));
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
        self.write_mem(consts::MESSAGE_DESTINATION_ADDRESS, &ip.octets())
            .await?;
        self.write_mem(consts::MESSAGE_DESTINATION_PORT, &port.to_be_bytes())
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
        self.write_mem(addr, &ip.octets()).await?;
        let addr = Self::stream_reg(channel, consts::STREAM_DESTINATION_PORT);
        self.write_mem(addr, &(port as u32).to_be_bytes()).await?;
        Ok(())
    }

    /// Configure the packet size for the stream channel.
    pub async fn set_stream_packet_size(
        &mut self,
        channel: u32,
        packet_size: u32,
    ) -> Result<(), GigeError> {
        info!(channel, packet_size, "configuring stream packet size");
        let addr = Self::stream_reg(channel, consts::STREAM_PACKET_SIZE);
        self.write_mem(addr, &packet_size.to_be_bytes()).await
    }

    /// Configure the packet delay (`GevSCPD`).
    pub async fn set_stream_packet_delay(
        &mut self,
        channel: u32,
        packet_delay: u32,
    ) -> Result<(), GigeError> {
        debug!(channel, packet_delay, "configuring stream packet delay");
        let addr = Self::stream_reg(channel, consts::STREAM_PACKET_DELAY);
        self.write_mem(addr, &packet_delay.to_be_bytes()).await
    }

    /// Negotiate GVSP parameters with the device given the host interface.
    pub async fn negotiate_stream(
        &mut self,
        channel: u32,
        iface: &Iface,
        port: u16,
        target_mtu: Option<u32>,
    ) -> Result<StreamParams, GigeError> {
        let host_ip = iface
            .ipv4()
            .ok_or_else(|| GigeError::Protocol("interface lacks IPv4 address".into()))?;
        let iface_mtu = nic::mtu(iface)?;
        let mtu = target_mtu.map_or(iface_mtu, |limit| limit.min(iface_mtu));
        let packet_size = nic::best_packet_size(mtu);
        let packet_delay = if mtu <= 1500 {
            // When jumbo frames are unavailable we space out packets by 2 µs to
            // prevent excessive buffering pressure on receivers. GVSP expresses
            // `GevSCPD` in units of 80 ns.
            const DELAY_NS: u32 = 2_000; // 2 µs.
            DELAY_NS / 80
        } else {
            0
        };

        self.set_stream_destination(channel, host_ip, port).await?;
        self.set_stream_packet_size(channel, packet_size).await?;
        self.set_stream_packet_delay(channel, packet_delay).await?;

        Ok(StreamParams {
            packet_size,
            packet_delay,
            mtu,
            host: host_ip,
            port,
        })
    }

    /// Enable or disable delivery of the provided event identifier.
    pub async fn enable_event_raw(&mut self, id: u16, on: bool) -> Result<(), GigeError> {
        let index = (id / 32) as u64;
        let bit = 1u32 << (id % 32);
        let addr = consts::EVENT_NOTIFICATION_BASE + index * consts::EVENT_NOTIFICATION_STRIDE;
        let current = self.read_mem(addr, 4).await?;
        if current.len() != 4 {
            return Err(GigeError::Protocol(
                "event notification register length mismatch".into(),
            ));
        }
        let mut bytes = [0u8; 4];
        bytes.copy_from_slice(&current);
        let mut value = u32::from_be_bytes(bytes);
        if on {
            value |= bit;
        } else {
            value &= !bit;
        }
        let new_bytes = value.to_be_bytes();
        self.write_mem(addr, &new_bytes).await?;
        debug!(event_id = id, enabled = on, "updated event mask");
        Ok(())
    }

    /// Request resend of a packet range for the provided block identifier.
    pub async fn request_resend(
        &mut self,
        block_id: u16,
        first_packet: u16,
        last_packet: u16,
    ) -> Result<(), GigeError> {
        let mut payload = BytesMut::with_capacity(8);
        payload.put_u16(block_id);
        payload.put_u16(0); // Reserved as per spec.
        payload.put_u16(first_packet);
        payload.put_u16(last_packet);

        let request_id = self.next_request_id();
        let header = GvcpRequestHeader {
            flags: CommandFlags::ACK_REQUIRED,
            command: consts::PACKET_RESEND_COMMAND,
            length: payload.len() as u16,
            request_id,
        };
        let packet = header.encode(&payload);
        trace!(
            block_id,
            first_packet,
            last_packet,
            request_id,
            "sending packet resend request"
        );
        self.socket.send(&packet).await?;
        let mut buf = [0u8; genicp::HEADER_SIZE];
        match time::timeout(consts::CONTROL_TIMEOUT, self.socket.recv(&mut buf)).await {
            Ok(Ok(len)) => {
                if len != genicp::HEADER_SIZE {
                    return Err(GigeError::Protocol("resend ack length mismatch".into()));
                }
                let mut cursor = &buf[..];
                let status = StatusCode::from_raw(cursor.get_u16());
                let command = cursor.get_u16();
                let length = cursor.get_u16();
                let ack_request_id = cursor.get_u16();
                if command != consts::PACKET_RESEND_ACK {
                    return Err(GigeError::Protocol("unexpected resend ack opcode".into()));
                }
                if length != 0 {
                    return Err(GigeError::Protocol("resend ack carried payload".into()));
                }
                if ack_request_id != request_id {
                    return Err(GigeError::Protocol("resend ack request id mismatch".into()));
                }
                if status != StatusCode::Success {
                    return Err(GigeError::Status(status));
                }
                Ok(())
            }
            Ok(Err(err)) => Err(err.into()),
            Err(_) => Err(GigeError::Timeout),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(encoded.len(), genicp::HEADER_SIZE + payload.len());
        // GVCP wire format: byte 0 = 0x42 key, byte 1 = flags byte.
        assert_eq!(encoded[0], GVCP_CMD_KEY);
        assert_eq!(encoded[1], 0x01); // ACK_REQUIRED
        assert_eq!(&encoded[2..4], &header.command.to_be_bytes());
        assert_eq!(&encoded[4..6], &header.length.to_be_bytes());
        assert_eq!(&encoded[6..8], &header.request_id.to_be_bytes());
        assert_eq!(&encoded[8..], &payload);
    }

    #[test]
    fn ack_header_conversion() {
        let ack = AckHeader {
            status: StatusCode::DeviceBusy,
            opcode: OpCode::ReadMem,
            length: 12,
            request_id: 0x44,
        };
        let converted = GvcpAckHeader::from(ack);
        assert_eq!(converted.status, StatusCode::DeviceBusy);
        assert_eq!(converted.command, OpCode::ReadMem.ack_code());
        assert_eq!(converted.length, 12);
        assert_eq!(converted.request_id, 0x44);
    }
}

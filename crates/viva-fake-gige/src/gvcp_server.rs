//! GVCP control channel server: discovery + GenCP register read/write.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use bytes::{BufMut, BytesMut};
use tokio::net::UdpSocket;
use tokio::sync::{Mutex, Notify};
use tracing::{debug, trace, warn};

use crate::registers::RegisterMap;

/// GVCP command key byte (first byte of every GVCP command).
const GVCP_CMD_KEY: u8 = 0x42;

// GVCP command opcodes
const DISCOVERY_CMD: u16 = 0x0002;
const FORCEIP_CMD: u16 = 0x0004;
const FORCEIP_ACK: u16 = 0x0005;
const READREG_CMD: u16 = 0x0080;
const WRITEREG_CMD: u16 = 0x0082;
const READMEM_CMD: u16 = 0x0084;
const WRITEMEM_CMD: u16 = 0x0086;
/// Action command. Deliberately spelled out rather than reused from
/// `viva-gige`: the fake must be able to disagree with the client, which is the
/// whole point of asserting its bytes independently (ADR-0019).
const ACTION_CMD: u16 = 0x0100;

// GVCP ack opcodes
const DISCOVERY_ACK: u16 = 0x0003;
const READREG_ACK: u16 = 0x0081;
const WRITEREG_ACK: u16 = 0x0083;
const READMEM_ACK: u16 = 0x0085;
const WRITEMEM_ACK: u16 = 0x0087;
const ACTION_ACK: u16 = 0x0101;

/// Action keys the fake accepts. A command whose keys do not match is ignored
/// without an acknowledgement, which is how a real device behaves — the command
/// is broadcast to every camera on the subnet and only the addressed group acts.
pub const FAKE_DEVICE_KEY: u32 = 0x0000_0042;
/// Group key the fake belongs to.
pub const FAKE_GROUP_KEY: u32 = 0x0000_0001;
/// Group mask bits the fake responds to.
pub const FAKE_GROUP_MASK: u32 = 0x0000_0001;

/// MAC address the fake camera reports. Public so tests can assert the exact
/// bytes rather than merely that a MAC is present (ADR-0019).
pub const FAKE_MAC: [u8; 6] = [0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE];
/// Manufacturer name reported in the Discovery ACK.
pub const FAKE_MANUFACTURER: &str = "viva-genicam";
/// Model name reported in the Discovery ACK.
pub const FAKE_MODEL: &str = "FakeGigE";
/// Device version reported in the Discovery ACK.
pub const FAKE_VERSION: &str = "1.0.0";
/// Serial number reported in the Discovery ACK.
pub const FAKE_SERIAL: &str = "FAKE-001";
/// User-defined name reported in the Discovery ACK.
pub const FAKE_USER_NAME: &str = "FakeCamera";

/// Status code for success.
const STATUS_SUCCESS: u16 = 0x0000;
/// GigE Vision `GEV_STATUS_NOT_IMPLEMENTED`.
const STATUS_NOT_IMPLEMENTED: u16 = 0x8001;
/// GigE Vision `GEV_STATUS_INVALID_PARAMETER`.
const STATUS_INVALID_PARAMETER: u16 = 0x8002;
/// GigE Vision `GEV_STATUS_BAD_ALIGNMENT`.
const STATUS_BAD_ALIGNMENT: u16 = 0x8005;
/// GigE Vision `GEV_STATUS_ACCESS_DENIED`.
const STATUS_ACCESS_DENIED: u16 = 0x8006;

/// The four GVCP register-access commands, as the fake counts them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GvcpCommand {
    /// `READREG_CMD` (0x0080).
    ReadReg,
    /// `WRITEREG_CMD` (0x0082).
    WriteReg,
    /// `READMEM_CMD` (0x0084).
    ReadMem,
    /// `WRITEMEM_CMD` (0x0086).
    WriteMem,
}

/// How the fake refuses READREG and WRITEREG when told to.
///
/// Both are shapes a non-conformant device can take, and a controller has to
/// survive either: one answers with a status it can act on, the other leaves
/// it waiting out its retry budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterCommandRefusal {
    /// Acknowledge with `NOT_IMPLEMENTED` (0x8001) and do nothing.
    NotImplemented,
    /// Drop the command without any acknowledgement.
    NoReply,
}

/// Per-command, per-address count of register-access commands received.
///
/// Counted on receipt, before the command is served or refused: the question a
/// test asks is which command left the host, and a refused command did leave
/// it. Without this nothing could assert *how* a value reached the device, only
/// that it did — and READMEM and READREG deliver identical bytes.
#[derive(Debug, Default)]
pub struct CommandCounters {
    counts: std::sync::Mutex<HashMap<(GvcpCommand, u64), u64>>,
}

impl CommandCounters {
    fn record(&self, command: GvcpCommand, addr: u64) {
        if let Ok(mut counts) = self.counts.lock() {
            *counts.entry((command, addr)).or_default() += 1;
        }
    }

    /// How many `command`s have been received, at any address.
    pub fn total(&self, command: GvcpCommand) -> u64 {
        self.counts.lock().map_or(0, |counts| {
            counts
                .iter()
                .filter(|((cmd, _), _)| *cmd == command)
                .map(|(_, n)| n)
                .sum()
        })
    }

    /// How many `command`s have been received for `addr`.
    ///
    /// A READMEM or WRITEMEM counts at its start address; a multi-register
    /// READREG or WRITEREG counts once at each address it names.
    pub fn at(&self, command: GvcpCommand, addr: u64) -> u64 {
        self.counts.lock().map_or(0, |counts| {
            counts.get(&(command, addr)).copied().unwrap_or(0)
        })
    }

    /// Forget every count so far.
    pub fn reset(&self) {
        if let Ok(mut counts) = self.counts.lock() {
            counts.clear();
        }
    }
}

/// Behaviour of the GVCP server beyond the register store.
#[derive(Debug, Default)]
pub struct ServerOptions {
    /// Counts of every register-access command received.
    pub counters: Arc<CommandCounters>,
    /// Refuse READREG/WRITEREG in this way, if set.
    pub refuse_register_commands: Option<RegisterCommandRefusal>,
}

/// Run the GVCP control server loop.
///
/// Listens for GVCP commands and sends appropriate responses.
/// Notifies `acq_notify` when AcquisitionStart is written.
pub async fn run(
    socket: Arc<UdpSocket>,
    regs: Arc<Mutex<RegisterMap>>,
    acq_start_notify: Arc<Notify>,
    acq_stop_flag: Arc<AtomicBool>,
    bind_ip: std::net::Ipv4Addr,
    options: ServerOptions,
) {
    let mut buf = [0u8; 2048];
    loop {
        let (len, peer) = match socket.recv_from(&mut buf).await {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, "GVCP recv error");
                continue;
            }
        };
        let pkt = &buf[..len];
        if len < 8 || pkt[0] != GVCP_CMD_KEY {
            trace!(len, "ignoring non-GVCP packet");
            continue;
        }

        let flags = pkt[1];
        let command = u16::from_be_bytes([pkt[2], pkt[3]]);
        let _length = u16::from_be_bytes([pkt[4], pkt[5]]);
        let request_id = u16::from_be_bytes([pkt[6], pkt[7]]);
        let payload = &pkt[8..];

        // Register access refreshes the device's heartbeat timer. Discovery and
        // FORCEIP do not: they are broadcast by any application on the subnet.
        // The command is still served after an expiry — a real device answers
        // register reads from a non-controller, it only refuses configuration.
        if matches!(
            command,
            READREG_CMD | WRITEREG_CMD | READMEM_CMD | WRITEMEM_CMD
        ) && regs.lock().await.note_register_command()
        {
            warn!(%peer, "heartbeat expired; control privilege released");
        }

        record_command(&options.counters, command, payload);

        if matches!(command, READREG_CMD | WRITEREG_CMD)
            && let Some(refusal) = options.refuse_register_commands
        {
            if refusal == RegisterCommandRefusal::NotImplemented {
                let ack = if command == READREG_CMD {
                    READREG_ACK
                } else {
                    WRITEREG_ACK
                };
                let resp = build_error_ack(ack, request_id, STATUS_NOT_IMPLEMENTED);
                let _ = socket.send_to(&resp, peer).await;
            }
            debug!(%peer, command, ?refusal, "register command refused");
            continue;
        }

        match command {
            DISCOVERY_CMD => {
                let resp = build_discovery_ack(request_id, bind_ip);
                let _ = socket.send_to(&resp, peer).await;
                debug!(%peer, "discovery response sent");
            }
            FORCEIP_CMD => {
                handle_forceip(&socket, peer, request_id, payload, bind_ip).await;
            }
            READREG_CMD => {
                handle_readreg(&socket, peer, request_id, payload, &regs).await;
            }
            WRITEREG_CMD => {
                handle_writereg(
                    &socket,
                    peer,
                    request_id,
                    payload,
                    &regs,
                    &acq_start_notify,
                    &acq_stop_flag,
                )
                .await;
            }
            READMEM_CMD => {
                handle_readmem(&socket, peer, request_id, payload, &regs).await;
            }
            WRITEMEM_CMD => {
                handle_writemem(
                    &socket,
                    peer,
                    request_id,
                    payload,
                    &regs,
                    &acq_start_notify,
                    &acq_stop_flag,
                )
                .await;
            }
            ACTION_CMD => {
                handle_action(&socket, peer, request_id, flags, payload).await;
            }
            _ => {
                debug!(command, "unsupported GVCP command");
            }
        }
    }
}

/// Build a 256-byte discovery ACK payload (GVCP header + device info).
fn build_discovery_ack(request_id: u16, ip: std::net::Ipv4Addr) -> Vec<u8> {
    // Discovery ack payload is 248 bytes (as defined by the GigE Vision spec).
    let payload_len: u16 = 248;
    let mut buf = BytesMut::with_capacity(8 + payload_len as usize);

    // ACK header: status(2) + ack_cmd(2) + length(2) + request_id(2)
    buf.put_u16(STATUS_SUCCESS);
    buf.put_u16(DISCOVERY_ACK);
    buf.put_u16(payload_len);
    buf.put_u16(request_id);

    // Discovery payload (248 bytes). Offsets below are payload-relative and
    // come from the specification's field table, NOT from what our own parser
    // happens to read — see ADR-0019. This layout previously placed the MAC at
    // offset 12 because `parse_discovery_payload` read it there; both were
    // wrong, and agreeing with each other is what hid it (#57).
    buf.put_u16(2); // 0   Spec major version
    buf.put_u16(0); // 2   Spec minor version
    buf.put_u32(0); // 4   Device mode

    // 8   Reserved: the padding half of the MAC-high register, 2 bytes only.
    buf.put_slice(&[0u8; 2]);

    // 10  MAC address (6 bytes): fake MAC DE:AD:BE:EF:CA:FE
    buf.put_slice(&FAKE_MAC);

    buf.put_u32(0x0000_0007); // 16  Supported IP config (DHCP + persistent + LLA)
    buf.put_u32(0x0000_0005); // 20  Current IP config

    // 24  Reserved (12 bytes)
    buf.put_slice(&[0u8; 12]);

    // 36  Current IP address
    buf.put_slice(&ip.octets());

    // 40  Reserved (12 bytes)
    buf.put_slice(&[0u8; 12]);

    // 52  Subnet mask (255.255.255.0)
    buf.put_slice(&[255, 255, 255, 0]);

    // 56  Reserved (12 bytes)
    buf.put_slice(&[0u8; 12]);

    // 68  Gateway
    buf.put_slice(&[0, 0, 0, 0]);

    // Manufacturer name (32 bytes)
    put_fixed_string(&mut buf, FAKE_MANUFACTURER, 32); // 72
    // Model name (32 bytes)
    put_fixed_string(&mut buf, FAKE_MODEL, 32); // 104
    // Device version (32 bytes)
    put_fixed_string(&mut buf, FAKE_VERSION, 32); // 136
    // Manufacturer specific info (48 bytes)
    put_fixed_string(&mut buf, "Fake camera for testing", 48); // 168
    // Serial number (16 bytes)
    put_fixed_string(&mut buf, FAKE_SERIAL, 16); // 216
    // User defined name (16 bytes)
    put_fixed_string(&mut buf, FAKE_USER_NAME, 16); // 232

    buf.to_vec()
}

fn put_fixed_string(buf: &mut BytesMut, s: &str, len: usize) {
    let bytes = s.as_bytes();
    let copy_len = bytes.len().min(len);
    buf.put_slice(&bytes[..copy_len]);
    for _ in copy_len..len {
        buf.put_u8(0);
    }
}

/// Build a generic GVCP ACK header + payload.
fn build_ack(ack_cmd: u16, request_id: u16, payload: &[u8]) -> Vec<u8> {
    let mut buf = BytesMut::with_capacity(8 + payload.len());
    buf.put_u16(STATUS_SUCCESS);
    buf.put_u16(ack_cmd);
    buf.put_u16(payload.len() as u16);
    buf.put_u16(request_id);
    buf.put_slice(payload);
    buf.to_vec()
}

/// Build a payload-less error ACK (8-byte header only), as real cameras send.
fn build_error_ack(ack_cmd: u16, request_id: u16, status: u16) -> Vec<u8> {
    let mut buf = BytesMut::with_capacity(8);
    buf.put_u16(status);
    buf.put_u16(ack_cmd);
    buf.put_u16(0);
    buf.put_u16(request_id);
    buf.to_vec()
}

async fn handle_readreg(
    socket: &UdpSocket,
    peer: SocketAddr,
    request_id: u16,
    payload: &[u8],
    regs: &Mutex<RegisterMap>,
) {
    // READREG payload: one or more 4-byte addresses
    if payload.len() < 4 || !payload.len().is_multiple_of(4) {
        return;
    }
    let store = regs.lock().await;
    let mut resp_payload = BytesMut::new();
    for chunk in payload.chunks(4) {
        let addr = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) as u64;
        // A register is a 32-bit word on a 32-bit boundary; anything else is
        // not a register, and GVCP says so with its own status.
        let refusal = if !addr.is_multiple_of(4) {
            Some(STATUS_BAD_ALIGNMENT)
        } else if store.is_write_only(addr, 4) {
            Some(STATUS_ACCESS_DENIED)
        } else {
            None
        };
        if let Some(status) = refusal {
            let resp = build_error_ack(READREG_ACK, request_id, status);
            let _ = socket.send_to(&resp, peer).await;
            debug!(%peer, addr = format!("0x{addr:x}"), status, "READREG refused");
            return;
        }
        let data = store.read(addr, 4);
        resp_payload.put_slice(&data);
    }
    let resp = build_ack(READREG_ACK, request_id, &resp_payload);
    let _ = socket.send_to(&resp, peer).await;
    trace!(%peer, regs = payload.len() / 4, "READREG response");
}

async fn handle_writereg(
    socket: &UdpSocket,
    peer: SocketAddr,
    request_id: u16,
    payload: &[u8],
    regs: &Mutex<RegisterMap>,
    acq_start: &Notify,
    acq_stop_flag: &AtomicBool,
) {
    // WRITEREG payload: pairs of (address: u32, value: u32)
    if payload.len() < 8 || !payload.len().is_multiple_of(8) {
        return;
    }
    let (test_packet, dest_ip, dest_port, max_on_wire) = {
        let mut store = regs.lock().await;
        for chunk in payload.chunks(8) {
            let addr = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) as u64;
            let value = &chunk[4..8];
            store.write(addr, value);
            store.handle_special_write(addr);
            check_acquisition(addr, value, acq_start, acq_stop_flag);
        }
        (
            store.take_pending_test_packet(),
            store.stream_dest_ip(),
            store.stream_dest_port(),
            store.max_on_wire(),
        )
    };
    // WRITEREG ACK includes a 4-byte data index placeholder.
    let resp = build_ack(WRITEREG_ACK, request_id, &[0, 0, 0, 0]);
    let _ = socket.send_to(&resp, peer).await;

    // `GevSCPSPacketSize` is a single register, so a controller that writes
    // registers with WRITEREG fires the test packet this way. Serving it only
    // from WRITEMEM would make the probe depend on which command the host
    // happened to choose — silently, since the write itself still succeeds.
    if let Some(size) = test_packet {
        fire_test_packet(size, dest_ip, dest_port, max_on_wire).await;
    }
    trace!(%peer, "WRITEREG response");
}

async fn handle_readmem(
    socket: &UdpSocket,
    peer: SocketAddr,
    request_id: u16,
    payload: &[u8],
    regs: &Mutex<RegisterMap>,
) {
    // READMEM payload: address(4) + reserved(2) + count(2)
    if payload.len() < 8 {
        return;
    }
    let addr = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]) as u64;
    let count = u16::from_be_bytes([payload[6], payload[7]]) as usize;

    // GVCP requires the address and byte count to be multiples of 4. Real
    // cameras (e.g. Hikrobot) reject violations with INVALID_PARAMETER and a
    // bare 8-byte ack header; be equally strict so that client bugs are
    // caught by the in-tree fake (regression guard for issue #35).
    if !addr.is_multiple_of(4) || count == 0 || !count.is_multiple_of(4) {
        let resp = build_error_ack(READMEM_ACK, request_id, STATUS_INVALID_PARAMETER);
        let _ = socket.send_to(&resp, peer).await;
        debug!(%peer, addr = format!("0x{addr:x}"), count, "READMEM rejected: unaligned");
        return;
    }

    let store = regs.lock().await;
    // A write-only register is no more readable through READMEM than through
    // READREG. Serving the stored bytes here is what let a read-modify-write
    // of a WO bitfield pass against the fake and fail on hardware (#135).
    if store.is_write_only(addr, count) {
        let resp = build_error_ack(READMEM_ACK, request_id, STATUS_ACCESS_DENIED);
        let _ = socket.send_to(&resp, peer).await;
        debug!(%peer, addr = format!("0x{addr:x}"), count, "READMEM refused: write-only");
        return;
    }
    let data = store.read(addr, count);

    // READMEM ACK payload: address(4) + data(N)
    let mut resp_payload = BytesMut::with_capacity(4 + data.len());
    resp_payload.put_u32(addr as u32);
    resp_payload.put_slice(&data);
    let resp = build_ack(READMEM_ACK, request_id, &resp_payload);
    let _ = socket.send_to(&resp, peer).await;
    trace!(%peer, addr = format!("0x{addr:x}"), count, "READMEM response");
}

async fn handle_writemem(
    socket: &UdpSocket,
    peer: SocketAddr,
    request_id: u16,
    payload: &[u8],
    regs: &Mutex<RegisterMap>,
    acq_start: &Notify,
    acq_stop_flag: &AtomicBool,
) {
    // WRITEMEM payload: address(4) + data(N)
    if payload.len() < 4 {
        return;
    }
    let addr = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]) as u64;
    let data = &payload[4..];
    let (test_packet, dest_ip, dest_port, max_on_wire) = {
        let mut store = regs.lock().await;
        store.write(addr, data);
        store.handle_special_write(addr);
        check_acquisition(addr, data, acq_start, acq_stop_flag);
        (
            store.take_pending_test_packet(),
            store.stream_dest_ip(),
            store.stream_dest_port(),
            store.max_on_wire(),
        )
    };

    // WRITEMEM ACK payload: address(4)
    let mut resp_payload = BytesMut::with_capacity(4);
    resp_payload.put_u32(addr as u32);
    let resp = build_ack(WRITEMEM_ACK, request_id, &resp_payload);
    let _ = socket.send_to(&resp, peer).await;

    // The acknowledgement goes first: a real device answers the write and then
    // emits the test packet, and a controller that waits for the ack before
    // listening must not miss it.
    if let Some(size) = test_packet {
        fire_test_packet(size, dest_ip, dest_port, max_on_wire).await;
    }
    trace!(%peer, addr = format!("0x{addr:x}"), len = data.len(), "WRITEMEM response");
}

/// Emit one GVSP test packet of `size` bytes to the configured stream
/// destination, unless the configured path ceiling would have swallowed it.
///
/// `size` is the IP datagram size the controller asked for, so the UDP payload
/// is `size` minus the 20-byte IPv4 and 8-byte UDP headers. What matters to the
/// probe is only whether *something* arrives, but sending the right length lets
/// a caller check it.
///
/// The `max_on_wire` arm is the point of the whole mechanism: a path that
/// cannot carry the frame drops it with nobody to report the loss, which is
/// exactly what a register read cannot discover
/// ([#112](https://github.com/VitalyVorobyev/viva-genicam/issues/112)).
async fn fire_test_packet(
    size: u32,
    dest_ip: std::net::Ipv4Addr,
    dest_port: u16,
    max_on_wire: Option<u32>,
) {
    if dest_port == 0 {
        debug!("test packet requested before a stream destination was set");
        return;
    }
    if max_on_wire.is_some_and(|max| size > max) {
        debug!(
            size,
            "test packet exceeds the path ceiling; dropped in flight"
        );
        return;
    }

    const IP_AND_UDP_HEADERS: u32 = 28;
    let payload_len = size.saturating_sub(IP_AND_UDP_HEADERS) as usize;
    let Ok(sock) = UdpSocket::bind("0.0.0.0:0").await else {
        return;
    };
    // A test packet is a GVSP payload-format datagram; the controller only
    // checks that it arrived, so the header just has to be well formed.
    let mut pkt = BytesMut::with_capacity(payload_len.max(8));
    pkt.put_u16(0); // status
    pkt.put_u16(0); // block id — a test packet belongs to no block
    pkt.put_u8(0x03); // packet format: payload
    pkt.put_u8(0);
    pkt.put_u16(0); // packet id
    pkt.resize(payload_len.max(8), 0);
    let _ = sock.send_to(&pkt, (dest_ip, dest_port)).await;
    debug!(size, %dest_ip, dest_port, "test packet sent");
}

/// Handle a GVCP `ACTION_CMD` (0x0100).
///
/// Payload is `device_key`, `group_key`, `group_mask` — 12 bytes — plus a
/// 64-bit action time when flags bit 7 is set. A 24-byte payload, or one
/// carrying a scheduled time without the flag, is malformed and gets no reply:
/// the client used to send exactly that, under opcode 0x0080 (`READREG`).
async fn handle_action(
    socket: &UdpSocket,
    peer: SocketAddr,
    request_id: u16,
    flags: u8,
    payload: &[u8],
) {
    const SCHEDULED: u8 = 0x80;
    const ACK_REQUIRED: u8 = 0x01;

    let scheduled = flags & SCHEDULED != 0;
    let expected = if scheduled { 20 } else { 12 };
    if payload.len() < expected {
        warn!(
            len = payload.len(),
            expected, scheduled, "malformed action command payload"
        );
        return;
    }

    let device_key = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
    let group_key = u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]);
    let group_mask = u32::from_be_bytes([payload[8], payload[9], payload[10], payload[11]]);

    if device_key != FAKE_DEVICE_KEY
        || group_key != FAKE_GROUP_KEY
        || group_mask & FAKE_GROUP_MASK == 0
    {
        debug!(
            device_key,
            group_key, group_mask, "action command not addressed to this device"
        );
        return;
    }

    debug!(%peer, scheduled, "action command accepted");
    if flags & ACK_REQUIRED != 0 {
        let mut buf = BytesMut::with_capacity(8);
        buf.put_u16(STATUS_SUCCESS);
        buf.put_u16(ACTION_ACK);
        buf.put_u16(0);
        buf.put_u16(request_id);
        let _ = socket.send_to(&buf, peer).await;
    }
}

async fn handle_forceip(
    socket: &UdpSocket,
    peer: SocketAddr,
    request_id: u16,
    payload: &[u8],
    bind_ip: std::net::Ipv4Addr,
) {
    // FORCEIP payload: 56 bytes
    // [0..2]   reserved
    // [2..8]   target MAC address
    // [8..20]  reserved
    // [20..24] static IP
    // [24..36] reserved
    // [36..40] subnet mask
    // [40..52] reserved
    // [52..56] gateway
    if payload.len() < 56 {
        warn!(len = payload.len(), "FORCEIP payload too short");
        return;
    }

    let target_mac = &payload[2..8];
    let fake_mac: [u8; 6] = FAKE_MAC;
    if target_mac != fake_mac {
        debug!(
            target = ?target_mac,
            "FORCEIP: MAC mismatch, ignoring"
        );
        return;
    }

    let ip = std::net::Ipv4Addr::new(payload[20], payload[21], payload[22], payload[23]);
    let subnet = std::net::Ipv4Addr::new(payload[36], payload[37], payload[38], payload[39]);
    let gateway = std::net::Ipv4Addr::new(payload[52], payload[53], payload[54], payload[55]);

    debug!(
        %bind_ip,
        %ip,
        %subnet,
        %gateway,
        "FORCEIP accepted (fake camera ignores IP change)"
    );

    // Send FORCEIP_ACK (empty payload).
    let resp = build_ack(FORCEIP_ACK, request_id, &[]);
    let _ = socket.send_to(&resp, peer).await;
}

/// Count a register-access command against every address it names.
fn record_command(counters: &CommandCounters, command: u16, payload: &[u8]) {
    let word = |chunk: &[u8]| u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) as u64;
    match command {
        READREG_CMD => {
            for addr in payload.as_chunks::<4>().0 {
                counters.record(GvcpCommand::ReadReg, word(addr));
            }
        }
        WRITEREG_CMD => {
            for pair in payload.as_chunks::<8>().0 {
                counters.record(GvcpCommand::WriteReg, word(pair));
            }
        }
        READMEM_CMD if payload.len() >= 4 => counters.record(GvcpCommand::ReadMem, word(payload)),
        WRITEMEM_CMD if payload.len() >= 4 => counters.record(GvcpCommand::WriteMem, word(payload)),
        _ => {}
    }
}

/// Check if a write targets an acquisition register and notify accordingly.
fn check_acquisition(addr: u64, data: &[u8], acq_start: &Notify, acq_stop_flag: &AtomicBool) {
    use crate::registers::{REG_ACQ_START, REG_ACQ_STOP};

    if addr == REG_ACQ_START && data.len() >= 4 {
        let val = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
        if val != 0 {
            debug!("AcquisitionStart triggered");
            acq_stop_flag.store(false, Ordering::SeqCst);
            acq_start.notify_one();
        }
    } else if addr == REG_ACQ_STOP && data.len() >= 4 {
        let val = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
        if val != 0 {
            debug!("AcquisitionStop triggered");
            acq_stop_flag.store(true, Ordering::SeqCst);
        }
    }
}

//! GVCP control channel server: discovery + GenCP register read/write.

use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use bytes::{BufMut, BytesMut};
use tokio::net::UdpSocket;
use tokio::sync::{Mutex, Notify, oneshot};
use tokio::task::JoinSet;
use tracing::{debug, trace, warn};

use crate::registers::RegisterMap;
use crate::resend::{self, PACKETRESEND_CMD, ResendCounters, Retention};

// GVCP's opcodes and key byte are vocabulary, imported from the transport
// crates (ADR-0030); their values are pinned to the specification by
// `viva-gige`'s own literal table. How the fake answers each command is its
// own reading of the specification.
use viva_gencp::{OpCode, StatusCode};
use viva_gige::consts::{
    ACTION_ACK, ACTION_COMMAND as ACTION_CMD, DISCOVERY_ACK, DISCOVERY_COMMAND as DISCOVERY_CMD,
    FORCEIP_ACK, FORCEIP_COMMAND as FORCEIP_CMD, GVCP_CMD_KEY, PENDING_ACK,
};

const READREG_CMD: u16 = OpCode::ReadRegister.command_code();
const WRITEREG_CMD: u16 = OpCode::WriteRegister.command_code();
const READMEM_CMD: u16 = OpCode::ReadMem.command_code();
const WRITEMEM_CMD: u16 = OpCode::WriteMem.command_code();
const READREG_ACK: u16 = OpCode::ReadRegister.ack_code();
const WRITEREG_ACK: u16 = OpCode::WriteRegister.ack_code();
const READMEM_ACK: u16 = OpCode::ReadMem.ack_code();
const WRITEMEM_ACK: u16 = OpCode::WriteMem.ack_code();

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

/// Subnet mask the fake reports unless told otherwise: loopback's `/8`, the
/// network every test fake lives on.
pub const FAKE_SUBNET_MASK: Ipv4Addr = Ipv4Addr::new(255, 0, 0, 0);

/// The per-instance identity a fake reports: the `FAKE_*` constants unless the
/// builder overrides them. Two fakes with one identity are indistinguishable,
/// which reproduces #137 but cannot demonstrate its fix.
#[derive(Debug, Clone)]
pub(crate) struct Identity {
    pub(crate) mac: [u8; 6],
    pub(crate) model: String,
    pub(crate) serial: String,
    pub(crate) user_name: String,
    /// The address the camera believes it has; `None` reports the one it is
    /// bound to. A camera on the wrong subnet answers a broadcast all the same,
    /// with an address the host cannot reach.
    pub(crate) ip: Option<Ipv4Addr>,
    pub(crate) subnet_mask: Ipv4Addr,
    pub(crate) gateway: Ipv4Addr,
}

impl Default for Identity {
    fn default() -> Self {
        Self {
            mac: FAKE_MAC,
            model: FAKE_MODEL.to_string(),
            serial: FAKE_SERIAL.to_string(),
            user_name: FAKE_USER_NAME.to_string(),
            ip: None,
            subnet_mask: FAKE_SUBNET_MASK,
            gateway: Ipv4Addr::UNSPECIFIED,
        }
    }
}

/// One `DISCOVERY_CMD` the fake received.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscoveryRequest {
    /// Where it came from.
    pub from: SocketAddr,
    /// The command's flag byte: `0x01` asks for an acknowledgement, `0x10`
    /// allows the device to broadcast it.
    pub flags: u8,
}

// GigE Vision status codes, imported as vocabulary (ADR-0030).
const STATUS_SUCCESS: u16 = StatusCode::Success.to_raw();
const STATUS_NOT_IMPLEMENTED: u16 = StatusCode::NotImplemented.to_raw();
const STATUS_INVALID_PARAMETER: u16 = StatusCode::InvalidParameter.to_raw();
const STATUS_BAD_ALIGNMENT: u16 = StatusCode::BadAlignment.to_raw();
const STATUS_ACCESS_DENIED: u16 = StatusCode::AccessDenied.to_raw();
const STATUS_INVALID_ADDRESS: u16 = StatusCode::InvalidAddress.to_raw();

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
    sequence: std::sync::Mutex<Vec<(GvcpCommand, u64, SocketAddr)>>,
}

impl CommandCounters {
    fn record(&self, command: GvcpCommand, addr: u64, peer: SocketAddr) {
        if let Ok(mut counts) = self.counts.lock() {
            *counts.entry((command, addr)).or_default() += 1;
        }
        if let Ok(mut sequence) = self.sequence.lock() {
            sequence.push((command, addr, peer));
        }
    }

    /// Every register-access command `peer` sent since the last
    /// [`reset`](Self::reset), in arrival order: what one application did,
    /// when several talk to the camera.
    pub fn from(&self, peer: SocketAddr) -> Vec<(GvcpCommand, u64)> {
        self.sequence
            .lock()
            .map(|sequence| {
                sequence
                    .iter()
                    .filter(|(_, _, from)| *from == peer)
                    .map(|&(command, addr, _)| (command, addr))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Every register-access command received since the last
    /// [`reset`](Self::reset), in arrival order: for a test about the order in
    /// which a controller configures the device.
    pub fn sequence(&self) -> Vec<(GvcpCommand, u64)> {
        self.sequence
            .lock()
            .map(|sequence| {
                sequence
                    .iter()
                    .map(|&(command, addr, _)| (command, addr))
                    .collect()
            })
            .unwrap_or_default()
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
        if let Ok(mut sequence) = self.sequence.lock() {
            sequence.clear();
        }
    }
}

/// Behaviour of the GVCP server beyond the register store.
#[derive(Debug)]
pub(crate) struct ServerOptions {
    /// End reception and drain delayed replies before releasing the socket.
    pub shutdown: oneshot::Receiver<()>,
    /// Counts of every register-access command received.
    pub counters: Arc<CommandCounters>,
    /// Refuse READREG/WRITEREG in this way, if set.
    pub refuse_register_commands: Option<RegisterCommandRefusal>,
    /// The packets of recent blocks, for `PACKETRESEND_CMD`.
    pub retention: Arc<std::sync::Mutex<Retention>>,
    /// What the device did with resend requests.
    pub resend: Arc<ResendCounters>,
    /// Every `DISCOVERY_CMD` received.
    pub discoveries: Arc<std::sync::Mutex<Vec<DiscoveryRequest>>>,
    /// Commands addressing these registers are acknowledged only after the
    /// delay, preceded by a `PENDING_ACK` once the controller enabled it.
    pub delayed_acks: Arc<HashMap<u64, Duration>>,
    /// How often the heartbeat rule has revoked control privilege.
    pub expiries: Arc<AtomicU64>,
    /// Action commands addressed to this device and acted on.
    pub actions: Arc<AtomicU64>,
    /// Whether action commands are implemented at all.
    pub action_commands: bool,
    /// Set while the camera's link is down ([`FakeCamera::unplug`]): every
    /// datagram that arrives is lost, unanswered.
    ///
    /// [`FakeCamera::unplug`]: crate::FakeCamera::unplug
    pub link_down: Arc<AtomicBool>,
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
    mut options: ServerOptions,
    identity: Identity,
) {
    let mut buf = [0u8; 2048];
    // Delayed commands not yet acknowledged, by sender and request id: a
    // retry of one is the same command, not a second one to execute.
    let in_flight: Arc<std::sync::Mutex<HashSet<(SocketAddr, u16)>>> = Arc::default();
    let mut delayed = JoinSet::new();
    loop {
        while delayed.try_join_next().is_some() {}
        let received = tokio::select! {
            biased;
            _ = &mut options.shutdown => break,
            received = socket.recv_from(&mut buf) => received,
        };
        let (len, peer) = match received {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, "GVCP recv error");
                continue;
            }
        };
        if options.link_down.load(Ordering::Relaxed) {
            trace!(%peer, "link down: datagram lost");
            continue;
        }
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
        let register_access = matches!(
            command,
            READREG_CMD | WRITEREG_CMD | READMEM_CMD | WRITEMEM_CMD
        );

        // Register access from the controller refreshes its heartbeat timer.
        // Discovery and FORCEIP do not: they are broadcast by any application
        // on the subnet. The command is still served after an expiry — a real
        // device answers register reads from a non-controller, it only
        // refuses configuration.
        if register_access && regs.lock().await.note_register_command(peer) {
            options.expiries.fetch_add(1, Ordering::Relaxed);
            warn!(%peer, "heartbeat expired; control privilege released");
        }

        record_command(&options.counters, command, payload, peer);

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

        let delay = register_access
            .then(|| first_address(payload))
            .flatten()
            .and_then(|addr| options.delayed_acks.get(&addr).copied());
        if delay.is_some()
            && in_flight
                .lock()
                .is_ok_and(|pending| pending.contains(&(peer, request_id)))
        {
            debug!(%peer, request_id, "retry of a command still in progress; not executed again");
            continue;
        }

        let acquisition = Acquisition {
            start: &acq_start_notify,
            stop: &acq_stop_flag,
        };
        let reply = match command {
            DISCOVERY_CMD => {
                if let Ok(mut seen) = options.discoveries.lock() {
                    seen.push(DiscoveryRequest { from: peer, flags });
                }
                let resp = build_discovery_ack(request_id, bind_ip, &identity);
                let _ = socket.send_to(&resp, peer).await;
                debug!(%peer, flags, "discovery response sent");
                continue;
            }
            FORCEIP_CMD => {
                handle_forceip(&socket, peer, request_id, payload, bind_ip, identity.mac).await;
                continue;
            }
            READREG_CMD => handle_readreg(peer, request_id, payload, &regs).await,
            WRITEREG_CMD => {
                handle_writereg(peer, request_id, payload, bind_ip, &regs, acquisition).await
            }
            READMEM_CMD => handle_readmem(peer, request_id, payload, &regs).await,
            WRITEMEM_CMD => {
                handle_writemem(peer, request_id, payload, bind_ip, &regs, acquisition).await
            }
            ACTION_CMD if !options.action_commands => {
                // What the Manta G-125B sent: the error under the command's
                // own opcode, whatever the keys.
                let resp = build_error_ack(ACTION_CMD, request_id, STATUS_NOT_IMPLEMENTED);
                let _ = socket.send_to(&resp, peer).await;
                continue;
            }
            ACTION_CMD => {
                if handle_action(&socket, peer, request_id, flags, payload).await {
                    options.actions.fetch_add(1, Ordering::Relaxed);
                }
                continue;
            }
            // No acknowledgement: the command asks for none, and the answer
            // is the packets themselves, sent from the stream socket.
            PACKETRESEND_CMD => {
                let supported = regs.lock().await.packet_resend_supported();
                resend::serve(
                    supported,
                    &options.retention,
                    &options.resend,
                    flags,
                    payload,
                )
                .await;
                continue;
            }
            _ => {
                debug!(command, "unsupported GVCP command");
                continue;
            }
        };
        let Some(reply) = reply else {
            continue;
        };

        match delay {
            None => reply.send(&socket, peer).await,
            Some(delay) => {
                if regs.lock().await.pending_ack_enabled() {
                    let resp = build_pending_ack(request_id, pending_estimate(delay));
                    let _ = socket.send_to(&resp, peer).await;
                }
                if let Ok(mut pending) = in_flight.lock() {
                    pending.insert((peer, request_id));
                }
                let socket = socket.clone();
                let in_flight = in_flight.clone();
                debug!(%peer, request_id, ?delay, "acknowledging after a delay");
                delayed.spawn(async move {
                    tokio::time::sleep(delay).await;
                    if let Ok(mut pending) = in_flight.lock() {
                        pending.remove(&(peer, request_id));
                    }
                    reply.send(&socket, peer).await;
                });
            }
        }
    }
    // A delayed reply owns an Arc of the listening socket. Aborting only the
    // receive loop leaves that old socket bound during the next fixture.
    delayed.shutdown().await;
}

/// The time a `PENDING_ACK` asks for: the delay and half again, so the
/// acknowledgement cannot race the controller's extended deadline.
fn pending_estimate(delay: Duration) -> u16 {
    let millis = delay.as_millis().saturating_mul(3) / 2;
    u16::try_from(millis).unwrap_or(u16::MAX)
}

/// A `PENDING_ACK` for `request_id`: status 0, opcode 0x0089, length 4, then
/// two reserved bytes and `time_to_completion` in milliseconds.
fn build_pending_ack(request_id: u16, millis: u16) -> Vec<u8> {
    let mut buf = BytesMut::with_capacity(12);
    buf.put_u16(STATUS_SUCCESS);
    buf.put_u16(PENDING_ACK);
    buf.put_u16(4);
    buf.put_u16(request_id);
    buf.put_u16(0);
    buf.put_u16(millis);
    buf.to_vec()
}

/// The first register address a register-access command names.
fn first_address(payload: &[u8]) -> Option<u64> {
    let word: [u8; 4] = payload.get(..4)?.try_into().ok()?;
    Some(u64::from(u32::from_be_bytes(word)))
}

/// What a register-access command answers, and a test packet to fire after
/// the acknowledgement.
struct Reply {
    ack: Vec<u8>,
    test_packet: Option<TestPacket>,
}

/// A fire-test-packet request, captured while the register lock was held.
struct TestPacket {
    size: u32,
    source_ip: Ipv4Addr,
    dest_ip: Ipv4Addr,
    dest_port: u16,
    max_on_wire: Option<u32>,
}

impl Reply {
    fn ack(ack: Vec<u8>) -> Option<Self> {
        Some(Self {
            ack,
            test_packet: None,
        })
    }

    /// Send the acknowledgement, then the test packet. A real device answers
    /// the write and then emits the packet, and a controller that waits for
    /// the ack before listening must not miss it.
    async fn send(self, socket: &UdpSocket, peer: SocketAddr) {
        let _ = socket.send_to(&self.ack, peer).await;
        if let Some(t) = self.test_packet {
            fire_test_packet(t.size, t.source_ip, t.dest_ip, t.dest_port, t.max_on_wire).await;
        }
    }
}

/// Build a 256-byte discovery ACK payload (GVCP header + device info).
fn build_discovery_ack(request_id: u16, ip: std::net::Ipv4Addr, identity: &Identity) -> Vec<u8> {
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

    // 10  MAC address (6 bytes): FAKE_MAC DE:AD:BE:EF:CA:FE unless overridden
    buf.put_slice(&identity.mac);

    buf.put_u32(0x0000_0007); // 16  Supported IP config (DHCP + persistent + LLA)
    buf.put_u32(0x0000_0005); // 20  Current IP config

    // 24  Reserved (12 bytes)
    buf.put_slice(&[0u8; 12]);

    // 36  Current IP address
    buf.put_slice(&identity.ip.unwrap_or(ip).octets());

    // 40  Reserved (12 bytes)
    buf.put_slice(&[0u8; 12]);

    // 52  Subnet mask
    buf.put_slice(&identity.subnet_mask.octets());

    // 56  Reserved (12 bytes)
    buf.put_slice(&[0u8; 12]);

    // 68  Default gateway
    buf.put_slice(&identity.gateway.octets());

    // Manufacturer name (32 bytes)
    put_fixed_string(&mut buf, FAKE_MANUFACTURER, 32); // 72
    // Model name (32 bytes)
    put_fixed_string(&mut buf, &identity.model, 32); // 104
    // Device version (32 bytes)
    put_fixed_string(&mut buf, FAKE_VERSION, 32); // 136
    // Manufacturer specific info (48 bytes)
    put_fixed_string(&mut buf, "Fake camera for testing", 48); // 168
    // Serial number (16 bytes)
    put_fixed_string(&mut buf, &identity.serial, 16); // 216
    // User defined name (16 bytes)
    put_fixed_string(&mut buf, &identity.user_name, 16); // 232

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
    peer: SocketAddr,
    request_id: u16,
    payload: &[u8],
    regs: &Mutex<RegisterMap>,
) -> Option<Reply> {
    // READREG payload: one or more 4-byte addresses
    if payload.len() < 4 || !payload.len().is_multiple_of(4) {
        return None;
    }
    let store = regs.lock().await;
    if !store.may_access(peer, false) {
        debug!(%peer, "READREG refused: another application holds exclusive access");
        return Reply::ack(build_error_ack(
            READREG_ACK,
            request_id,
            STATUS_ACCESS_DENIED,
        ));
    }
    let mut resp_payload = BytesMut::new();
    for chunk in payload.chunks(4) {
        let addr = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) as u64;
        // A register is a 32-bit word on a 32-bit boundary; anything else is
        // not a register, and GVCP says so with its own status.
        let refusal = if !addr.is_multiple_of(4) {
            Some(STATUS_BAD_ALIGNMENT)
        } else if store.is_invalid_address(addr, 4) {
            Some(STATUS_INVALID_ADDRESS)
        } else if store.is_write_only(addr, 4) {
            Some(STATUS_ACCESS_DENIED)
        } else {
            None
        };
        if let Some(status) = refusal {
            debug!(%peer, addr = format!("0x{addr:x}"), status, "READREG refused");
            return Reply::ack(build_error_ack(READREG_ACK, request_id, status));
        }
        let data = store.read(addr, 4);
        resp_payload.put_slice(&data);
    }
    trace!(%peer, regs = payload.len() / 4, "READREG response");
    Reply::ack(build_ack(READREG_ACK, request_id, &resp_payload))
}

/// Apply a write from `peer` of `data` at `addr`, CCP included, or refuse it
/// with `ACCESS_DENIED` while another application holds control.
fn write_checked(
    store: &mut RegisterMap,
    peer: SocketAddr,
    addr: u64,
    data: &[u8],
    acquisition: Acquisition<'_>,
) -> Result<(), u16> {
    if addr == crate::registers::CONTROL_CHANNEL_PRIVILEGE && data.len() == 4 {
        let value = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
        return if store.write_ccp(peer, value) {
            Ok(())
        } else {
            Err(STATUS_ACCESS_DENIED)
        };
    }
    if !store.may_access(peer, true) {
        return Err(STATUS_ACCESS_DENIED);
    }
    store.write(addr, data);
    store.handle_special_write(addr);
    check_acquisition(addr, data, acquisition.start, acquisition.stop);
    Ok(())
}

/// The test packet the last write asked for, if any, with where it goes.
fn take_test_packet(store: &mut RegisterMap, source_ip: Ipv4Addr) -> Option<TestPacket> {
    let size = store.take_pending_test_packet()?;
    Some(TestPacket {
        size,
        source_ip,
        dest_ip: store.stream_dest_ip(),
        dest_port: store.stream_dest_port(),
        max_on_wire: store.max_on_wire(),
    })
}

async fn handle_writereg(
    peer: SocketAddr,
    request_id: u16,
    payload: &[u8],
    source_ip: std::net::Ipv4Addr,
    regs: &Mutex<RegisterMap>,
    acquisition: Acquisition<'_>,
) -> Option<Reply> {
    // WRITEREG payload: pairs of (address: u32, value: u32)
    if payload.len() < 8 || !payload.len().is_multiple_of(8) {
        return None;
    }
    let mut store = regs.lock().await;
    for (index, chunk) in payload.chunks(8).enumerate() {
        let addr = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) as u64;
        if let Err(status) = write_checked(&mut store, peer, addr, &chunk[4..8], acquisition) {
            debug!(%peer, addr = format!("0x{addr:x}"), status, "WRITEREG refused");
            debug!(%peer, index, "WRITEREG stopped at this register");
            return Reply::ack(build_error_ack(WRITEREG_ACK, request_id, status));
        }
    }
    // `GevSCPSPacketSize` is a single register, so a controller that writes
    // registers with WRITEREG fires the test packet this way. Serving it only
    // from WRITEMEM would make the probe depend on which command the host
    // happened to choose — silently, since the write itself still succeeds.
    let test_packet = take_test_packet(&mut store, source_ip);
    trace!(%peer, "WRITEREG response");
    // WRITEREG ACK includes a 4-byte data index placeholder.
    Some(Reply {
        ack: build_ack(WRITEREG_ACK, request_id, &[0, 0, 0, 0]),
        test_packet,
    })
}

async fn handle_readmem(
    peer: SocketAddr,
    request_id: u16,
    payload: &[u8],
    regs: &Mutex<RegisterMap>,
) -> Option<Reply> {
    // READMEM payload: address(4) + reserved(2) + count(2)
    if payload.len() < 8 {
        return None;
    }
    let addr = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]) as u64;
    let count = u16::from_be_bytes([payload[6], payload[7]]) as usize;

    // GVCP requires the address and byte count to be multiples of 4. Real
    // cameras (e.g. Hikrobot) reject violations with INVALID_PARAMETER and a
    // bare 8-byte ack header; be equally strict so that client bugs are
    // caught by the in-tree fake (regression guard for issue #35).
    if !addr.is_multiple_of(4) || count == 0 || !count.is_multiple_of(4) {
        debug!(%peer, addr = format!("0x{addr:x}"), count, "READMEM rejected: unaligned");
        return Reply::ack(build_error_ack(
            READMEM_ACK,
            request_id,
            STATUS_INVALID_PARAMETER,
        ));
    }

    let store = regs.lock().await;
    if store.is_invalid_address(addr, count) {
        debug!(%peer, addr = format!("0x{addr:x}"), count, "READMEM refused: no such register");
        return Reply::ack(build_error_ack(
            READMEM_ACK,
            request_id,
            STATUS_INVALID_ADDRESS,
        ));
    }
    // A write-only register is no more readable through READMEM than through
    // READREG. Serving the stored bytes here is what let a read-modify-write
    // of a WO bitfield pass against the fake and fail on hardware (#135).
    if store.is_write_only(addr, count) || !store.may_access(peer, false) {
        debug!(%peer, addr = format!("0x{addr:x}"), count, "READMEM refused");
        return Reply::ack(build_error_ack(
            READMEM_ACK,
            request_id,
            STATUS_ACCESS_DENIED,
        ));
    }
    let data = store.read(addr, count);

    // READMEM ACK payload: address(4) + data(N)
    let mut resp_payload = BytesMut::with_capacity(4 + data.len());
    resp_payload.put_u32(addr as u32);
    resp_payload.put_slice(&data);
    trace!(%peer, addr = format!("0x{addr:x}"), count, "READMEM response");
    Reply::ack(build_ack(READMEM_ACK, request_id, &resp_payload))
}

async fn handle_writemem(
    peer: SocketAddr,
    request_id: u16,
    payload: &[u8],
    source_ip: std::net::Ipv4Addr,
    regs: &Mutex<RegisterMap>,
    acquisition: Acquisition<'_>,
) -> Option<Reply> {
    // WRITEMEM payload: address(4) + data(N)
    if payload.len() < 4 {
        return None;
    }
    let addr = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]) as u64;
    let data = &payload[4..];
    // The same rule as READMEM: a 32-bit aligned address and a whole number
    // of 32-bit words (TC-32). No device has been seen to enforce it on a
    // write; the fake does, so a client that relies on a lenient device
    // fails here first.
    if !addr.is_multiple_of(4) || data.is_empty() || !data.len().is_multiple_of(4) {
        debug!(%peer, addr = format!("0x{addr:x}"), len = data.len(), "WRITEMEM rejected: unaligned");
        return Reply::ack(build_error_ack(
            WRITEMEM_ACK,
            request_id,
            STATUS_INVALID_PARAMETER,
        ));
    }
    let mut store = regs.lock().await;
    if let Err(status) = write_checked(&mut store, peer, addr, data, acquisition) {
        debug!(%peer, addr = format!("0x{addr:x}"), status, "WRITEMEM refused");
        return Reply::ack(build_error_ack(WRITEMEM_ACK, request_id, status));
    }
    let test_packet = take_test_packet(&mut store, source_ip);

    // WRITEMEM ACK payload: address(4)
    let mut resp_payload = BytesMut::with_capacity(4);
    resp_payload.put_u32(addr as u32);
    trace!(%peer, addr = format!("0x{addr:x}"), len = data.len(), "WRITEMEM response");
    Some(Reply {
        ack: build_ack(WRITEMEM_ACK, request_id, &resp_payload),
        test_packet,
    })
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
    source_ip: std::net::Ipv4Addr,
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
    // From the camera's own address, as every GVSP packet a device sends.
    let Ok(sock) = UdpSocket::bind((source_ip, 0)).await else {
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
///
/// Returns whether the device acts on it: its keys are this device's.
async fn handle_action(
    socket: &UdpSocket,
    peer: SocketAddr,
    request_id: u16,
    flags: u8,
    payload: &[u8],
) -> bool {
    const SCHEDULED: u8 = 0x80;
    const ACK_REQUIRED: u8 = 0x01;

    let scheduled = flags & SCHEDULED != 0;
    let expected = if scheduled { 20 } else { 12 };
    if payload.len() < expected {
        warn!(
            len = payload.len(),
            expected, scheduled, "malformed action command payload"
        );
        return false;
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
        return false;
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
    true
}

async fn handle_forceip(
    socket: &UdpSocket,
    peer: SocketAddr,
    request_id: u16,
    payload: &[u8],
    bind_ip: std::net::Ipv4Addr,
    fake_mac: [u8; 6],
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
fn record_command(counters: &CommandCounters, command: u16, payload: &[u8], peer: SocketAddr) {
    let word = |chunk: &[u8]| u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) as u64;
    match command {
        READREG_CMD => {
            for addr in payload.as_chunks::<4>().0 {
                counters.record(GvcpCommand::ReadReg, word(addr), peer);
            }
        }
        WRITEREG_CMD => {
            for pair in payload.as_chunks::<8>().0 {
                counters.record(GvcpCommand::WriteReg, word(pair), peer);
            }
        }
        READMEM_CMD if payload.len() >= 4 => {
            counters.record(GvcpCommand::ReadMem, word(payload), peer)
        }
        WRITEMEM_CMD if payload.len() >= 4 => {
            counters.record(GvcpCommand::WriteMem, word(payload), peer)
        }
        _ => {}
    }
}

/// How a register write starts and stops the GVSP sender.
#[derive(Clone, Copy)]
pub(crate) struct Acquisition<'a> {
    start: &'a Notify,
    stop: &'a AtomicBool,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stopping_the_server_releases_delayed_reply_socket() {
        let socket = Arc::new(UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap());
        let address = socket.local_addr().unwrap();
        let weak = Arc::downgrade(&socket);
        let regs = Arc::new(Mutex::new(RegisterMap::new(
            64,
            48,
            crate::MONO8,
            false,
            Default::default(),
        )));
        let (shutdown, stopped) = oneshot::channel();
        let options = ServerOptions {
            shutdown: stopped,
            counters: Arc::default(),
            refuse_register_commands: None,
            retention: Arc::new(std::sync::Mutex::new(Retention::new(4))),
            resend: Arc::default(),
            discoveries: Arc::default(),
            delayed_acks: Arc::new(HashMap::from([(crate::REG_WIDTH, Duration::from_secs(60))])),
            expiries: Arc::default(),
            actions: Arc::default(),
            action_commands: true,
            link_down: Arc::default(),
        };
        let server = tokio::spawn(run(
            socket,
            regs,
            Arc::default(),
            Arc::default(),
            Ipv4Addr::LOCALHOST,
            options,
            Identity::default(),
        ));
        let client = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let mut request = vec![GVCP_CMD_KEY, 1, 0, 0x80, 0, 4, 0, 1];
        request.extend_from_slice(&(crate::REG_WIDTH as u32).to_be_bytes());
        client.send_to(&request, address).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while weak.strong_count() < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the delayed reply owns a socket clone");

        shutdown.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .expect("shutdown drains pending replies without waiting for their delay")
            .expect("server");
        assert!(
            weak.upgrade().is_none(),
            "delayed reply retained the stopped server's socket"
        );
    }
}

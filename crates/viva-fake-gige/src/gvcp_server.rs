//! GVCP control channel server: discovery + GenCP register read/write.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use bytes::{BufMut, BytesMut};
use tokio::net::UdpSocket;
use tokio::sync::{Mutex, Notify};
use tracing::{debug, trace, warn};

use crate::registers::RegisterMap;

/// GVCP command key byte (first byte of every GVCP command).
const GVCP_CMD_KEY: u8 = 0x42;

// GVCP command opcodes
const DISCOVERY_CMD: u16 = 0x0002;
const READREG_CMD: u16 = 0x0080;
const WRITEREG_CMD: u16 = 0x0082;
const READMEM_CMD: u16 = 0x0084;
const WRITEMEM_CMD: u16 = 0x0086;

// GVCP ack opcodes
const DISCOVERY_ACK: u16 = 0x0003;
const READREG_ACK: u16 = 0x0081;
const WRITEREG_ACK: u16 = 0x0083;
const READMEM_ACK: u16 = 0x0085;
const WRITEMEM_ACK: u16 = 0x0087;

/// Status code for success.
const STATUS_SUCCESS: u16 = 0x0000;

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

        let _flags = pkt[1];
        let command = u16::from_be_bytes([pkt[2], pkt[3]]);
        let _length = u16::from_be_bytes([pkt[4], pkt[5]]);
        let request_id = u16::from_be_bytes([pkt[6], pkt[7]]);
        let payload = &pkt[8..];

        match command {
            DISCOVERY_CMD => {
                let resp = build_discovery_ack(request_id, bind_ip);
                let _ = socket.send_to(&resp, peer).await;
                debug!(%peer, "discovery response sent");
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

    // Discovery payload (248 bytes):
    buf.put_u16(2); // Spec major version
    buf.put_u16(0); // Spec minor version
    buf.put_u32(0); // Device mode
    buf.put_u32(0); // Reserved

    // MAC address (6 bytes): fake MAC DE:AD:BE:EF:CA:FE
    buf.put_slice(&[0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE]);

    buf.put_u32(0x0000_0007); // Supported IP config (DHCP + persistent + LLA)
    buf.put_u32(0x0000_0005); // Current IP config

    // 10 bytes reserved
    buf.put_slice(&[0u8; 10]);

    // Current IP address
    buf.put_slice(&ip.octets());

    // 12 bytes reserved
    buf.put_slice(&[0u8; 12]);

    // Subnet mask (255.255.255.0)
    buf.put_slice(&[255, 255, 255, 0]);

    // 12 bytes reserved
    buf.put_slice(&[0u8; 12]);

    // Gateway
    buf.put_slice(&[0, 0, 0, 0]);

    // Manufacturer name (32 bytes)
    put_fixed_string(&mut buf, "genicam-rs", 32);
    // Model name (32 bytes)
    put_fixed_string(&mut buf, "FakeGigE", 32);
    // Device version (32 bytes)
    put_fixed_string(&mut buf, "1.0.0", 32);
    // Manufacturer specific info (48 bytes)
    put_fixed_string(&mut buf, "Fake camera for testing", 48);
    // Serial number (16 bytes)
    put_fixed_string(&mut buf, "FAKE-001", 16);
    // User defined name (16 bytes)
    put_fixed_string(&mut buf, "FakeCamera", 16);

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
    let mut store = regs.lock().await;
    for chunk in payload.chunks(8) {
        let addr = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) as u64;
        let value = &chunk[4..8];
        store.write(addr, value);
        check_acquisition(addr, value, acq_start, acq_stop_flag);
    }
    // WRITEREG ACK includes a 4-byte data index placeholder.
    let resp = build_ack(WRITEREG_ACK, request_id, &[0, 0, 0, 0]);
    let _ = socket.send_to(&resp, peer).await;
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
    let store = regs.lock().await;
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
    let mut store = regs.lock().await;
    store.write(addr, data);
    check_acquisition(addr, data, acq_start, acq_stop_flag);

    // WRITEMEM ACK payload: address(4)
    let mut resp_payload = BytesMut::with_capacity(4);
    resp_payload.put_u32(addr as u32);
    let resp = build_ack(WRITEMEM_ACK, request_id, &resp_payload);
    let _ = socket.send_to(&resp, peer).await;
    trace!(%peer, addr = format!("0x{addr:x}"), len = data.len(), "WRITEMEM response");
}

/// Check if a write targets an acquisition register and notify accordingly.
fn check_acquisition(addr: u64, data: &[u8], acq_start: &Notify, acq_stop_flag: &AtomicBool) {
    if addr == 0x20020 && data.len() >= 4 {
        // AcquisitionStart command register
        let val = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
        if val != 0 {
            debug!("AcquisitionStart triggered");
            acq_stop_flag.store(false, Ordering::SeqCst);
            acq_start.notify_one();
        }
    } else if addr == 0x20024 && data.len() >= 4 {
        // AcquisitionStop command register
        let val = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
        if val != 0 {
            debug!("AcquisitionStop triggered");
            acq_stop_flag.store(true, Ordering::SeqCst);
        }
    }
}

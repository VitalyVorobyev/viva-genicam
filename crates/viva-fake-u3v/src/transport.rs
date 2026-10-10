//! Fake USB transport implementing [`UsbTransfer`] via an in-memory register map.
//!
//! The control endpoint speaks GenCP as GenCP 1.2 defines it — the `U3VC`
//! prefix both ways, RequestAck at bit 14, READMEM `0x0800` and WRITEMEM
//! `0x0802`, every field little-endian — and refuses what a device refuses.
//! The stream endpoint sends a block only while the host has set StreamEnable
//! and started acquisition, in the transfer sizes the host wrote to the SIRM.
//! Its constants are its own, not `viva-u3v`'s (ADR-0019, backlog TC-27).

use std::sync::Mutex;
use std::time::Duration;

use bytes::{BufMut, BytesMut};
use viva_gencp::StatusCode;
use viva_u3v::U3vError;
use viva_u3v::usb::UsbTransfer;

use crate::registers::{
    LEADER_SIZE, REG_ACQ_START, REG_ACQ_STOP, RegisterMap, SIRM_CONTROL, TRAILER_SIZE,
};

/// "U3VC", little-endian: the prefix of every command and acknowledge.
const CONTROL_PREFIX: [u8; 4] = *b"U3VC";
/// Prefix plus common command data.
const HEADER_SIZE: usize = 12;
/// GenCP 1.2 Table 4: the RequestAck flag.
const REQUEST_ACK: u16 = 1 << 14;
// GenCP 1.2 Table 7.
const READMEM_CMD: u16 = 0x0800;
const WRITEMEM_CMD: u16 = 0x0802;
/// GenCP 1.2 Table 6: `GENCP_INVALID_HEADER`.
const INVALID_HEADER: u16 = 0x800E;

/// The control endpoints, as [`FakeU3vCamera`](crate::FakeU3vCamera) opens them.
const STREAM_EP: u8 = 0x82;

/// "U3VL" and "U3VT", little-endian.
const LEADER_PREFIX: [u8; 4] = *b"U3VL";
const TRAILER_PREFIX: [u8; 4] = *b"U3VT";

/// Longest a stream read blocks before reporting no data, standing in for
/// the host's timeout so an idle reader does not spin.
const IDLE_READ: Duration = Duration::from_millis(20);

/// Fake USB transport backed by an in-memory register map.
///
/// Implements [`UsbTransfer`] by executing bulk writes as GenCP commands and
/// queuing their acknowledges. Stream endpoint reads return synthetic blocks.
pub struct FakeU3vTransport {
    state: Mutex<TransportState>,
}

struct TransportState {
    registers: RegisterMap,
    /// The acknowledge for the last command, until the host reads it.
    pending_ack: Option<Vec<u8>>,
    /// Whether AcquisitionStart has run without an AcquisitionStop since.
    acquiring: bool,
    /// The block being sent, once its leader has gone.
    block: Option<Block>,
    next_block_id: u64,
    /// `(address, length)` of every register read the device was asked for,
    /// served or refused, in order.
    reads: Vec<(u64, usize)>,
    /// `(address, bytes)` of every write the device accepted, in order.
    writes: Vec<(u64, Vec<u8>)>,
}

/// One block on its way out: its payload, and the transfers left to send it
/// in.
struct Block {
    id: u64,
    payload: Vec<u8>,
    sent: usize,
    transfers: std::vec::IntoIter<u32>,
    height: u32,
}

impl FakeU3vTransport {
    /// Create a new fake transport with the given image dimensions.
    pub fn new(width: u32, height: u32, pixel_format: u32) -> Self {
        Self {
            state: Mutex::new(TransportState {
                registers: RegisterMap::new(width, height, pixel_format),
                pending_ack: None,
                acquiring: false,
                block: None,
                next_block_id: 1,
                reads: Vec::new(),
                writes: Vec::new(),
            }),
        }
    }

    /// `(address, length)` of every READMEM received so far, including the
    /// ones refused for touching a write-only register.
    ///
    /// The device's own record, so a test can tell a read that never happened
    /// from one whose failure the client swallowed.
    pub fn reads(&self) -> Vec<(u64, usize)> {
        self.state.lock().unwrap().reads.clone()
    }

    /// Forget the reads recorded so far, e.g. the XML fetch at open.
    pub fn clear_reads(&self) {
        self.state.lock().unwrap().reads.clear();
    }

    /// Whether any recorded read touched the 4-byte register at `addr`.
    pub fn was_read(&self, addr: u64) -> bool {
        self.state
            .lock()
            .unwrap()
            .reads
            .iter()
            .any(|&(a, len)| a < addr + 4 && addr < a.saturating_add(len as u64))
    }

    /// `(address, bytes)` of every write the device accepted, in order.
    pub fn writes(&self) -> Vec<(u64, Vec<u8>)> {
        self.state.lock().unwrap().writes.clone()
    }

    /// The device's register bytes at `[addr, addr + len)`, as it holds them.
    pub fn peek(&self, addr: u64, len: usize) -> Vec<u8> {
        self.state.lock().unwrap().registers.read(addr, len)
    }

    /// Whether the host has StreamEnable set.
    pub fn stream_enabled(&self) -> bool {
        self.state.lock().unwrap().registers.stream_enabled()
    }

    /// Whether acquisition is running.
    pub fn acquiring(&self) -> bool {
        self.state.lock().unwrap().acquiring
    }
}

impl UsbTransfer for FakeU3vTransport {
    fn bulk_write(
        &self,
        _endpoint: u8,
        data: &[u8],
        _timeout: Duration,
    ) -> Result<usize, U3vError> {
        let mut state = self.state.lock().unwrap();
        state.pending_ack = execute(&mut state, data);
        Ok(data.len())
    }

    fn bulk_read(
        &self,
        endpoint: u8,
        buf: &mut [u8],
        timeout: Duration,
    ) -> Result<usize, U3vError> {
        let mut state = self.state.lock().unwrap();

        if endpoint == STREAM_EP {
            if !(state.registers.stream_enabled() && state.acquiring) {
                drop(state);
                std::thread::sleep(timeout.min(IDLE_READ));
                return Err(U3vError::Timeout);
            }
            return stream_transfer(&mut state, buf);
        }

        // Control endpoint: the acknowledge, if the command asked for one.
        let Some(ack) = state.pending_ack.take() else {
            drop(state);
            std::thread::sleep(timeout.min(IDLE_READ));
            return Err(U3vError::Timeout);
        };
        if buf.len() < ack.len() {
            return Err(U3vError::Usb(format!(
                "overflow: a {}-byte acknowledge into a {}-byte buffer",
                ack.len(),
                buf.len()
            )));
        }
        buf[..ack.len()].copy_from_slice(&ack);
        Ok(ack.len())
    }
}

/// Execute one command packet; the acknowledge, or `None` when the command
/// did not ask for one or is not a GenCP command at all.
fn execute(state: &mut TransportState, packet: &[u8]) -> Option<Vec<u8>> {
    if packet.len() < HEADER_SIZE || packet[..4] != CONTROL_PREFIX {
        return None;
    }
    let le16 = |at: usize| u16::from_le_bytes([packet[at], packet[at + 1]]);
    let le64 = |at: usize| u64::from_le_bytes(packet[at..at + 8].try_into().unwrap());
    let (flags, command, length, request_id) = (le16(4), le16(6), le16(8), le16(10));
    if flags & REQUEST_ACK == 0 {
        return None;
    }
    let ack_id = command | 1;
    if packet.len() != HEADER_SIZE + length as usize {
        return Some(build_ack(
            StatusCode::from_raw(INVALID_HEADER),
            ack_id,
            request_id,
            &[],
        ));
    }

    Some(match command {
        READMEM_CMD if length == 12 => {
            let (addr, reserved, count) = (le64(12), le16(20), le16(22) as usize);
            state.reads.push((addr, count));
            let status = if reserved != 0 {
                StatusCode::InvalidParameter
            } else if !state.registers.is_mapped(addr, count) {
                StatusCode::InvalidAddress
            } else if state.registers.is_write_only(addr, count) {
                StatusCode::AccessDenied
            } else {
                let data = state.registers.read(addr, count);
                return Some(build_ack(StatusCode::Success, ack_id, request_id, &data));
            };
            build_ack(status, ack_id, request_id, &[])
        }
        WRITEMEM_CMD if length >= 8 => {
            let (addr, data) = (le64(12), &packet[HEADER_SIZE + 8..]);
            let status = if request_id == 0 {
                // GenCP §3.1: request_id 0 is for reads only.
                StatusCode::InvalidParameter
            } else if !state.registers.is_mapped(addr, data.len()) {
                StatusCode::InvalidAddress
            } else if state.registers.is_read_only(addr, data.len()) {
                StatusCode::WriteProtect
            } else {
                state.registers.write(addr, data);
                state.writes.push((addr, data.to_vec()));
                apply_side_effects(state, addr, data);
                // The Written Length Field capability: reserved, then the
                // bytes written (GenCP 1.2 Table 11).
                let mut scd = [0u8; 4];
                scd[2..].copy_from_slice(&(data.len() as u16).to_le_bytes());
                return Some(build_ack(StatusCode::Success, ack_id, request_id, &scd));
            };
            build_ack(status, ack_id, request_id, &[])
        }
        _ => build_ack(StatusCode::NotImplemented, ack_id, request_id, &[]),
    })
}

/// What a write starts or stops.
fn apply_side_effects(state: &mut TransportState, addr: u64, data: &[u8]) {
    let touches = |reg: u64| addr < reg + 4 && reg < addr + data.len() as u64;
    if touches(REG_ACQ_START) {
        state.acquiring = true;
    }
    if touches(REG_ACQ_STOP) {
        state.acquiring = false;
    }
    // A stream disabled mid-block abandons it.
    if touches(SIRM_CONTROL) && !state.registers.stream_enabled() {
        state.block = None;
    }
}

/// Build an acknowledge.
fn build_ack(status: StatusCode, command: u16, request_id: u16, scd: &[u8]) -> Vec<u8> {
    let mut buf = BytesMut::with_capacity(HEADER_SIZE + scd.len());
    buf.extend_from_slice(&CONTROL_PREFIX);
    buf.put_u16_le(status.to_raw());
    buf.put_u16_le(command);
    buf.put_u16_le(scd.len() as u16);
    buf.put_u16_le(request_id);
    buf.extend_from_slice(scd);
    buf.to_vec()
}

/// Bytes per pixel for the test pattern.
fn bytes_per_pixel(pixel_format: u32) -> usize {
    match pixel_format {
        0x0218_0014 => 3, // RGB8Packed
        _ => 1,           // Mono8 and others default to 1
    }
}

/// The next transfer on the stream endpoint: a leader, the payload in the
/// host's transfer sizes, then a trailer.
///
/// Reads width, height and pixel format at each block's start, so a format
/// change takes effect on the next block. A host buffer smaller than what the
/// device sends overflows, as on USB.
fn stream_transfer(state: &mut TransportState, buf: &mut [u8]) -> Result<usize, U3vError> {
    let overflow = |sent: usize, buf: &[u8]| {
        U3vError::Usb(format!(
            "overflow: the device sent {sent} bytes into a {}-byte buffer",
            buf.len()
        ))
    };

    let Some(block) = state.block.as_mut() else {
        // A new block: its leader.
        let layout = state.registers.host_layout();
        if layout.max_leader_size < LEADER_SIZE || layout.max_trailer_size < TRAILER_SIZE {
            return Err(U3vError::Usb(format!(
                "the SIRM's leader/trailer sizes {}/{} are below the {LEADER_SIZE}/{TRAILER_SIZE} the device requires",
                layout.max_leader_size, layout.max_trailer_size
            )));
        }
        let (width, height) = (state.registers.width(), state.registers.height());
        let pixel_format = state.registers.pixel_format_code();
        let id = state.next_block_id;
        state.next_block_id += 1;

        let leader = build_leader(id, width, height, pixel_format);
        if buf.len() < leader.len() {
            return Err(overflow(leader.len(), buf));
        }
        buf[..leader.len()].copy_from_slice(&leader);

        let mut payload = generate_test_pattern(
            width as usize,
            height as usize,
            bytes_per_pixel(pixel_format),
            id as u16,
        );
        // A packed format's frame is not a whole number of bytes a pixel;
        // its payload is the declared size whatever the pattern.
        payload.resize(state.registers.payload_size() as usize, 0x80);
        let transfers: Vec<u32> = std::iter::repeat_n(
            layout.payload_transfer_size,
            layout.payload_transfer_count as usize,
        )
        .chain([layout.final_transfer1_size, layout.final_transfer2_size])
        .filter(|&size| size > 0)
        .collect();
        state.block = Some(Block {
            id,
            payload,
            sent: 0,
            transfers: transfers.into_iter(),
            height,
        });
        return Ok(leader.len());
    };

    // The payload, a transfer at a time; a transfer past the payload's end
    // is a zero-length one, which ends it as a short transfer does.
    if block.sent < block.payload.len()
        && let Some(size) = block.transfers.next()
    {
        let n = (size as usize).min(block.payload.len() - block.sent);
        if buf.len() < n {
            return Err(overflow(n, buf));
        }
        buf[..n].copy_from_slice(&block.payload[block.sent..block.sent + n]);
        block.sent += n;
        return Ok(n);
    }

    // The trailer: what was sent is what is valid.
    let trailer = build_trailer(block.id, block.sent as u64, block.height);
    state.block = None;
    if buf.len() < trailer.len() {
        return Err(overflow(trailer.len(), buf));
    }
    buf[..trailer.len()].copy_from_slice(&trailer);
    Ok(trailer.len())
}

/// Mono8: concentric rings radiating from center, shifting with each frame.
/// RGB8: colorful plasma-like pattern with animated hue rotation.
fn generate_test_pattern(width: usize, height: usize, bpp: usize, frame: u16) -> Vec<u8> {
    let size = width * height * bpp;
    let mut data = vec![0u8; size];
    let cx = width as f32 / 2.0;
    let cy = height as f32 / 2.0;
    let phase = frame as f32 * 0.15;

    for y in 0..height {
        for x in 0..width {
            let offset = (y * width + x) * bpp;
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let dist = (dx * dx + dy * dy).sqrt();

            if bpp == 1 {
                // Concentric rings with radial gradient
                let ring = ((dist * 0.1 - phase) * 2.0).sin();
                let radial = 1.0 - (dist / (cx.max(cy) * 1.2)).min(1.0);
                let val = ((ring * 0.5 + 0.5) * radial * 255.0) as u8;
                data[offset] = val;
            } else if bpp >= 3 {
                // Plasma pattern with hue rotation
                let angle = dy.atan2(dx);
                let v1 = ((dist * 0.05 - phase).sin() + 1.0) * 0.5;
                let v2 = ((angle * 3.0 + phase * 2.0).sin() + 1.0) * 0.5;
                let v3 = (((x as f32 * 0.02 + y as f32 * 0.03 + phase).sin()) + 1.0) * 0.5;

                let hue = (v1 + v2) * 0.5 + phase * 0.1;
                let sat = 0.7 + v3 * 0.3;
                let val = 0.5 + v1 * 0.5;
                let (r, g, b) = hsv_to_rgb(hue % 1.0, sat, val);
                data[offset] = r;
                data[offset + 1] = g;
                data[offset + 2] = b;
            }
        }
    }
    data
}

/// Convert HSV (all 0.0..1.0) to RGB (0..255).
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let h = h * 6.0;
    let i = h.floor() as u32;
    let f = h - i as f32;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    let (r, g, b) = match i % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    ((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

/// A 52-byte image leader, as Wireshark's `dissect_u3v_stream_leader`
/// reads one.
fn build_leader(block_id: u64, width: u32, height: u32, pixel_format: u32) -> Vec<u8> {
    let mut buf = vec![0u8; LEADER_SIZE as usize];
    buf[0..4].copy_from_slice(&LEADER_PREFIX);
    buf[6..8].copy_from_slice(&(LEADER_SIZE as u16).to_le_bytes());
    buf[8..16].copy_from_slice(&block_id.to_le_bytes());
    buf[18..20].copy_from_slice(&0x0001u16.to_le_bytes()); // payload type: image
    buf[20..28].copy_from_slice(&(block_id * 1_000_000).to_le_bytes()); // timestamp
    buf[28..32].copy_from_slice(&pixel_format.to_le_bytes());
    buf[32..36].copy_from_slice(&width.to_le_bytes());
    buf[36..40].copy_from_slice(&height.to_le_bytes());
    buf
}

/// A 32-byte image trailer, as `dissect_u3v_stream_trailer` reads one.
fn build_trailer(block_id: u64, valid_payload_size: u64, height: u32) -> Vec<u8> {
    let mut buf = vec![0u8; TRAILER_SIZE as usize];
    buf[0..4].copy_from_slice(&TRAILER_PREFIX);
    buf[6..8].copy_from_slice(&(TRAILER_SIZE as u16).to_le_bytes());
    buf[8..16].copy_from_slice(&block_id.to_le_bytes());
    // status 0: success
    buf[20..28].copy_from_slice(&valid_payload_size.to_le_bytes());
    buf[28..32].copy_from_slice(&height.to_le_bytes());
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registers::{REG_ACQ_START, SIRM_BASE, SIRM_REQUIRED_LEADER_SIZE};
    use std::sync::Arc;
    use viva_u3v::device::U3vDevice;

    fn open(transport: &Arc<FakeU3vTransport>) -> U3vDevice<FakeU3vTransport> {
        U3vDevice::open(Arc::clone(transport), 0x81, 0x01, Some(STREAM_EP), None).unwrap()
    }

    #[test]
    fn fake_transport_open_device() {
        let transport = Arc::new(FakeU3vTransport::new(640, 480, 0x0108_0001));
        let device = open(&transport);
        assert_eq!(device.abrm().manufacturer_name, "FakeCorp");
        assert_eq!(device.abrm().model_name, "FakeU3V");
        assert_eq!(device.abrm().family_name, "Test");
        assert_eq!(device.sbrm().max_cmd_transfer, 4096);
        assert_eq!(device.sbrm().sirm_address, SIRM_BASE);
    }

    #[test]
    fn fake_transport_fetch_xml() {
        let transport = Arc::new(FakeU3vTransport::new(320, 240, 0x0108_0001));
        let xml = open(&transport).fetch_xml().unwrap();
        assert!(xml.contains("RegisterDescription"));
        assert!(xml.contains("Width"));
    }

    /// The fake refuses what a device refuses, and records the attempt.
    #[test]
    fn a_write_only_register_read_is_refused_and_recorded() {
        let transport = Arc::new(FakeU3vTransport::new(64, 64, 0x0108_0001));
        let mut device = open(&transport);
        transport.clear_reads();

        let err = device
            .read_mem(REG_ACQ_START, 4)
            .expect_err("a WO register must refuse a read");
        assert!(
            matches!(
                err,
                U3vError::Status {
                    status: StatusCode::AccessDenied
                }
            ),
            "{err:?}"
        );
        assert!(transport.was_read(REG_ACQ_START));

        device
            .write_mem(REG_ACQ_START, &1u32.to_le_bytes())
            .expect("a WO register still accepts a write");
    }

    #[test]
    fn a_required_size_refuses_a_write() {
        let transport = Arc::new(FakeU3vTransport::new(64, 64, 0x0108_0001));
        let err = open(&transport)
            .write_mem(SIRM_REQUIRED_LEADER_SIZE, &64u32.to_le_bytes())
            .unwrap_err();
        assert!(
            matches!(
                err,
                U3vError::Status {
                    status: StatusCode::WriteProtect
                }
            ),
            "{err:?}"
        );
    }

    #[test]
    fn an_unmapped_address_is_refused() {
        let transport = Arc::new(FakeU3vTransport::new(64, 64, 0x0108_0001));
        let err = open(&transport).read_mem(0x0228, 8).unwrap_err();
        assert!(
            matches!(
                err,
                U3vError::Status {
                    status: StatusCode::InvalidAddress
                }
            ),
            "{err:?}"
        );
    }

    /// Nothing streams until the host has enabled the stream and started
    /// acquisition; then blocks arrive in the configured transfers.
    #[test]
    fn a_stream_flows_only_when_enabled_and_acquiring() {
        let transport = Arc::new(FakeU3vTransport::new(64, 48, 0x0108_0001));
        let mut device = open(&transport);
        let mut stream = device.open_stream(0).unwrap();
        assert!(transport.stream_enabled());
        assert!(matches!(
            stream.next_frame_within(Duration::from_millis(1)),
            Ok(None)
        ));

        device
            .write_mem(REG_ACQ_START, &1u32.to_le_bytes())
            .unwrap();
        let frame = stream.next_frame().unwrap();
        assert_eq!((frame.leader.width, frame.leader.height), (64, 48));
        assert_eq!(frame.payload.len(), 64 * 48);
        assert_eq!(frame.trailer.block_id, frame.leader.block_id);

        device.stop_stream().unwrap();
        assert!(!transport.stream_enabled());
    }
}

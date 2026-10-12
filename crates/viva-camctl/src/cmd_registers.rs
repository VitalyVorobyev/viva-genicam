//! `read-reg` and `write-reg`: raw register access, past the GenApi XML — so
//! it works on a camera whose XML we cannot read.
//!
//! GigE Vision goes through GVCP: `READREG`/`WRITEREG` for one 32-bit word,
//! `READMEM` for more. GVCP carries whole 32-bit words at 32-bit aligned
//! addresses, so an unaligned address or length is refused here rather than
//! sent. USB3 Vision goes through GenCP memory reads and writes, which take
//! any address and length.
//!
//! A 32-bit value is shown and written in the device's register byte order:
//! big-endian on GigE Vision, little-endian on USB3 Vision (the order its
//! bootstrap registers declare, see `viva_u3v::bootstrap`).

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;

use viva_gige::gvcp::GigeDevice;
use viva_gige::nic::IfaceSelector;
use viva_u3v::device::U3vDevice;
use viva_u3v::usb::UsbTransfer;

use crate::common::{self, DEFAULT_DISCOVERY_TIMEOUT_MS, Target};

/// The byte order of a device's registers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WordOrder {
    /// GigE Vision.
    Big,
    /// USB3 Vision.
    Little,
}

impl WordOrder {
    fn word(self, bytes: [u8; 4]) -> u32 {
        match self {
            WordOrder::Big => u32::from_be_bytes(bytes),
            WordOrder::Little => u32::from_le_bytes(bytes),
        }
    }

    fn bytes(self, value: u32) -> [u8; 4] {
        match self {
            WordOrder::Big => value.to_be_bytes(),
            WordOrder::Little => value.to_le_bytes(),
        }
    }
}

/// Refuse what GVCP cannot carry: an address or a length that is not a
/// whole number of 32-bit words.
fn check_gvcp_alignment(addr: u64, len: usize) -> Result<()> {
    if !addr.is_multiple_of(4) {
        bail!("GVCP addresses registers in 32-bit words: {addr:#x} is not a multiple of 4");
    }
    if len == 0 || !len.is_multiple_of(4) {
        bail!("GVCP reads whole 32-bit words: a length of {len} bytes is not a multiple of 4");
    }
    Ok(())
}

/// Read `len` bytes at `addr` from a GigE Vision camera: `READREG` for one
/// word, `READMEM` for more.
pub async fn read_gige(device: &mut GigeDevice, addr: u64, len: usize) -> Result<Vec<u8>> {
    check_gvcp_alignment(addr, len)?;
    device
        .read_register_or_mem(addr, len)
        .await
        .with_context(|| format!("read {len} bytes at {addr:#x}"))
}

/// Write one 32-bit word at `addr` to a GigE Vision camera, holding control
/// privilege only for the write.
pub async fn write_gige(device: &mut GigeDevice, addr: u64, value: u32) -> Result<()> {
    check_gvcp_alignment(addr, 4)?;
    device
        .claim_control()
        .await
        .context("claim control privilege (another application may hold it)")?;
    let written = device
        .write_register_or_mem(addr, &WordOrder::Big.bytes(value))
        .await
        .with_context(|| format!("write {value:#x} at {addr:#x}"));
    if let Err(err) = device.release_control().await {
        eprintln!("warning: could not release control privilege: {err}");
    }
    written
}

/// Read `len` bytes at `addr` from a USB3 Vision camera.
pub fn read_usb<T: UsbTransfer>(
    device: &mut U3vDevice<T>,
    addr: u64,
    len: usize,
) -> Result<Vec<u8>> {
    if len == 0 {
        bail!("nothing to read: the length is 0");
    }
    device
        .read_mem(addr, len)
        .map_err(|err| anyhow!("read {len} bytes at {addr:#x}: {err}"))
}

/// Write one 32-bit word, little-endian, at `addr` to a USB3 Vision camera.
pub fn write_usb<T: UsbTransfer>(device: &mut U3vDevice<T>, addr: u64, value: u32) -> Result<()> {
    device
        .write_mem(addr, &WordOrder::Little.bytes(value))
        .map_err(|err| anyhow!("write {value:#x} at {addr:#x}: {err}"))
}

/// What `read-reg` printed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RegisterRead {
    pub address: String,
    /// The bytes, as two hexadecimal digits each, in the order read.
    pub bytes: String,
    /// The 32-bit value, in the device's byte order, when four bytes were
    /// read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<u32>,
}

impl RegisterRead {
    pub fn new(addr: u64, bytes: &[u8], order: WordOrder) -> Self {
        RegisterRead {
            address: format!("{addr:#010x}"),
            bytes: bytes
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<Vec<_>>()
                .join(" "),
            value: <[u8; 4]>::try_from(bytes).ok().map(|word| order.word(word)),
        }
    }
}

impl std::fmt::Display for RegisterRead {
    /// `0x00000938: 00 00 0b b8 = 0x00000bb8 (3000)`
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.address, self.bytes)?;
        if let Some(value) = self.value {
            write!(f, " = {value:#010x} ({value})")?;
        }
        Ok(())
    }
}

/// Open the control channel of the GigE Vision camera `target` names,
/// without fetching its XML.
async fn open_gige(target: &Target, iface: Option<&IfaceSelector>) -> Result<GigeDevice> {
    let timeout = Duration::from_millis(DEFAULT_DISCOVERY_TIMEOUT_MS);
    let device = common::select_device(target.ip, target.index, iface, timeout).await?;
    common::open_control(&device).await
}

fn open_usb(index: usize) -> Result<U3vDevice<viva_u3v::usb::RusbTransfer>> {
    let info = viva_u3v::discovery::discover()
        .map_err(|err| anyhow!("USB discovery failed: {err}"))?
        .into_iter()
        .nth(index)
        .ok_or_else(|| anyhow!("no USB3 Vision device at index {index}"))?;
    U3vDevice::open_device(&info).map_err(|err| anyhow!("open the USB3 Vision camera: {err}"))
}

pub async fn run_read(
    target: Target,
    iface: Option<IfaceSelector>,
    addr: u64,
    len: usize,
    json: bool,
) -> Result<()> {
    let (bytes, order) = match target.usb {
        Some(index) => (
            read_usb(&mut open_usb(index)?, addr, len)?,
            WordOrder::Little,
        ),
        None => {
            let mut device = open_gige(&target, iface.as_ref()).await?;
            (read_gige(&mut device, addr, len).await?, WordOrder::Big)
        }
    };
    let read = RegisterRead::new(addr, &bytes, order);
    if json {
        return common::print_json(&read);
    }
    println!("{read}");
    Ok(())
}

pub async fn run_write(
    target: Target,
    iface: Option<IfaceSelector>,
    addr: u64,
    value: u32,
) -> Result<()> {
    match target.usb {
        Some(index) => write_usb(&mut open_usb(index)?, addr, value)?,
        None => {
            let mut device = open_gige(&target, iface.as_ref()).await?;
            write_gige(&mut device, addr, value).await?;
        }
    }
    println!("wrote {value:#010x} ({value}) at {addr:#010x}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gvcp_takes_whole_aligned_words_only() {
        assert!(check_gvcp_alignment(0x0938, 4).is_ok());
        assert!(check_gvcp_alignment(0x0200, 512).is_ok());
        assert!(check_gvcp_alignment(0x0939, 4).is_err());
        assert!(check_gvcp_alignment(0x0938, 2).is_err());
        assert!(check_gvcp_alignment(0x0938, 0).is_err());
    }

    #[test]
    fn a_word_is_shown_in_the_devices_byte_order() {
        let gige = RegisterRead::new(0x938, &[0, 0, 0x0b, 0xb8], WordOrder::Big);
        assert_eq!(
            gige.to_string(),
            "0x00000938: 00 00 0b b8 = 0x00000bb8 (3000)"
        );
        let usb = RegisterRead::new(0x10, &[0xb8, 0x0b, 0, 0], WordOrder::Little);
        assert_eq!(usb.value, Some(3000));
        let long = RegisterRead::new(0x0, &[1, 2, 3, 4, 5, 6, 7, 8], WordOrder::Big);
        assert_eq!(long.to_string(), "0x00000000: 01 02 03 04 05 06 07 08");
    }
}

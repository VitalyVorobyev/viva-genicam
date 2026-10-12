//! GigE Vision commands addressed to the network rather than to one open
//! camera: action commands and FORCEIP.

use std::net::{Ipv4Addr, SocketAddr};

use pyo3::prelude::*;
use viva_genicam::gige;

use crate::camera::iface_from_str;
use crate::errors::{parse_error, transport_error};
use crate::runtime::runtime;

fn parse_ip(what: &str, text: &str) -> PyResult<Ipv4Addr> {
    text.parse()
        .map_err(|err| parse_error(format!("{what} '{text}': {err}")))
}

/// A MAC address written as six hexadecimal bytes separated by `:` or `-`.
pub(crate) fn parse_mac(text: &str) -> PyResult<[u8; 6]> {
    let parts: Vec<&str> = text.split([':', '-']).collect();
    let invalid = || {
        parse_error(format!(
            "MAC address '{text}': expected six hexadecimal bytes, such as DE:AD:BE:EF:CA:FE"
        ))
    };
    if parts.len() != 6 {
        return Err(invalid());
    }
    let mut mac = [0u8; 6];
    for (byte, part) in mac.iter_mut().zip(parts) {
        if part.len() != 2 {
            return Err(invalid());
        }
        *byte = u8::from_str_radix(part, 16).map_err(|_| invalid())?;
    }
    Ok(mac)
}

/// Send an `ACTION_CMD` to `destination` and count the acknowledgements
/// within `timeout_ms`: `(sent, acknowledged)`.
#[pyfunction]
#[pyo3(signature = (device_key, group_key, group_mask, scheduled_time=None, destination="255.255.255.255", timeout_ms=200))]
fn send_action(
    py: Python<'_>,
    device_key: u32,
    group_key: u32,
    group_mask: u32,
    scheduled_time: Option<u64>,
    destination: &str,
    timeout_ms: u64,
) -> PyResult<(usize, usize)> {
    let ip = parse_ip("destination", destination)?;
    let params = gige::ActionParams {
        device_key,
        group_key,
        group_mask,
        scheduled_time,
    };
    let dest = SocketAddr::from((ip, gige::GVCP_PORT));
    let summary = py
        .detach(|| runtime().block_on(gige::send_action(dest, &params, timeout_ms)))
        .map_err(|err| transport_error(format!("action command to {ip}: {err}")))?;
    Ok((summary.sent, summary.acks))
}

/// Broadcast a FORCEIP: give the camera with `mac` a temporary address.
#[pyfunction]
#[pyo3(signature = (mac, ip, subnet, gateway="0.0.0.0", iface=None))]
fn force_ip(
    py: Python<'_>,
    mac: &str,
    ip: &str,
    subnet: &str,
    gateway: &str,
    iface: Option<&str>,
) -> PyResult<()> {
    let mac = parse_mac(mac)?;
    let ip = parse_ip("ip", ip)?;
    let subnet = parse_ip("subnet", subnet)?;
    let gateway = parse_ip("gateway", gateway)?;
    let iface = iface.map(iface_from_str).transpose()?;
    py.detach(|| runtime().block_on(gige::force_ip(mac, ip, subnet, gateway, iface.as_ref())))
        .map_err(|err| transport_error(format!("FORCEIP: {err}")))
}

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(send_action, m)?)?;
    m.add_function(wrap_pyfunction!(force_ip, m)?)?;
    Ok(())
}

//! Network interface utilities for GigE Vision streaming.
//!
//! This module provides helpers for querying network interface capabilities and
//! constructing UDP sockets tuned for high-throughput GVSP traffic. The
//! functionality is intentionally conservative so it can operate on most Unix
//! like systems without additional privileges. Platform specific code paths are
//! gated via conditional compilation and otherwise fall back to sane defaults.

use std::collections::VecDeque;
#[cfg(any(target_os = "linux", target_os = "android"))]
use std::fs;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Mutex;

use bytes::BytesMut;
use if_addrs::IfAddr;
use socket2::{Domain, Protocol, SockRef, Socket, Type};
use tokio::net::UdpSocket;
use tracing::info;

#[cfg(any(target_os = "linux", target_os = "android"))]
use tracing::warn;

/// Default socket receive buffer size used when the caller does not provide a
/// custom value. The number mirrors what many operating systems allow without
/// requiring elevated privileges.
pub const DEFAULT_RCVBUF_BYTES: usize = 4 << 20; // 4 MiB

/// Maximum size for interface names supported by the kernel on Linux. The
/// constant is used to validate the provided names before using them when
/// interfacing with low level sysfs files.
const IFACE_NAME_MAX: usize = 15; // As per `IFNAMSIZ - 1`.

/// Resolve an interface index from its name using platform-specific APIs.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn iface_name_to_index(name: &str) -> io::Result<u32> {
    let index_path = format!("/sys/class/net/{name}/ifindex");
    fs::read_to_string(&index_path)
        .map_err(|err| io::Error::new(err.kind(), format!("{index_path}: {err}")))?
        .trim()
        .parse::<u32>()
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

/// Resolve an interface index from its name using `if_nametoindex(3)`.
#[cfg(all(
    unix,
    not(any(target_os = "linux", target_os = "android"))
))]
fn iface_name_to_index(name: &str) -> io::Result<u32> {
    use std::ffi::CString;

    let c_name = CString::new(name).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("interface name contains null byte: '{name}'"),
        )
    })?;
    // SAFETY: `if_nametoindex` is a POSIX function that takes a valid C string.
    let index = unsafe { libc::if_nametoindex(c_name.as_ptr()) };
    if index == 0 {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("interface '{name}' not found"),
        ))
    } else {
        Ok(index)
    }
}

/// Resolve an interface index from its name on Windows.
#[cfg(target_os = "windows")]
fn iface_name_to_index(name: &str) -> io::Result<u32> {
    // On Windows, interface indices are not easily resolved by name via libc.
    // Fall back to iterating if_addrs to find a matching interface.
    for (idx, iface) in if_addrs::get_if_addrs()?.iter().enumerate() {
        if iface.name == name {
            return Ok((idx + 1) as u32);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("interface '{name}' not found"),
    ))
}

/// Representation of a host network interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Iface {
    name: String,
    index: u32,
    ipv4: Option<Ipv4Addr>,
    ipv6: Option<Ipv6Addr>,
}

impl Iface {
    /// Resolve an interface from the operating system by its name.
    pub fn from_system(name: &str) -> io::Result<Self> {
        if name.is_empty() || name.len() > IFACE_NAME_MAX {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid interface name '{name}'"),
            ));
        }

        let index = iface_name_to_index(name)?;

        let mut ipv4 = None;
        let mut ipv6 = None;
        for iface in if_addrs::get_if_addrs()? {
            if iface.name != name {
                continue;
            }
            match iface.addr {
                IfAddr::V4(v4) => ipv4 = Some(v4.ip),
                IfAddr::V6(v6) => ipv6 = Some(v6.ip),
            }
        }

        Ok(Self {
            name: name.to_string(),
            index,
            ipv4,
            ipv6,
        })
    }

    /// Resolve an interface by its primary IPv4 address.
    pub fn from_ipv4(addr: Ipv4Addr) -> io::Result<Self> {
        for iface in if_addrs::get_if_addrs()? {
            if let IfAddr::V4(v4) = iface.addr {
                if v4.ip == addr {
                    return Self::from_system(&iface.name);
                }
            }
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("no interface with IPv4 {addr}"),
        ))
    }

    /// Interface name as provided by the operating system (e.g. `eth0`).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Interface index as reported by the kernel. The index is used by some
    /// socket options (e.g. multicast subscriptions).
    pub fn index(&self) -> u32 {
        self.index
    }

    /// Primary IPv4 address associated with the interface, if any.
    pub fn ipv4(&self) -> Option<Ipv4Addr> {
        self.ipv4
    }

    /// Primary IPv6 address associated with the interface, if any.
    #[allow(dead_code)]
    pub fn ipv6(&self) -> Option<Ipv6Addr> {
        self.ipv6
    }
}

/// Read the MTU configured for the provided interface.
///
/// On Linux the value is obtained from `/sys/class/net/<iface>/mtu` to avoid
/// platform specific `ioctl` calls. The function falls back to the canonical
/// Ethernet MTU (1500 bytes) when the information cannot be fetched.
pub fn mtu(_iface: &Iface) -> io::Result<u32> {
    #[cfg(target_os = "linux")]
    {
        let path = format!("/sys/class/net/{}/mtu", _iface.name());
        match fs::read_to_string(path) {
            Ok(contents) => {
                let mtu = contents
                    .trim()
                    .parse::<u32>()
                    .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
                tracing::debug!(name = _iface.name(), mtu, "resolved interface MTU");
                return Ok(mtu);
            }
            Err(err) => {
                warn!(name = _iface.name(), error = %err, "failed to read MTU, using default");
            }
        }
    }

    Ok(1500)
}

/// Compute an optimal GVSP payload size from the link MTU.
///
/// The resulting value subtracts the Ethernet, IPv4 and UDP headers to produce
/// the maximum amount of user payload that fits in a single packet.
pub fn best_packet_size(mtu: u32) -> u32 {
    const ETHERNET_L2: u32 = 14; // Ethernet II header without VLAN tags.
    const IPV4_HEADER: u32 = 20; // RFC 791 minimum header size.
    const UDP_HEADER: u32 = 8; // RFC 768 header size.

    mtu.saturating_sub(ETHERNET_L2 + IPV4_HEADER + UDP_HEADER)
}

/// Multicast socket options applied while binding.
#[derive(Debug, Clone)]
pub struct McOptions {
    /// Whether multicast packets sent locally should be looped back.
    pub loopback: bool,
    /// IPv4 time-to-live for outbound multicast packets.
    pub ttl: u32,
    /// Receive buffer size in bytes.
    pub rcvbuf_bytes: usize,
    /// Whether to enable address/port reuse when binding.
    pub reuse_addr: bool,
}

impl Default for McOptions {
    fn default() -> Self {
        Self {
            loopback: false,
            ttl: 1,
            rcvbuf_bytes: DEFAULT_RCVBUF_BYTES,
            reuse_addr: true,
        }
    }
}

/// Bind a UDP socket configured for GVSP traffic.
pub async fn bind_udp(
    bind: IpAddr,
    port: u16,
    iface: Option<Iface>,
    recv_buffer: Option<usize>,
) -> io::Result<UdpSocket> {
    let recv_buffer = recv_buffer.unwrap_or(DEFAULT_RCVBUF_BYTES);
    if let Some(ipv4) = iface.as_ref().and_then(|iface| iface.ipv4()) {
        info!(name = iface.as_ref().map(Iface::name), %ipv4, port, "binding GVSP socket");
    } else {
        info!(%bind, port, "binding GVSP socket");
    }

    let domain = match bind {
        IpAddr::V4(_) => Domain::IPV4,
        IpAddr::V6(_) => Domain::IPV6,
    };
    let socket = Socket::new(domain, Type::DGRAM, Some(Protocol::UDP))?;

    socket.set_reuse_address(true)?;
    #[cfg(all(unix, not(target_os = "solaris")))]
    socket.set_reuse_port(true)?;

    socket.set_recv_buffer_size(recv_buffer)?;

    #[cfg(any(target_os = "linux", target_os = "android"))]
    if let Some(iface) = iface.as_ref() {
        if let Err(err) = socket.bind_device(Some(iface.name().as_bytes())) {
            warn!(name = iface.name(), error = %err, "SO_BINDTODEVICE failed");
        }
    }

    let addr = SocketAddr::new(bind, port);
    socket.bind(&addr.into())?;

    let std_socket: std::net::UdpSocket = socket.into();
    std_socket.set_nonblocking(true)?;
    UdpSocket::from_std(std_socket)
}

fn validate_multicast_inputs(group: Ipv4Addr, ttl: u32) -> io::Result<()> {
    if ttl > 255 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "multicast TTL must be <= 255",
        ));
    }
    if (group.octets()[0] & 0xF0) != 0xE0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "multicast group must be within 224.0.0.0/4",
        ));
    }
    Ok(())
}

/// Bind a UDP socket subscribed to the provided multicast group on the interface.
pub async fn bind_multicast(
    iface: &Iface,
    group: Ipv4Addr,
    port: u16,
    opts: &McOptions,
) -> io::Result<UdpSocket> {
    validate_multicast_inputs(group, opts.ttl)?;
    let iface_addr = iface
        .ipv4()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "interface lacks IPv4"))?;

    info!(name = iface.name(), %group, port, "binding multicast GVSP socket");

    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;

    if opts.reuse_addr {
        socket.set_reuse_address(true)?;
        #[cfg(all(unix, not(target_os = "solaris")))]
        socket.set_reuse_port(true)?;
    }

    socket.set_recv_buffer_size(opts.rcvbuf_bytes)?;
    socket.set_multicast_loop_v4(opts.loopback)?;
    socket.set_multicast_ttl_v4(opts.ttl)?;
    socket.set_multicast_if_v4(&iface_addr)?;

    #[cfg(any(target_os = "linux", target_os = "android"))]
    if let Err(err) = socket.bind_device(Some(iface.name().as_bytes())) {
        warn!(name = iface.name(), error = %err, "SO_BINDTODEVICE failed");
    }

    let bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port);
    socket.bind(&bind_addr.into())?;
    socket.join_multicast_v4(&group, &iface_addr)?;

    let std_socket: std::net::UdpSocket = socket.into();
    std_socket.set_nonblocking(true)?;
    UdpSocket::from_std(std_socket)
}

/// Subscribe the provided socket to a multicast group on the supplied interface.
pub fn join_multicast(sock: &UdpSocket, group: Ipv4Addr, iface: &Iface) -> io::Result<()> {
    let socket = SockRef::from(sock);
    let iface_addr = iface.ipv4().unwrap_or(Ipv4Addr::UNSPECIFIED);
    socket.join_multicast_v4(&group, &iface_addr)?;
    Ok(())
}

/// Simple lock-free pool for reusable buffers backing frame assembly.
#[derive(Debug)]
pub struct BufferPool {
    buffers: Mutex<VecDeque<BytesMut>>,
    size: usize,
}

impl BufferPool {
    /// Create a pool with the given capacity and buffer size.
    pub fn new(capacity: usize, size: usize) -> Self {
        let mut buffers = VecDeque::with_capacity(capacity);
        for _ in 0..capacity {
            buffers.push_back(BytesMut::with_capacity(size));
        }
        Self {
            buffers: Mutex::new(buffers),
            size,
        }
    }

    /// Acquire a buffer from the pool.
    pub fn acquire(&self) -> Option<BytesMut> {
        self.buffers
            .lock()
            .ok()
            .and_then(|mut guard| guard.pop_front())
    }

    /// Return a buffer to the pool.
    pub fn release(&self, mut buffer: BytesMut) {
        buffer.truncate(0);
        buffer.reserve(self.size);
        if let Ok(mut guard) = self.buffers.lock() {
            guard.push_back(buffer);
        }
    }
}

/// Helper returning the default bind address for discovery convenience.
pub fn default_bind_addr() -> IpAddr {
    IpAddr::V4(Ipv4Addr::UNSPECIFIED)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reject_invalid_ttl() {
        let err = validate_multicast_inputs(Ipv4Addr::new(239, 0, 0, 1), 512).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn reject_non_multicast_group() {
        let err = validate_multicast_inputs(Ipv4Addr::new(192, 168, 1, 1), 1).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn accept_valid_group() {
        assert!(validate_multicast_inputs(Ipv4Addr::new(239, 192, 1, 10), 1).is_ok());
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn from_system_loopback() {
        let lo_name = if cfg!(target_os = "macos") {
            "lo0"
        } else {
            "lo"
        };
        let iface = Iface::from_system(lo_name).expect("loopback interface should exist");
        assert!(iface.ipv4().unwrap().is_loopback());
    }

    #[test]
    fn packet_size_respects_headers() {
        let mtu = 1500;
        let size = best_packet_size(mtu);
        assert!(size < mtu);
        assert_eq!(size, 1500 - (14 + 20 + 8));
    }

    #[test]
    fn buffer_pool_recycles() {
        let pool = BufferPool::new(2, 1024);
        let mut buf = pool.acquire().expect("buffer");
        buf.extend_from_slice(&[1, 2, 3]);
        pool.release(buf);
        let buf2 = pool.acquire().expect("buffer");
        assert!(buf2.is_empty());
        assert!(buf2.capacity() >= 1024);
    }
}

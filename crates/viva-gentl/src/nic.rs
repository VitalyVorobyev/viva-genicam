//! The host's network interfaces: one GenTL Interface module each.
//!
//! GenTL §2.2.2: "An Interface module represents one physical interface in
//! the system. For Ethernet based transport layer technologies, this would
//! be a Network Interface Card." GigE Vision runs over IPv4, so an interface
//! without an IPv4 address carries no camera and is not listed. Loopback is
//! listed: a camera there is a simulator, which is how the producer is
//! tested (ADR-0031).

use std::net::Ipv4Addr;

use crate::abi::GC_ERR_IO;
use crate::error::{Failure, Result};

/// One host interface and its IPv4 subnets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Nic {
    /// The operating system's name for it: the Interface module's ID.
    pub(crate) name: String,
    /// Its IPv4 addresses and their netmasks, in the order the OS lists them.
    pub(crate) subnets: Vec<(Ipv4Addr, Ipv4Addr)>,
}

impl Nic {
    /// The address its first subnet has (`GevInterfaceDefaultIPAddress`).
    pub(crate) fn address(&self) -> Ipv4Addr {
        self.subnets.first().map_or(Ipv4Addr::UNSPECIFIED, |s| s.0)
    }

    /// The netmask of its first subnet.
    pub(crate) fn netmask(&self) -> Ipv4Addr {
        self.subnets.first().map_or(Ipv4Addr::UNSPECIFIED, |s| s.1)
    }

    /// `INTERFACE_INFO_DISPLAYNAME`: the name and the first address.
    pub(crate) fn display_name(&self) -> String {
        format!("{} ({}, GigE Vision)", self.name, self.address())
    }

    /// Whether `id` names this interface: its name, or one of its addresses
    /// (a convenience name, §3.2).
    pub(crate) fn answers_to(&self, id: &str) -> bool {
        self.name == id
            || id
                .parse::<Ipv4Addr>()
                .is_ok_and(|ip| self.subnets.iter().any(|(own, _)| *own == ip))
    }
}

/// Every interface with an IPv4 address, sorted by name.
pub(crate) fn list() -> Result<Vec<Nic>> {
    let mut nics: Vec<Nic> = Vec::new();
    let addrs = if_addrs::get_if_addrs()
        .map_err(|err| Failure::new(GC_ERR_IO, format!("list network interfaces: {err}")))?;
    for iface in addrs {
        let if_addrs::IfAddr::V4(v4) = iface.addr else {
            continue;
        };
        match nics.iter_mut().find(|nic| nic.name == iface.name) {
            Some(nic) => nic.subnets.push((v4.ip, v4.netmask)),
            None => nics.push(Nic {
                name: iface.name,
                subnets: vec![(v4.ip, v4.netmask)],
            }),
        }
    }
    nics.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(nics)
}

/// The interface `id` names, from the system's list or, failing that, the
/// host as it is now (§3.2: an interface may be opened "without a previous
/// enumeration").
pub(crate) fn find(listed: &[Nic], id: &str) -> Result<Option<Nic>> {
    if let Some(nic) = listed.iter().find(|nic| nic.answers_to(id)) {
        return Ok(Some(nic.clone()));
    }
    Ok(list()?.into_iter().find(|nic| nic.answers_to(id)))
}

#![cfg_attr(docsrs, feature(doc_cfg))]
//! GigE Vision TL: discovery (GVCP), control (GenCP/GVCP), streaming (GVSP).

pub mod action;
pub mod gvcp;
pub mod gvsp;
pub mod message;
pub mod nic;
pub mod stats;
pub mod time;

pub use gvcp::{
    DeviceInfo, DiscoveryOptions, GVCP_PORT, GigeDevice, GigeError, GvcpCapability, discover,
    discover_all, discover_ip, discover_on_interface, discover_with, force_ip,
};

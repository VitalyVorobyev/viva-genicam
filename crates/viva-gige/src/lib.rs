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
    DeviceInfo, GVCP_PORT, GigeDevice, GigeError, discover, discover_all, discover_on_interface,
};

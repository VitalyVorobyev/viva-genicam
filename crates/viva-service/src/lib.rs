//! The camera services' library: bridges viva-genicam to Zenoh for Viva
//! Studio.
//!
//! Both services run on it — `viva-service` for GigE Vision and
//! `viva-service-u3v` for USB3 Vision — so they answer every query, publish
//! every stream and follow every camera's lifecycle the same way (backlog
//! API-04). A transport supplies a [`Discovery`], whose cameras are
//! [`DeviceOps`] for their features and a [`StreamSource`] for their
//! frames and events; [`run_discovery`] does the rest. [`serve_device`] runs
//! one camera's task set, for tests that serve a camera in-process.

mod acquisition;
mod device;
mod discovery;
mod events;
mod gige;
mod nodes;
mod pixel_format;
mod report;
mod serve;
mod source;
mod status;
mod xml;

pub use device::{DeviceOps, SharedCamera};
pub use discovery::{Discovery, init_tracing, run_discovery, run_service};
pub use gige::{
    DeviceHandle, GigeDiscovery, GigeFound, derive_device_id, device_announce, publish_announce,
};
pub use serve::serve_device;
pub use source::{BlockStream, EventReceiver, StreamSource};

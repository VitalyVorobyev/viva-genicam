//! Backend abstraction for device communication.
//!
//! The app supports two operating modes:
//! - **Embedded** (default): direct GigE/USB3 camera access via `viva-genicam`.
//! - **Remote**: camera access through `viva-service` over Zenoh.
//!
//! Both implement [`DeviceBackend`], and the Tauri commands call only the
//! trait: no command asks which mode it is in (ADR-0024, backlog ST-27).

pub(crate) mod camera;
pub mod embedded;
pub mod remote;

use std::collections::HashMap;
use std::net::Ipv4Addr;

use async_trait::async_trait;
use viva_zenoh_api::{EventMonitorStatus, FeatureState};

use crate::state::device_state::{ConnectionAccess, DeviceInfo, StreamerInfo};

/// Which backend mode the application is currently using.
#[derive(Debug, Clone, serde::Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub enum BackendMode {
    /// Direct camera communication (GigE + USB3) embedded in the app process.
    Embedded,
    /// Camera access via an external service process over Zenoh.
    Remote,
}

/// Result of a successful device connection.
pub struct ConnectResult {
    /// Raw GenICam XML fetched from the device.
    pub xml: String,
    /// Human-readable device name (user-defined name or model fallback).
    pub device_name: String,
    /// Device model string.
    pub model: String,
}

/// How to start an acquisition.
#[derive(Debug, Clone, Copy, Default)]
pub struct StartOptions {
    /// The GVSP packet size to use, as a ceiling the path probe stays under;
    /// `None` negotiates one from the NIC's MTU (backlog ST-22, issue #112).
    pub packet_size: Option<u32>,
}

/// Network configuration of a GigE Vision camera.
#[derive(Debug, Clone, serde::Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct NetworkConfig {
    pub current_ip: String,
    pub persistent_ip: String,
    pub persistent_subnet: String,
    pub persistent_gateway: String,
    pub mac: String,
}

/// Device communication, implemented by both modes.
#[async_trait]
pub trait DeviceBackend: Send + Sync + 'static {
    /// Return the current backend mode.
    fn mode(&self) -> BackendMode;

    /// The cameras found so far.
    async fn discover(&self) -> Vec<DeviceInfo>;

    /// Connect to a device by the `id` its discovery reported — for GigE the
    /// MAC-derived `viva_zenoh_api::gige_device_id`, in both modes.
    async fn connect(&self, device_id: &str) -> Result<ConnectResult, String>;

    /// Select direct access; unsupported transports refuse monitor mode.
    async fn connect_as(
        &self,
        device_id: &str,
        access: ConnectionAccess,
    ) -> Result<ConnectResult, String> {
        if access != ConnectionAccess::Control {
            return Err("Direct monitor access requires an embedded GigE Vision camera; remote services own camera access".into());
        }
        self.connect(device_id).await
    }

    /// Open and publish one camera model. Embedded sessions override this to
    /// keep the complete transition under their cancellation/generation guard.
    async fn connect_model(
        &self,
        device_id: &str,
        access: ConnectionAccess,
        connection: &std::sync::Arc<crate::state::connection::Connection>,
        model: &crate::state::SharedModelState,
    ) -> Result<crate::commands::xml_model::ParseXmlResponse, String> {
        if access != ConnectionAccess::Control {
            return Err("Direct monitor access is unavailable in remote mode; the service owns camera access".into());
        }
        use crate::commands::xml_model::parse_camera_model;
        use crate::state::device_state::ConnectionState;
        connection
            .set(ConnectionState::Connecting {
                device_id: device_id.into(),
            })
            .await;
        let result: Result<crate::commands::xml_model::ParseXmlResponse, String> = async {
            let result = self.connect_as(device_id, access).await?;
            let response = parse_camera_model(result.xml)?;
            model.write().await.update(
                response.xml.clone(),
                response.graph.clone(),
                response.diags.clone(),
                Some(device_id.into()),
            );
            connection
                .set(ConnectionState::Connected {
                    device_id: device_id.into(),
                    device_name: result.device_name,
                    model: result.model,
                    access: None,
                })
                .await;
            Ok(response)
        }
        .await;
        if let Err(message) = &result {
            self.disconnect().await.ok();
            connection
                .set(ConnectionState::Error {
                    message: message.clone(),
                })
                .await;
        }
        result
    }

    /// Finish the UI disconnect transition; embedded mode serializes publication.
    async fn disconnect_model(
        &self,
        connection: &std::sync::Arc<crate::state::connection::Connection>,
    ) -> Result<(), String> {
        let result = self.disconnect().await;
        connection
            .set(crate::state::device_state::ConnectionState::Disconnected)
            .await;
        result
    }

    /// Publish a command's immediate status without reviving an obsolete stream.
    async fn publish_acquisition_status(
        &self,
        active: bool,
        events: &crate::state::events::Events,
    ) {
        events.send(&if active {
            viva_zenoh_api::AcquisitionStatus::active(None, None)
        } else {
            viva_zenoh_api::AcquisitionStatus::inactive()
        });
    }

    /// Let go of the connected device, if any.
    async fn disconnect(&self) -> Result<(), String>;

    /// The live state of a feature: value, access mode, kind, range,
    /// accepted enum entries, unit, and implementation/availability flags —
    /// computed by `viva-genapi` in both modes (ST-26).
    async fn get_feature_state(&self, name: &str) -> Result<FeatureState, String>;

    /// [`DeviceBackend::get_feature_state`] for many features in one
    /// round-trip. A feature whose state cannot be read is left out.
    async fn bulk_feature_state(
        &self,
        names: &[String],
    ) -> Result<HashMap<String, FeatureState>, String>;

    /// Write a feature, and return the features the write may have changed
    /// (`NodeMap::invalidated_by`).
    async fn set_feature(
        &self,
        name: &str,
        value: &serde_json::Value,
    ) -> Result<Vec<String>, String>;

    /// Execute a command, and return the features it may have changed.
    async fn exec_command(&self, name: &str) -> Result<Vec<String>, String>;

    /// Start image acquisition and return the WebSocket URL for the stream.
    async fn start_acquisition(&self, options: StartOptions) -> Result<StreamerInfo, String>;

    /// Stop image acquisition.
    async fn stop_acquisition(&self) -> Result<(), String>;

    /// Turn notification on for the `EventSelector` entries `names` and
    /// forward each event as a `CameraEvent` (backlog ST-51), replacing any
    /// monitoring already on. Every change of monitoring is also sent as an
    /// `EventMonitorStatus`.
    async fn start_events(&self, names: Vec<String>) -> Result<EventMonitorStatus, String>;

    /// Stop forwarding events and restore the camera's event settings; the
    /// status lists what could not be restored. Disconnecting and exiting
    /// stop monitoring too.
    async fn stop_events(&self) -> Result<EventMonitorStatus, String>;

    /// Leave the camera free before the app exits; see `on_run_event` in
    /// `main.rs`.
    async fn shutdown(&self) {}

    /// The diagnostic report `viva-camctl report` writes, on the connected
    /// camera — or on this host and what discovery finds, if none is
    /// connected (backlog ST-41).
    async fn report(&self, include_xml: bool) -> Result<String, String>;

    // ── IP configuration (GigE, embedded mode) ──────────────────────────────

    /// Broadcast FORCEIP: give the camera with `mac` a temporary address.
    async fn force_ip(
        &self,
        _mac: [u8; 6],
        _ip: Ipv4Addr,
        _subnet: Ipv4Addr,
        _gateway: Ipv4Addr,
    ) -> Result<(), String> {
        Err("ForceIP requires embedded mode (direct network access)".to_string())
    }

    /// Read the network configuration of the connected camera.
    async fn get_network_config(&self) -> Result<NetworkConfig, String> {
        Err("Network configuration not supported in this mode".to_string())
    }

    /// Set persistent IP configuration on the connected camera.
    async fn set_persistent_ip(
        &self,
        _ip: Ipv4Addr,
        _subnet: Ipv4Addr,
        _gateway: Ipv4Addr,
    ) -> Result<(), String> {
        Err("Persistent IP not supported in this mode".to_string())
    }
}

/// Type alias for the managed backend state in Tauri.
pub type BackendState = std::sync::Arc<dyn DeviceBackend>;

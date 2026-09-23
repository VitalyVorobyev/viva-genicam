//! Backend abstraction for device communication.
//!
//! The app supports two operating modes:
//! - **Embedded** (default): direct GigE/USB3 camera access via `viva-genicam`.
//! - **Remote**: camera access through a separate service process via Zenoh.
//!
//! The [`DeviceBackend`] trait defines the interface that both modes implement,
//! allowing Tauri commands to delegate without knowing the transport.

pub mod embedded;
pub mod remote;

use std::collections::HashMap;

use async_trait::async_trait;
use viva_zenoh_api::FeatureState;

use crate::state::device_state::{DeviceInfo, NodeValueEntry, StreamerInfo};

/// Which backend mode the application is currently using.
#[derive(Debug, Clone, serde::Serialize)]
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

/// Network configuration of a GigE Vision camera.
#[derive(Debug, Clone, serde::Serialize)]
pub struct NetworkConfig {
    pub current_ip: String,
    pub persistent_ip: String,
    pub persistent_subnet: String,
    pub persistent_gateway: String,
    pub mac: String,
}

/// Trait abstracting device communication for both embedded and remote modes.
#[async_trait]
pub trait DeviceBackend: Send + Sync + 'static {
    /// Return the current backend mode.
    fn mode(&self) -> BackendMode;

    /// Discover available cameras on the network / USB bus.
    async fn discover(&self) -> Vec<DeviceInfo>;

    /// Connect to a device by the `id` its discovery reported — for GigE the
    /// MAC-derived `viva_zenoh_api::gige_device_id`, in both modes.
    async fn connect(&self, device_id: &str) -> Result<ConnectResult, String>;

    /// Disconnect from the currently connected device.
    async fn disconnect(&self, device_id: &str) -> Result<(), String>;

    /// Read a single feature value from the connected camera.
    ///
    /// Legacy projection of [`get_feature_state`] into the older shape kept
    /// for compatibility during the [`FeatureState`] migration. New callers
    /// should prefer `get_feature_state`.
    async fn get_feature(&self, name: &str) -> Result<NodeValueEntry, String>;

    /// Read the full live state of a feature: value, access mode, kind, range,
    /// available enum entries, unit, and implementation/availability flags.
    ///
    /// This is the authoritative snapshot the UI consumes as its single source
    /// of truth. The default implementation projects from [`get_feature`] with
    /// `kind: "Unknown"` and no range / enum data — good enough for remote
    /// mode stubs during migration. Embedded backends override this to perform
    /// typed reads and return full introspection.
    async fn get_feature_state(&self, name: &str) -> Result<FeatureState, String> {
        let entry = self.get_feature(name).await?;
        let numeric = match (entry.min, entry.max) {
            (Some(min), Some(max)) => Some(viva_zenoh_api::NumericRange {
                min,
                max,
                inc: entry.inc,
            }),
            _ => None,
        };
        Ok(FeatureState {
            value: entry.value,
            access_mode: entry.access_mode,
            kind: "Unknown".to_string(),
            is_implemented: true,
            is_available: true,
            numeric,
            enum_available: None,
            unit: None,
        })
    }

    /// Write a feature value on the connected camera.
    async fn set_feature(&self, name: &str, value: &serde_json::Value) -> Result<(), String>;

    /// Execute a command feature on the connected camera.
    async fn exec_command(&self, name: &str) -> Result<(), String>;

    /// Read multiple feature values in a single round-trip.
    async fn bulk_read(&self, names: &[String]) -> Result<HashMap<String, NodeValueEntry>, String>;

    /// Bulk variant of [`get_feature_state`]. Default implementation loops; an
    /// embedded backend can override for a single camera-lock acquisition.
    async fn bulk_feature_state(
        &self,
        names: &[String],
    ) -> Result<HashMap<String, FeatureState>, String> {
        let mut out = HashMap::with_capacity(names.len());
        for name in names {
            if let Ok(state) = self.get_feature_state(name).await {
                out.insert(name.clone(), state);
            }
        }
        Ok(out)
    }

    /// Start image acquisition and return the WebSocket URL for the stream.
    async fn start_acquisition(&self) -> Result<StreamerInfo, String>;

    /// Stop image acquisition.
    async fn stop_acquisition(&self) -> Result<(), String>;

    /// Return the raw GenICam XML of the connected device.
    #[allow(dead_code)]
    async fn get_xml(&self) -> Result<String, String>;

    // ── IP Configuration (GigE only) ────────────────────────────────────────

    /// Read the network configuration of the connected camera.
    async fn get_network_config(&self) -> Result<NetworkConfig, String> {
        Err("Network configuration not supported in this mode".to_string())
    }

    /// Set persistent IP configuration on the connected camera.
    async fn set_persistent_ip(
        &self,
        _ip: std::net::Ipv4Addr,
        _subnet: std::net::Ipv4Addr,
        _gateway: std::net::Ipv4Addr,
    ) -> Result<(), String> {
        Err("Persistent IP not supported in this mode".to_string())
    }
}

/// Type alias for the managed backend state in Tauri.
pub type BackendState = std::sync::Arc<dyn DeviceBackend>;

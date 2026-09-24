use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use viva_zenoh_api::{AcquisitionStatus, DeviceAnnounce, ImageMeta};

/// Payload for the `disconnect-reason` Tauri event emitted on unexpected loss of device.
#[derive(Debug, Clone, Serialize)]
pub struct DisconnectReason {
    pub message: String,
    pub device_id: String,
}

/// Payload for the `api-version-mismatch` Tauri event.
///
/// Emitted when the announced `api_version` of a discovered device does not
/// match `viva_zenoh_api::API_VERSION`, or is absent (old service).
#[derive(Debug, Clone, Serialize)]
pub struct ApiVersionMismatch {
    pub device_id: String,
    /// Version reported by the service, or `null` if absent.
    pub device_version: Option<u32>,
    /// Version expected by the app.
    pub app_version: u32,
}

// ── Types exposed to the UI via Tauri IPC ───────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub model: String,
    pub serial: String,
    /// Transport type: "gige", "usb3", or "zenoh" (remote service).
    #[serde(default = "default_transport")]
    pub transport: String,
    /// Current IPv4 address, when the transport has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    /// MAC address as `AA:BB:CC:DD:EE:FF`, when the transport has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mac: Option<String>,
    /// User-defined device name, when one is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
}

impl DeviceInfo {
    /// The UI's view of an announce. Both backends go through here — the
    /// embedded one builds its announce with `DeviceAnnounce::for_gige`, the
    /// same rule the service uses — so a camera shows one identity in either
    /// mode (ST-25).
    pub fn from_announce(announce: DeviceAnnounce, transport: &str) -> Self {
        Self {
            id: announce.id,
            name: announce.name,
            model: announce.model,
            serial: announce.serial,
            transport: transport.to_string(),
            ip: announce.ip,
            mac: announce.mac,
            user_name: announce.user_name,
            manufacturer: announce.manufacturer,
        }
    }
}

fn default_transport() -> String {
    "gige".to_string()
}

/// Connection state, serialized with a `kind` tag so TypeScript can switch on it.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConnectionState {
    Disconnected,
    Connecting {
        device_id: String,
    },
    Connected {
        device_id: String,
        device_name: String,
        model: String,
    },
    Reconnecting {
        device_id: String,
        attempt: u32,
        max_attempts: u32,
        reason: String,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeValueEntry {
    pub value: serde_json::Value,
    pub access_mode: String,
    /// Runtime minimum constraint forwarded from the camera service, if available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    /// Runtime maximum constraint forwarded from the camera service, if available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// Runtime increment (step) forwarded from the camera service, if available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inc: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StreamerInfo {
    pub ws_url: String,
    pub width: u32,
    pub height: u32,
}

// ── Internal registry ────────────────────────────────────────────────────────

pub struct DiscoveredDevice {
    pub info: DeviceInfo,
    pub last_seen: Instant,
}

#[derive(Default)]
pub struct DeviceRegistry {
    pub devices: HashMap<String, DiscoveredDevice>,
}

impl DeviceRegistry {
    pub fn update(&mut self, info: DeviceInfo) {
        self.devices.insert(
            info.id.clone(),
            DiscoveredDevice {
                info,
                last_seen: Instant::now(),
            },
        );
    }

    /// Remove devices not seen for `timeout_secs` and return their IDs.
    pub fn expire_old(&mut self, timeout_secs: u64) -> Vec<String> {
        let now = Instant::now();
        let expired: Vec<String> = self
            .devices
            .iter()
            .filter(|(_, d)| now.duration_since(d.last_seen).as_secs() > timeout_secs)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &expired {
            self.devices.remove(id);
        }
        expired
    }

    pub fn list(&self) -> Vec<DeviceInfo> {
        self.devices.values().map(|d| d.info.clone()).collect()
    }

    pub fn contains(&self, id: &str) -> bool {
        self.devices.contains_key(id)
    }

    pub fn get_name(&self, id: &str) -> Option<String> {
        self.devices.get(id).map(|d| d.info.name.clone())
    }

    pub fn get_model(&self, id: &str) -> Option<String> {
        self.devices.get(id).map(|d| d.info.model.clone())
    }
}

// ── Acquisition inner state ──────────────────────────────────────────────────

pub struct AcquisitionInner {
    /// Channel sender used to signal embedded streamer tasks to shut down.
    pub shutdown_tx: Option<tokio::sync::watch::Sender<bool>>,
    /// Handles to the embedded streamer tokio tasks (Zenoh source + WS server).
    pub task_handles: Vec<tauri::async_runtime::JoinHandle<()>>,
    pub ws_url: Option<String>,
    pub status: AcquisitionStatus,
    pub width: u32,
    pub height: u32,
    pub image_meta: Option<ImageMeta>,
    /// Active recording handle, if recording is in progress.
    pub recording: Option<viva_streamer::recording::RecordingHandle>,
}

impl AcquisitionInner {
    pub fn new() -> Self {
        Self {
            shutdown_tx: None,
            task_handles: Vec::new(),
            ws_url: None,
            status: AcquisitionStatus {
                active: false,
                fps: None,
                dropped: 0,
            },
            width: 640,
            height: 480,
            image_meta: None,
            recording: None,
        }
    }
}

// ── Shared Zenoh application state ──────────────────────────────────────────

/// All Zenoh-related state for the Tauri backend.
///
/// Wrapped in `Arc` so it can be cloned into async background tasks while
/// Tauri holds the canonical reference via `.manage()`.
pub struct ZenohState {
    pub session: tokio::sync::Mutex<Option<Arc<zenoh::Session>>>,
    pub registry: tokio::sync::Mutex<DeviceRegistry>,
    pub connection: tokio::sync::Mutex<ConnectionState>,
    pub node_cache: tokio::sync::RwLock<HashMap<String, NodeValueEntry>>,
    pub acquisition: tokio::sync::Mutex<AcquisitionInner>,
    /// Handles for per-connection subscriber tasks; aborted on disconnect.
    pub sub_tasks: tokio::sync::Mutex<Vec<tauri::async_runtime::JoinHandle<()>>>,
}

impl ZenohState {
    pub fn new() -> Self {
        Self {
            session: tokio::sync::Mutex::new(None),
            registry: tokio::sync::Mutex::new(DeviceRegistry::default()),
            connection: tokio::sync::Mutex::new(ConnectionState::Disconnected),
            node_cache: tokio::sync::RwLock::new(HashMap::new()),
            acquisition: tokio::sync::Mutex::new(AcquisitionInner::new()),
            sub_tasks: tokio::sync::Mutex::new(Vec::new()),
        }
    }

    pub async fn get_session(&self) -> Result<Arc<zenoh::Session>, String> {
        self.session
            .lock()
            .await
            .clone()
            .ok_or_else(|| "Zenoh session not initialized".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_announce_carries_the_identity_fields_to_the_ui() {
        let announce = DeviceAnnounce::for_gige(
            &[0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE],
            std::net::Ipv4Addr::new(192, 168, 1, 10),
            Some("FakeGigE"),
            Some("FAKE-001"),
            Some("Left"),
            Some("viva-genicam"),
        );
        let info = DeviceInfo::from_announce(announce, "gige");
        let json = serde_json::to_value(&info).expect("serialize");
        assert_eq!(json["id"], "cam-deadbeefcafe");
        assert_eq!(json["serial"], "FAKE-001");
        assert_eq!(json["ip"], "192.168.1.10");
        assert_eq!(json["mac"], "DE:AD:BE:EF:CA:FE");
        assert_eq!(json["user_name"], "Left");
        assert_eq!(json["manufacturer"], "viva-genicam");
        assert_eq!(json["transport"], "gige");
    }

    /// A version 2 service's announce has none of the identity fields; the UI
    /// type marks them optional, so they must be absent rather than `null`.
    #[test]
    fn from_announce_omits_what_an_old_service_did_not_send() {
        let v2 = r#"{"id":"cam-deadbeefcafe","name":"M","model":"M","serial":"cam-deadbeefcafe","api_version":2}"#;
        let announce: DeviceAnnounce = serde_json::from_str(v2).expect("deserialize");
        let json =
            serde_json::to_value(DeviceInfo::from_announce(announce, "zenoh")).expect("serialize");
        for field in ["ip", "mac", "user_name", "manufacturer"] {
            assert!(json.get(field).is_none(), "{field} present in {json}");
        }
    }
}

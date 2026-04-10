//! Device connection status publisher.

use std::sync::Arc;

use tracing::{info, warn};
use viva_zenoh_api::{keys, DeviceStatus};
use zenoh::Session;

/// Publish a connected status for the device.
pub async fn publish_connected(session: &Arc<Session>, device_id: &str) {
    let status = DeviceStatus {
        connected: true,
        error: None,
    };
    let key = keys::status(device_id);
    match serde_json::to_vec(&status) {
        Ok(payload) => {
            if let Err(e) = session.put(&key, payload).await {
                warn!(device_id, error = %e, "failed to publish connected status");
            } else {
                info!(device_id, "published connected status");
            }
        }
        Err(e) => warn!(device_id, error = %e, "failed to serialize status"),
    }
}

/// Publish a disconnected status for the device.
#[allow(dead_code)]
pub async fn publish_disconnected(session: &Arc<Session>, device_id: &str, reason: &str) {
    let status = DeviceStatus {
        connected: false,
        error: Some(reason.to_string()),
    };
    let key = keys::status(device_id);
    match serde_json::to_vec(&status) {
        Ok(payload) => {
            if let Err(e) = session.put(&key, payload).await {
                warn!(device_id, error = %e, "failed to publish disconnected status");
            } else {
                info!(device_id, reason, "published disconnected status");
            }
        }
        Err(e) => warn!(device_id, error = %e, "failed to serialize status"),
    }
}

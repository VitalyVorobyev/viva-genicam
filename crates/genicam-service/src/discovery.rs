//! Periodic camera discovery and DeviceAnnounce publishing.

use std::time::Duration;

use genicam::gige;
use tokio::sync::watch;
use tracing::{debug, error, info, warn};

use crate::device::DeviceHandle;

/// Run the discovery loop: periodically discover cameras and publish announcements.
///
/// When a new camera is found, connects to it and calls `on_new_device`.
pub async fn run<F, Fut>(
    discovery_timeout: Duration,
    discovery_interval: Duration,
    iface: Option<String>,
    on_new_device: F,
    mut shutdown: watch::Receiver<bool>,
) where
    F: Fn(DeviceHandle) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = ()> + Send,
{
    let known_devices: std::collections::HashMap<String, DeviceHandle> =
        std::collections::HashMap::new();

    loop {
        // Discover cameras.
        let devices = match &iface {
            Some(name) => gige::discover_on_interface(discovery_timeout, name).await,
            None => gige::discover(discovery_timeout).await,
        };

        match devices {
            Ok(found) => {
                debug!("discovery found {} cameras", found.len());
                for dev_info in found {
                    let device_id = derive_device_id(&dev_info);
                    if known_devices.contains_key(&device_id) {
                        continue;
                    }
                    info!(device_id, ip = %dev_info.ip, "new camera discovered, connecting...");
                    match DeviceHandle::connect(&dev_info).await {
                        Ok(handle) => {
                            info!(device_id, "camera connected successfully");
                            on_new_device(handle).await;
                            // Store a reference for deduplication.
                            // Note: the actual handle is moved to on_new_device.
                            // We re-connect here just for the announce loop.
                            // TODO: share handle properly.
                        }
                        Err(e) => {
                            warn!(device_id, error = %e, "failed to connect to camera");
                        }
                    }
                }
            }
            Err(e) => {
                error!(error = %e, "camera discovery failed");
            }
        }

        // Wait for next discovery cycle or shutdown.
        tokio::select! {
            _ = tokio::time::sleep(discovery_interval) => {}
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    info!("discovery loop shutting down");
                    return;
                }
            }
        }
    }
}

fn derive_device_id(info: &gige::DeviceInfo) -> String {
    let mac = info
        .mac
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join("");
    format!("cam-{mac}")
}

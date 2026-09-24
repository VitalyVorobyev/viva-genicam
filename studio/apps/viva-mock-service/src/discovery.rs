use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tracing::{debug, error};

use viva_zenoh_api::DeviceAnnounce;

use crate::config::MockConfig;

pub async fn run(
    session: Arc<zenoh::Session>,
    config: MockConfig,
    mut shutdown: watch::Receiver<bool>,
) {
    let key = viva_zenoh_api::keys::announce(&config.device_id);
    let announce = DeviceAnnounce::new(
        config.device_id.clone(),
        config.device_name.clone(),
        config.model.clone(),
        config.serial.clone(),
    );
    let payload = match serde_json::to_vec(&announce) {
        Ok(p) => p,
        Err(e) => {
            error!("Failed to serialize DeviceAnnounce: {e}");
            return;
        }
    };

    let publisher = match session.declare_publisher(&key).await {
        Ok(p) => p,
        Err(e) => {
            error!("Failed to declare discovery publisher: {e}");
            return;
        }
    };

    let mut interval = tokio::time::interval(Duration::from_secs(2));

    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                if *shutdown.borrow() { break; }
            }
            _ = interval.tick() => {
                debug!("Publishing discovery announce for {}", config.device_id);
                let _ = publisher.put(payload.clone()).await;
            }
        }
    }
}

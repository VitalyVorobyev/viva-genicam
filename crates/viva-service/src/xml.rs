//! XML queryable: responds with the device's GenICam XML.

use std::sync::Arc;

use tokio::sync::watch;
use tracing::{debug, info, warn};
use viva_zenoh_api::{keys, DeviceXmlResponse};
use zenoh::Session;

/// Declare a queryable for the device XML endpoint.
pub async fn run(
    session: Arc<Session>,
    device_id: String,
    xml: String,
    mut shutdown: watch::Receiver<bool>,
) {
    let key = keys::xml(&device_id);
    let queryable = match session.declare_queryable(&key).await {
        Ok(q) => q,
        Err(e) => {
            warn!(device_id, error = %e, "failed to declare XML queryable");
            return;
        }
    };
    info!(device_id, key, "XML queryable ready");

    let response = DeviceXmlResponse { xml };

    loop {
        tokio::select! {
            query = queryable.recv_async() => {
                match query {
                    Ok(query) => {
                        debug!(device_id, "XML query received");
                        let Ok(payload) = serde_json::to_vec(&response) else {
                            tracing::error!("failed to serialize XML response");
                            continue;
                        };
                        if let Err(e) = query.reply(&key, payload).await {
                            warn!(device_id, error = %e, "failed to reply to XML query");
                        }
                    }
                    Err(e) => {
                        warn!(device_id, error = %e, "XML queryable channel closed");
                        break;
                    }
                }
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    break;
                }
            }
        }
    }
}

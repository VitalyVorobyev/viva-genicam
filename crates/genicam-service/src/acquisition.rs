//! Acquisition control queryable and frame streaming.

use std::sync::Arc;

use genicam_zenoh_api::{
    keys, AcquisitionCommand, AcquisitionControlRequest, AcquisitionStatus, NodeOpResponse,
};
use tokio::sync::watch;
use tracing::{info, warn};
use zenoh::Session;

use crate::device::DeviceHandle;

/// Run the acquisition control queryable.
pub async fn run(
    session: Arc<Session>,
    device: Arc<DeviceHandle>,
    mut shutdown: watch::Receiver<bool>,
) {
    let device_id = device.device_id().to_string();
    let key = keys::acquisition_control(&device_id);
    let queryable = match session.declare_queryable(&key).await {
        Ok(q) => q,
        Err(e) => {
            warn!(device_id, error = %e, "failed to declare acquisition queryable");
            return;
        }
    };
    info!(device_id, key, "acquisition control queryable ready");

    let mut active = false;

    loop {
        tokio::select! {
            query = queryable.recv_async() => {
                match query {
                    Ok(query) => {
                        let response = match query.payload() {
                            Some(payload) => {
                                match serde_json::from_slice::<AcquisitionControlRequest>(
                                    &payload.to_bytes(),
                                ) {
                                    Ok(req) => match req.command {
                                        AcquisitionCommand::Start => {
                                            match device.exec_command("AcquisitionStart").await {
                                                Ok(()) => {
                                                    active = true;
                                                    publish_status(&session, &device_id, true).await;
                                                    info!(device_id, "acquisition started");
                                                    NodeOpResponse { ok: true, error: None }
                                                }
                                                Err(e) => NodeOpResponse {
                                                    ok: false,
                                                    error: Some(e.to_string()),
                                                },
                                            }
                                        }
                                        AcquisitionCommand::Stop => {
                                            match device.exec_command("AcquisitionStop").await {
                                                Ok(()) => {
                                                    active = false;
                                                    publish_status(&session, &device_id, false).await;
                                                    info!(device_id, "acquisition stopped");
                                                    NodeOpResponse { ok: true, error: None }
                                                }
                                                Err(e) => NodeOpResponse {
                                                    ok: false,
                                                    error: Some(e.to_string()),
                                                },
                                            }
                                        }
                                    },
                                    Err(e) => NodeOpResponse {
                                        ok: false,
                                        error: Some(format!("invalid payload: {e}")),
                                    },
                                }
                            }
                            None => NodeOpResponse {
                                ok: false,
                                error: Some("missing payload".to_string()),
                            },
                        };

                        let payload = serde_json::to_vec(&response).unwrap();
                        let _ = query.reply(&key, payload).await;
                    }
                    Err(_) => break,
                }
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    if active {
                        let _ = device.exec_command("AcquisitionStop").await;
                    }
                    break;
                }
            }
        }
    }
}

async fn publish_status(session: &Session, device_id: &str, active: bool) {
    let status = AcquisitionStatus {
        active,
        fps: None,
        dropped: 0,
    };
    let key = keys::acquisition_status(device_id);
    if let Ok(payload) = serde_json::to_vec(&status) {
        let _ = session.put(&key, payload).await;
    }
}

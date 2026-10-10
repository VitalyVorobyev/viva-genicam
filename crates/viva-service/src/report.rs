//! Report queryable: the diagnostic report on the service's device.
//!
//! The camera may be on a host the client cannot reach, so in remote mode the
//! report has to be written where the service runs; Viva Studio asks for it
//! here (backlog ST-41).

use std::sync::Arc;

use tokio::sync::watch;
use tracing::{info, warn};
use viva_genicam::report::{ReportRequest, ReportTarget, build_report};
use viva_zenoh_api::{DeviceReportRequest, DeviceReportResponse, keys};
use zenoh::Session;

use crate::device::DeviceHandle;

/// Declare a queryable that answers with a report on `device`.
pub async fn run(
    session: Arc<Session>,
    device: Arc<DeviceHandle>,
    mut shutdown: watch::Receiver<bool>,
) {
    let device_id = device.device_id().to_string();
    let key = keys::report(&device_id);
    let queryable = match session.declare_queryable(&key).await {
        Ok(q) => q,
        Err(e) => {
            warn!(device_id, error = %e, "failed to declare report queryable");
            return;
        }
    };
    info!(device_id, key, "report queryable ready");

    loop {
        tokio::select! {
            query = queryable.recv_async() => {
                let Ok(query) = query else { break };
                let include_xml = query
                    .payload()
                    .and_then(|p| serde_json::from_slice::<DeviceReportRequest>(&p.to_bytes()).ok())
                    .is_none_or(|req| req.include_xml);
                let request = ReportRequest {
                    notes: vec![format!("device id:   {device_id}")],
                    include_xml,
                    ..ReportRequest::new(
                        format!("viva-service {}", env!("CARGO_PKG_VERSION")),
                        ReportTarget::Ip(device.info().ip),
                    )
                };
                let response = DeviceReportResponse {
                    report: build_report(&request).await,
                };
                let Ok(payload) = serde_json::to_vec(&response) else {
                    tracing::error!("failed to serialize the report");
                    continue;
                };
                if let Err(e) = query.reply(&key, payload).await {
                    warn!(device_id, error = %e, "failed to reply to the report query");
                }
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() { break; }
            }
        }
    }
}

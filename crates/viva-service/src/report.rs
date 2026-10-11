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

/// Declare a queryable that answers with a report on the GigE camera
/// `device`, which the report opens read-only by its address.
pub async fn run(
    session: Arc<Session>,
    device: Arc<DeviceHandle>,
    shutdown: watch::Receiver<bool>,
) {
    serve(
        session,
        device.device_id().to_string(),
        format!("viva-service {}", env!("CARGO_PKG_VERSION")),
        ReportTarget::Ip(device.info().ip),
        shutdown,
    )
    .await;
}

/// Declare a queryable that answers for `device_id` with a report by `tool`
/// describing `target`. A service whose camera the report cannot open itself
/// — a USB3 Vision device admits one owner — passes
/// [`ReportTarget::Open`] (backlog DX-06).
pub async fn serve(
    session: Arc<Session>,
    device_id: String,
    tool: String,
    target: ReportTarget,
    mut shutdown: watch::Receiver<bool>,
) {
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
                    ..ReportRequest::new(tool.clone(), target.clone())
                };
                let response = DeviceReportResponse::new(build_report(&request).await);
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use viva_genicam::report::OpenCamera;

    use super::*;

    /// A service holding a camera the report cannot open answers with what
    /// it read: `viva-service-u3v` serves its USB3 Vision camera this way
    /// (backlog DX-06).
    #[tokio::test(flavor = "multi_thread")]
    async fn a_report_on_a_camera_the_service_holds_open() {
        let session = Arc::new(zenoh::open(zenoh::Config::default()).await.unwrap());
        let device_id = format!("report-test-{}", std::process::id());
        let target = ReportTarget::Open(OpenCamera {
            transport: "USB3 Vision".to_string(),
            identity: vec![("model".to_string(), "FakeU3V".to_string())],
            xml: "<RegisterDescription/>".to_string(),
        });
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let task = tokio::spawn(serve(
            session.clone(),
            device_id.clone(),
            "viva-service-u3v test".to_string(),
            target,
            shutdown_rx,
        ));

        let ask = async {
            loop {
                let replies = session
                    .get(keys::report(&device_id))
                    .payload(serde_json::to_vec(&DeviceReportRequest::new(false)).unwrap())
                    .timeout(Duration::from_secs(5))
                    .await
                    .unwrap();
                if let Ok(reply) = replies.recv_async().await
                    && let Ok(sample) = reply.result()
                {
                    return serde_json::from_slice::<DeviceReportResponse>(
                        &sample.payload().to_bytes(),
                    )
                    .unwrap()
                    .report;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        };
        let report = tokio::time::timeout(Duration::from_secs(10), ask)
            .await
            .expect("no report within 10 s");

        assert!(
            report.contains("tool:        viva-service-u3v test"),
            "{report}"
        );
        assert!(report.contains("transport: USB3 Vision"), "{report}");
        assert!(report.contains("model: FakeU3V"), "{report}");
        let _ = shutdown_tx.send(true);
        let _ = task.await;
    }
}

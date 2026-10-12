//! The service's per-camera task set, shared by both binaries and by tests
//! that run the service in-process.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use viva_zenoh_api::DeviceAnnounce;

use crate::source::StreamSource;
use crate::{acquisition, nodes, report, status, xml};

/// Start everything a service runs for one connected camera: publish its
/// status and initial feature states, then answer its XML, feature, command,
/// report and acquisition queries until `shutdown` turns true.
///
/// [`run_discovery`](crate::run_discovery) calls this for each camera it
/// connects, GigE Vision and USB3 Vision alike, and the integration tests —
/// this crate's and Viva Studio's remote-mode tests — call it to run the
/// service in-process, so they exercise the task set the binaries run rather
/// than a copy of it.
pub async fn serve_device<D: StreamSource>(
    session: Arc<zenoh::Session>,
    device: Arc<D>,
    shutdown: watch::Receiver<bool>,
) -> Vec<tokio::task::JoinHandle<()>> {
    let device_id = device.device_id().to_string();

    status::publish_connected(&session, &device_id).await;
    nodes::publish_initial_values(&session, device.as_ref()).await;

    let tool = format!("{} {}", device.service_name(), env!("CARGO_PKG_VERSION"));
    let target = device.report_target().await;

    vec![
        tokio::spawn(xml::run(
            session.clone(),
            device_id.clone(),
            device.raw_xml().to_string(),
            shutdown.clone(),
        )),
        tokio::spawn(nodes::run_set_queryable(
            session.clone(),
            device.clone(),
            shutdown.clone(),
        )),
        tokio::spawn(nodes::run_execute_queryable(
            session.clone(),
            device.clone(),
            shutdown.clone(),
        )),
        tokio::spawn(nodes::run_bulk_read_queryable(
            session.clone(),
            device.clone(),
            shutdown.clone(),
        )),
        tokio::spawn(nodes::run_introspect_queryable(
            session.clone(),
            device.clone(),
            shutdown.clone(),
        )),
        tokio::spawn(nodes::run_bulk_state_queryable(
            session.clone(),
            device.clone(),
            shutdown.clone(),
        )),
        tokio::spawn(report::serve(
            session.clone(),
            device_id,
            tool,
            target,
            shutdown.clone(),
        )),
        tokio::spawn(acquisition::run(session.clone(), device, shutdown)),
    ]
}

/// The announces a service keeps repeating, by device id.
pub(crate) type Announces = Arc<tokio::sync::Mutex<HashMap<String, DeviceAnnounce>>>;

/// Re-announce every camera in `announces` each `interval` until `shutdown`
/// turns true.
///
/// On a cadence of its own, not the discovery loop's: announcing from inside
/// discovery repeated each camera every discovery timeout plus discovery
/// interval, about 7 s, while Viva Studio drops a camera unannounced for 6 s,
/// so cameras flickered in and out of its list (backlog SVC-01). The main
/// loop adds a camera once it is served and removes it once it is lost.
pub(crate) async fn announce_loop(
    session: Arc<zenoh::Session>,
    announces: Announces,
    interval: Duration,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        let current: Vec<DeviceAnnounce> = announces.lock().await.values().cloned().collect();
        for announce in &current {
            put_announce(&session, announce).await;
        }
        tokio::select! {
            _ = tokio::time::sleep(interval) => {}
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    return;
                }
            }
        }
    }
}

/// The served cameras a discovery pass has lost: those it did not find, if it
/// succeeded. `None` is a failed pass, which says nothing about which cameras
/// are gone; a successful pass that found nothing has lost them all. The two
/// were one case once, so an unplugged only camera stayed served and
/// announced (Codex review of #185).
pub(crate) fn lost_cameras<'a>(
    discovered: Option<&std::collections::HashSet<String>>,
    served: impl Iterator<Item = &'a String>,
) -> Vec<String> {
    let Some(discovered) = discovered else {
        return Vec::new();
    };
    served
        .filter(|id| !discovered.contains(id.as_str()))
        .cloned()
        .collect()
}

/// Publish one announce on `keys::announce`.
pub(crate) async fn put_announce(session: &zenoh::Session, announce: &DeviceAnnounce) {
    let key = viva_zenoh_api::keys::announce(&announce.id);
    if let Ok(payload) = serde_json::to_vec(announce) {
        let _ = session.put(&key, payload).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A served camera is announced every interval, whatever discovery is
    /// doing, and no longer once it is lost (backlog SVC-01).
    #[tokio::test(flavor = "multi_thread")]
    async fn a_served_camera_is_announced_every_interval_until_it_is_lost() {
        let session = Arc::new(zenoh::open(zenoh::Config::default()).await.unwrap());
        let id = format!("announce-test-{}", std::process::id());
        let subscriber = session
            .declare_subscriber(viva_zenoh_api::keys::announce(&id))
            .await
            .unwrap();
        let announces = Announces::default();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let task = tokio::spawn(announce_loop(
            session.clone(),
            announces.clone(),
            Duration::from_millis(100),
            shutdown_rx,
        ));

        let count_for = |window: Duration| {
            let subscriber = &subscriber;
            async move {
                let deadline = tokio::time::Instant::now() + window;
                let mut count = 0;
                while let Ok(Ok(_)) =
                    tokio::time::timeout_at(deadline, subscriber.recv_async()).await
                {
                    count += 1;
                }
                count
            }
        };

        announces.lock().await.insert(
            id.clone(),
            DeviceAnnounce::new(id.clone(), "cam", "Model", "SN"),
        );
        let served = count_for(Duration::from_millis(650)).await;
        assert!(
            served >= 4,
            "{served} announces in 650 ms at a 100 ms interval"
        );

        announces.lock().await.remove(&id);
        // Drain one that may have been on its way.
        let _ = count_for(Duration::from_millis(150)).await;
        assert_eq!(
            count_for(Duration::from_millis(400)).await,
            0,
            "a lost camera is still announced"
        );

        let _ = shutdown_tx.send(true);
        let _ = task.await;
    }

    /// A successful discovery pass that finds nothing has lost every camera;
    /// a failed one has lost none (Codex review of #185: an unplugged only
    /// camera was announced forever).
    #[test]
    fn a_successful_empty_discovery_loses_every_camera() {
        let active = ["a".to_string(), "b".to_string()];
        let none = std::collections::HashSet::new();
        assert_eq!(lost_cameras(Some(&none), active.iter()), vec!["a", "b"]);
        assert!(lost_cameras(None, active.iter()).is_empty());
        let some: std::collections::HashSet<String> = ["b".to_string()].into();
        assert_eq!(lost_cameras(Some(&some), active.iter()), vec!["a"]);
    }

    /// Three announces fit in the expiry a client applies.
    #[test]
    fn a_client_waits_out_three_missed_announces() {
        assert_eq!(
            viva_zenoh_api::ANNOUNCE_EXPIRY,
            3 * viva_zenoh_api::ANNOUNCE_INTERVAL
        );
    }
}

//! The service's per-camera task set, shared by the binary and by tests that
//! run the service in-process.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use viva_zenoh_api::DeviceAnnounce;

use crate::device::DeviceHandle;
use crate::{acquisition, nodes, report, status, xml};

/// Start everything the service runs for one connected camera: publish its
/// status and initial feature states, then answer its XML, feature, command
/// and acquisition queries until `shutdown` turns true.
///
/// `main` calls this for each camera it connects, and the integration tests —
/// this crate's and Viva Studio's remote-mode tests — call it to run the
/// service in-process, so they exercise the task set the binary runs rather
/// than a copy of it.
pub async fn serve_device(
    session: Arc<zenoh::Session>,
    device: Arc<DeviceHandle>,
    shutdown: watch::Receiver<bool>,
) -> Vec<tokio::task::JoinHandle<()>> {
    let device_id = device.device_id().to_string();

    // Publish connected status.
    status::publish_connected(&session, &device_id).await;
    nodes::publish_initial_values(&session, device.as_ref()).await;

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
        tokio::spawn(report::run(
            session.clone(),
            device.clone(),
            shutdown.clone(),
        )),
        tokio::spawn(acquisition::run(
            session.clone(),
            device.clone(),
            shutdown.clone(),
        )),
    ]
}

/// The announces a service keeps repeating, by device id.
pub type Announces = Arc<tokio::sync::Mutex<HashMap<String, DeviceAnnounce>>>;

/// Re-announce every camera in `announces` each `interval` until `shutdown`
/// turns true.
///
/// On a cadence of its own, not the discovery loop's: announcing from inside
/// discovery repeated each camera every discovery timeout plus discovery
/// interval, about 7 s, while Viva Studio drops a camera unannounced for 6 s,
/// so cameras flickered in and out of its list (backlog SVC-01). The main
/// loop adds a camera once it is served and removes it once it is lost.
pub async fn announce_loop(
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

/// Publish one announce on `keys::announce`.
pub async fn put_announce(session: &zenoh::Session, announce: &DeviceAnnounce) {
    let key = viva_zenoh_api::keys::announce(&announce.id);
    if let Ok(payload) = serde_json::to_vec(announce) {
        let _ = session.put(&key, payload).await;
    }
}

/// Announce a discovered camera on `keys::announce`.
pub async fn publish_announce(session: &zenoh::Session, info: &viva_genicam::gige::DeviceInfo) {
    put_announce(session, &device_announce(info)).await;
}

/// Build the announce for a discovered camera from its Discovery ACK.
///
/// `serial` is the serial the camera reports. It used to be the device id,
/// which left two cameras of one model with nothing to tell them apart but a
/// MAC-derived string no client rendered (#137).
pub fn device_announce(info: &viva_genicam::gige::DeviceInfo) -> viva_zenoh_api::DeviceAnnounce {
    viva_zenoh_api::DeviceAnnounce::for_gige(
        &info.mac,
        info.ip,
        info.model.as_deref(),
        info.serial.as_deref(),
        info.user_name.as_deref(),
        info.manufacturer.as_deref(),
    )
}

/// Must agree with the `id` of [`device_announce`], which is the key the
/// service publishes under.
pub fn derive_device_id(info: &viva_genicam::gige::DeviceInfo) -> String {
    viva_zenoh_api::gige_device_id(&info.mac)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use viva_genicam::gige::DeviceInfo;

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

    /// Three announces fit in the expiry a client applies.
    #[test]
    fn a_client_waits_out_three_missed_announces() {
        assert_eq!(
            viva_zenoh_api::ANNOUNCE_EXPIRY,
            3 * viva_zenoh_api::ANNOUNCE_INTERVAL
        );
    }

    fn discovered(serial: Option<&str>, user_name: Option<&str>) -> DeviceInfo {
        DeviceInfo {
            mac: [0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE],
            model: Some("FakeGigE".into()),
            manufacturer: Some("viva-genicam".into()),
            serial: serial.map(Into::into),
            user_name: user_name.map(Into::into),
            ..DeviceInfo::from_ip(Ipv4Addr::new(192, 168, 1, 10))
        }
    }

    #[test]
    fn announce_carries_the_device_identity_not_the_device_id() {
        let info = discovered(Some("SN-1"), Some("Left"));
        let a = device_announce(&info);
        assert_eq!(a.id, "cam-deadbeefcafe");
        assert_eq!(a.id, derive_device_id(&info));
        assert_eq!(a.serial, "SN-1");
        assert_eq!(a.name, "Left");
        assert_eq!(a.model, "FakeGigE");
        assert_eq!(a.ip.as_deref(), Some("192.168.1.10"));
        assert_eq!(a.mac.as_deref(), Some("DE:AD:BE:EF:CA:FE"));
        assert_eq!(a.user_name.as_deref(), Some("Left"));
        assert_eq!(a.manufacturer.as_deref(), Some("viva-genicam"));
        assert_eq!(a.api_version, Some(viva_zenoh_api::API_VERSION));
    }

    #[test]
    fn announce_without_serial_or_user_name_leaves_them_empty() {
        let info = discovered(None, None);
        let a = device_announce(&info);
        assert_eq!(a.serial, "", "the device id must not stand in for a serial");
        assert_eq!(a.user_name, None);
        assert_eq!(a.name, "FakeGigE");
    }
}

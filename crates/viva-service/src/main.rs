//! GenICam camera service — bridges viva-genicam to Zenoh for Viva Studio.

use std::collections::HashMap;
use std::sync::Arc;

use clap::Parser;
use tokio::sync::watch;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;
use viva_genicam::gige::nic::IfaceSelector;
use viva_service::config::Cli;
use viva_service::device::DeviceHandle;
use viva_service::{acquisition, nodes, status, xml};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    info!("Starting genicam-service");

    let zenoh_config = load_zenoh_config(cli.zenoh_config.as_deref())?;
    let session = Arc::new(zenoh::open(zenoh_config).await?);
    info!("Zenoh session opened");

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let discovery_timeout = cli.discovery_timeout();
    let discovery_interval = cli.discovery_interval();
    let iface = cli.iface.clone();

    // Per-device task tracking.
    let active_devices: Arc<tokio::sync::Mutex<HashMap<String, Vec<tokio::task::JoinHandle<()>>>>> =
        Arc::new(tokio::sync::Mutex::new(HashMap::new()));

    let session_ref = session.clone();
    let shutdown_rx_ref = shutdown_rx.clone();
    let active_ref = active_devices.clone();

    // Discovery loop.
    let discovery_handle = tokio::spawn(async move {
        run_discovery_loop(
            session_ref,
            discovery_timeout,
            discovery_interval,
            iface,
            shutdown_rx_ref,
            active_ref,
        )
        .await;
    });

    // Wait for CTRL+C.
    tokio::signal::ctrl_c().await?;
    info!("Shutdown requested (CTRL+C)");
    let _ = shutdown_tx.send(true);

    // Wait for discovery loop to finish.
    let _ = discovery_handle.await;

    // Wait for all device tasks.
    let mut active = active_devices.lock().await;
    for (device_id, tasks) in active.drain() {
        info!(device_id, "waiting for device tasks to finish");
        for task in tasks {
            let _ = task.await;
        }
    }

    session.close().await?;
    info!("genicam-service shut down");
    Ok(())
}

async fn run_discovery_loop(
    session: Arc<zenoh::Session>,
    discovery_timeout: std::time::Duration,
    discovery_interval: std::time::Duration,
    iface: Option<IfaceSelector>,
    mut shutdown: watch::Receiver<bool>,
    active_devices: Arc<tokio::sync::Mutex<HashMap<String, Vec<tokio::task::JoinHandle<()>>>>>,
) {
    use viva_genicam::gige;

    loop {
        // Resolve `--iface` on every pass rather than once at startup, so an
        // interface that comes up after the service does is still picked up.
        // A selector that resolves to nothing is logged and retried on the
        // next tick, not fatal — and the error carries the interfaces this
        // host reports, which is what a Windows operator needs to find the
        // GUID. It must not fall through to a broadcast scan: the operator
        // named an interface, and scanning every other one instead would be
        // answering a question they did not ask.
        let resolved = match iface.as_ref().map(IfaceSelector::resolve).transpose() {
            Ok(resolved) => resolved,
            Err(e) => {
                error!(error = %e, "--iface could not be resolved, retrying");
                if wait_or_shutdown(&mut shutdown, discovery_interval).await {
                    return;
                }
                continue;
            }
        };

        // Discover cameras.
        let devices = match &resolved {
            Some(iface) => gige::discover_on_interface(discovery_timeout, iface.name()).await,
            None => gige::discover(discovery_timeout).await,
        };

        let mut discovered_ids = std::collections::HashSet::new();

        match devices {
            Ok(found) => {
                for dev_info in &found {
                    discovered_ids.insert(derive_device_id(dev_info));
                }
                for dev_info in found {
                    let device_id = derive_device_id(&dev_info);
                    let mut active = active_devices.lock().await;
                    if active.contains_key(&device_id) {
                        drop(active);
                        publish_announce(&session, &dev_info).await;
                        continue;
                    }

                    info!(device_id, ip = %dev_info.ip, "new camera, connecting...");
                    match DeviceHandle::connect(&dev_info, resolved.clone()).await {
                        Ok(handle) => {
                            let handle = Arc::new(handle);
                            info!(device_id, "connected, spawning service tasks");

                            let shutdown_rx = shutdown.clone();
                            let tasks =
                                spawn_device_tasks(session.clone(), handle, shutdown_rx).await;
                            active.insert(device_id.clone(), tasks);

                            publish_announce(&session, &dev_info).await;
                        }
                        Err(e) => {
                            error!(device_id, error = %e, "failed to connect");
                        }
                    }
                }
            }
            Err(e) => {
                error!(error = %e, "discovery failed");
            }
        }

        // Detect lost devices (discovered_ids is empty on discovery failure — skip cleanup).
        if !discovered_ids.is_empty() {
            let mut active = active_devices.lock().await;
            let lost: Vec<String> = active
                .keys()
                .filter(|id| !discovered_ids.contains(id.as_str()))
                .cloned()
                .collect();
            for device_id in lost {
                warn!(device_id, "device lost, cleaning up");
                if let Some(tasks) = active.remove(&device_id) {
                    for task in tasks {
                        task.abort();
                    }
                }
                status::publish_disconnected(&session, &device_id, "device lost").await;
            }
        }

        if wait_or_shutdown(&mut shutdown, discovery_interval).await {
            return;
        }
    }
}

/// Sleep until the next discovery pass, returning `true` if the service is
/// shutting down and the loop should stop instead.
async fn wait_or_shutdown(
    shutdown: &mut watch::Receiver<bool>,
    interval: std::time::Duration,
) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(interval) => false,
        _ = shutdown.changed() => {
            if *shutdown.borrow() {
                info!("discovery loop shutting down");
                true
            } else {
                false
            }
        }
    }
}

async fn spawn_device_tasks(
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
        tokio::spawn(acquisition::run(
            session.clone(),
            device.clone(),
            shutdown.clone(),
        )),
    ]
}

async fn publish_announce(session: &zenoh::Session, info: &viva_genicam::gige::DeviceInfo) {
    use viva_zenoh_api::keys;

    let announce = device_announce(info);
    let key = keys::announce(&announce.id);
    if let Ok(payload) = serde_json::to_vec(&announce) {
        let _ = session.put(&key, payload).await;
    }
}

/// Build the announce for a discovered camera from its Discovery ACK.
///
/// `serial` is the serial the camera reports. It used to be the device id,
/// which left two cameras of one model with nothing to tell them apart but a
/// MAC-derived string no client rendered (#137).
fn device_announce(info: &viva_genicam::gige::DeviceInfo) -> viva_zenoh_api::DeviceAnnounce {
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
fn derive_device_id(info: &viva_genicam::gige::DeviceInfo) -> String {
    viva_zenoh_api::gige_device_id(&info.mac)
}

fn init_tracing(verbose: u8) {
    let default_level = match verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

fn load_zenoh_config(
    path: Option<&str>,
) -> Result<zenoh::Config, Box<dyn std::error::Error + Send + Sync>> {
    match path {
        Some(p) => Ok(zenoh::Config::from_file(p)?),
        None => Ok(zenoh::Config::default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use viva_genicam::gige::DeviceInfo;

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

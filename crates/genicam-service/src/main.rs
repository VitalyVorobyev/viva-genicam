//! GenICam camera service — bridges genicam-rs to Zenoh for genicam-studio.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use genicam_service::config::Cli;
use genicam_service::device::DeviceHandle;
use genicam_service::{acquisition, nodes, status, xml};
use tokio::sync::watch;
use tracing::{debug, error, info, warn};
use tracing_subscriber::EnvFilter;

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
    iface: Option<String>,
    mut shutdown: watch::Receiver<bool>,
    active_devices: Arc<tokio::sync::Mutex<HashMap<String, Vec<tokio::task::JoinHandle<()>>>>>,
) {
    use genicam::gige;

    loop {
        // Discover cameras.
        let devices = match &iface {
            Some(name) => gige::discover_on_interface(discovery_timeout, name).await,
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
                        publish_announce(
                            &session,
                            &device_id,
                            dev_info.model.as_deref().unwrap_or("Unknown"),
                        )
                        .await;
                        continue;
                    }

                    info!(device_id, ip = %dev_info.ip, "new camera, connecting...");
                    match DeviceHandle::connect(&dev_info, iface.clone()).await {
                        Ok(handle) => {
                            let handle = Arc::new(handle);
                            info!(device_id, "connected, spawning service tasks");

                            let shutdown_rx = shutdown.clone();
                            let tasks =
                                spawn_device_tasks(session.clone(), handle, shutdown_rx).await;
                            active.insert(device_id.clone(), tasks);

                            publish_announce(
                                &session,
                                &device_id,
                                dev_info.model.as_deref().unwrap_or("Unknown"),
                            )
                            .await;
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

async fn spawn_device_tasks(
    session: Arc<zenoh::Session>,
    device: Arc<DeviceHandle>,
    shutdown: watch::Receiver<bool>,
) -> Vec<tokio::task::JoinHandle<()>> {
    let device_id = device.device_id().to_string();

    // Publish connected status.
    status::publish_connected(&session, &device_id).await;
    nodes::publish_initial_values(&session, &device).await;

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
        tokio::spawn(acquisition::run(
            session.clone(),
            device.clone(),
            shutdown.clone(),
        )),
        tokio::spawn(heartbeat_loop(device.clone(), shutdown.clone())),
    ]
}

async fn publish_announce(session: &zenoh::Session, device_id: &str, model: &str) {
    use genicam_zenoh_api::{API_VERSION, DeviceAnnounce, keys};

    let announce = DeviceAnnounce {
        id: device_id.to_string(),
        name: model.to_string(),
        model: model.to_string(),
        serial: device_id.to_string(),
        api_version: Some(API_VERSION),
    };
    let key = keys::announce(device_id);
    if let Ok(payload) = serde_json::to_vec(&announce) {
        let _ = session.put(&key, payload).await;
    }
}

fn derive_device_id(info: &genicam::gige::DeviceInfo) -> String {
    let mac = info
        .mac
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join("");
    format!("cam-{mac}")
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

/// Periodically read a register to keep the GVCP control channel alive.
///
/// GigE Vision cameras drop CCP (Control Channel Privilege) after a heartbeat
/// timeout (~3 s on aravis). We ping every 500 ms so that even with mutex
/// contention or scheduling delays we have 5 chances before the timer expires.
///
/// The loop respects [`DeviceHandle::is_heartbeat_paused`] so that
/// `refresh_connection` can swap the underlying camera without the old
/// socket's heartbeat holding the mutex and starving the new CCP timer.
async fn heartbeat_loop(device: Arc<DeviceHandle>, mut shutdown: watch::Receiver<bool>) {
    use tokio::time::MissedTickBehavior;

    let mut interval = tokio::time::interval(Duration::from_millis(500));
    // After a long pause (e.g. during refresh_connection) reset the interval
    // so we don't burst-fire stale ticks — one fresh ping is enough.
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

    let mut consecutive_failures: u32 = 0;

    loop {
        tokio::select! {
            _ = interval.tick() => {
                if device.is_heartbeat_paused() {
                    debug!("heartbeat skipped (paused for connection refresh)");
                    continue;
                }
                let start = tokio::time::Instant::now();
                match device.heartbeat_ping().await {
                    Ok(()) => {
                        if consecutive_failures > 0 {
                            info!(
                                consecutive_failures,
                                "heartbeat recovered"
                            );
                        }
                        consecutive_failures = 0;
                    }
                    Err(e) => {
                        consecutive_failures += 1;
                        warn!(
                            error = %e,
                            consecutive_failures,
                            "heartbeat failed"
                        );
                    }
                }
                let elapsed = start.elapsed();
                if elapsed > Duration::from_millis(400) {
                    warn!(
                        elapsed_ms = elapsed.as_millis() as u64,
                        "heartbeat ping slow (possible mutex contention)"
                    );
                }
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() { break; }
            }
        }
    }
}

fn load_zenoh_config(
    path: Option<&str>,
) -> Result<zenoh::Config, Box<dyn std::error::Error + Send + Sync>> {
    match path {
        Some(p) => Ok(zenoh::Config::from_file(p)?),
        None => Ok(zenoh::Config::default()),
    }
}

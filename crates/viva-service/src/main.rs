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
use viva_service::serve::{
    Announces, announce_loop, derive_device_id, device_announce, lost_cameras, publish_announce,
    serve_device,
};
use viva_service::status;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    info!("Starting viva-service");

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

    // Every served camera, re-announced on its own cadence (SVC-01).
    let announces = Announces::default();
    let announce_handle = tokio::spawn(announce_loop(
        session.clone(),
        announces.clone(),
        viva_zenoh_api::ANNOUNCE_INTERVAL,
        shutdown_rx.clone(),
    ));

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
            announces,
        )
        .await;
    });

    // Wait for CTRL+C.
    tokio::signal::ctrl_c().await?;
    info!("Shutdown requested (CTRL+C)");
    let _ = shutdown_tx.send(true);

    // Wait for discovery loop to finish.
    let _ = discovery_handle.await;
    let _ = announce_handle.await;

    // Wait for all device tasks.
    let mut active = active_devices.lock().await;
    for (device_id, tasks) in active.drain() {
        info!(device_id, "waiting for device tasks to finish");
        for task in tasks {
            let _ = task.await;
        }
    }

    session.close().await?;
    info!("viva-service shut down");
    Ok(())
}

async fn run_discovery_loop(
    session: Arc<zenoh::Session>,
    discovery_timeout: std::time::Duration,
    discovery_interval: std::time::Duration,
    iface: Option<IfaceSelector>,
    mut shutdown: watch::Receiver<bool>,
    active_devices: Arc<tokio::sync::Mutex<HashMap<String, Vec<tokio::task::JoinHandle<()>>>>>,
    announces: Announces,
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
        let discovery_succeeded = devices.is_ok();

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
                        // A camera's address or name can change between
                        // passes; the announce loop sends what was found last.
                        announces
                            .lock()
                            .await
                            .insert(device_id, device_announce(&dev_info));
                        continue;
                    }

                    info!(device_id, ip = %dev_info.ip, "new camera, connecting...");
                    match DeviceHandle::connect(&dev_info, resolved.clone()).await {
                        Ok(handle) => {
                            let handle = Arc::new(handle);
                            info!(device_id, "connected, spawning service tasks");

                            let shutdown_rx = shutdown.clone();
                            let tasks = serve_device(session.clone(), handle, shutdown_rx).await;
                            active.insert(device_id.clone(), tasks);

                            // Announced at once, then by the announce loop.
                            publish_announce(&session, &dev_info).await;
                            announces
                                .lock()
                                .await
                                .insert(device_id.clone(), device_announce(&dev_info));
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

        // Lost devices: those a successful pass did not find.
        {
            let mut active = active_devices.lock().await;
            let lost = lost_cameras(
                discovery_succeeded.then_some(&discovered_ids),
                active.keys(),
            );
            for device_id in lost {
                warn!(device_id, "device lost, cleaning up");
                announces.lock().await.remove(&device_id);
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

//! The service's lifecycle, for every transport: find cameras, serve the new
//! ones, re-announce the known ones, and let go of the lost ones.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};
use viva_genicam::GenicamError;
use viva_zenoh_api::DeviceAnnounce;

use crate::serve::{Announces, announce_loop, lost_cameras, put_announce, serve_device};
use crate::source::StreamSource;
use crate::status;

/// How a service finds the cameras it serves: a GigE Vision broadcast, a USB
/// enumeration, or a test's list.
#[async_trait::async_trait]
pub trait Discovery: Send + Sync + 'static {
    /// What a pass reports of one camera, enough to connect to it.
    type Found: Send + Sync + 'static;
    /// The camera, served.
    type Device: StreamSource;

    /// One pass: the cameras present now. An error is a failed pass, which
    /// says nothing about which cameras are gone.
    async fn discover(&self) -> Result<Vec<Self::Found>, GenicamError>;

    /// The announce a camera is published under; its `id` is the device id,
    /// and must not change from pass to pass while the camera is present.
    fn announce(&self, found: &Self::Found) -> DeviceAnnounce;

    /// Open a camera a pass found.
    async fn connect(&self, found: &Self::Found) -> Result<Self::Device, GenicamError>;
}

/// Run the service until `shutdown` turns true: every `interval`, discover;
/// connect and serve each new camera, publishing its status and announcing
/// it; keep announcing the cameras served; and for each one a successful
/// pass no longer finds, stop its tasks, stop announcing it and publish it
/// disconnected. Waits for every camera's tasks before returning.
///
/// Both services run this: GigE Vision with
/// [`GigeDiscovery`](crate::GigeDiscovery), USB3 Vision with its own
/// (backlog API-04, SVC-04).
pub async fn run_discovery<D: Discovery>(
    session: Arc<zenoh::Session>,
    discovery: D,
    interval: Duration,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut active: HashMap<String, Vec<JoinHandle<()>>> = HashMap::new();
    // Every served camera, re-announced on its own cadence (SVC-01).
    let announces = Announces::default();
    let announcing = tokio::spawn(announce_loop(
        session.clone(),
        announces.clone(),
        viva_zenoh_api::ANNOUNCE_INTERVAL,
        shutdown.clone(),
    ));

    loop {
        let pass = discovery.discover().await;
        let mut present = HashSet::new();
        match &pass {
            Ok(found) => {
                for found in found {
                    let announce = discovery.announce(found);
                    let device_id = announce.id.clone();
                    present.insert(device_id.clone());
                    if active.contains_key(&device_id) {
                        // A camera's address or name can change between
                        // passes; the announce loop sends what was found last.
                        announces.lock().await.insert(device_id, announce);
                        continue;
                    }
                    info!(device_id, "new camera, connecting");
                    match discovery.connect(found).await {
                        Ok(device) => {
                            let tasks =
                                serve_device(session.clone(), Arc::new(device), shutdown.clone())
                                    .await;
                            active.insert(device_id.clone(), tasks);
                            // Announced at once, then by the announce loop.
                            put_announce(&session, &announce).await;
                            announces.lock().await.insert(device_id.clone(), announce);
                            info!(device_id, "serving");
                        }
                        Err(e) => error!(device_id, error = %e, "failed to connect"),
                    }
                }
            }
            Err(e) => error!(error = %e, "discovery failed"),
        }

        // Lost cameras: those a successful pass did not find.
        for device_id in lost_cameras(pass.is_ok().then_some(&present), active.keys()) {
            warn!(device_id, "camera lost, cleaning up");
            announces.lock().await.remove(&device_id);
            if let Some(tasks) = active.remove(&device_id) {
                for task in tasks {
                    task.abort();
                }
            }
            status::publish_disconnected(&session, &device_id, "device lost").await;
        }

        tokio::select! {
            _ = tokio::time::sleep(interval) => {}
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    info!("discovery loop shutting down");
                    break;
                }
            }
        }
    }

    let _ = announcing.await;
    for (device_id, tasks) in active.drain() {
        info!(device_id, "waiting for the camera's tasks to finish");
        for task in tasks {
            let _ = task.await;
        }
    }
}

/// A service's `main`: open the Zenoh session `zenoh_config` names (the
/// default without one), [`run_discovery`] until CTRL+C, then shut down —
/// every camera's tasks first, the session last.
pub async fn run_service<D: Discovery>(
    name: &str,
    zenoh_config: Option<&str>,
    discovery: D,
    interval: Duration,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("starting {name}");
    let config = match zenoh_config {
        Some(path) => zenoh::Config::from_file(path)?,
        None => zenoh::Config::default(),
    };
    let session = Arc::new(zenoh::open(config).await?);
    info!("Zenoh session opened");

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let service = tokio::spawn(run_discovery(
        session.clone(),
        discovery,
        interval,
        shutdown_rx,
    ));

    tokio::signal::ctrl_c().await?;
    info!("shutdown requested (CTRL+C)");
    let _ = shutdown_tx.send(true);
    let _ = service.await;

    session.close().await?;
    info!("{name} shut down");
    Ok(())
}

/// Log to stderr at `info`, `debug` or `trace` for 0, 1 or more `-v`, unless
/// `RUST_LOG` says otherwise.
pub fn init_tracing(verbose: u8) {
    let default_level = match verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default_level));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

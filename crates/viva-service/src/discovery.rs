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

use crate::device::DeviceOps;
use crate::serve::{Announces, announce_loop, lost_cameras, put_announce, serve_device, stopped};
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

    /// Whether the served session still reaches the camera this pass found.
    ///
    /// A stable device id need not imply a stable endpoint: GigE cameras keep
    /// their MAC when their IP or the chosen host interface changes. Returning
    /// `false` retires the whole served session before opening the new one.
    /// Transports whose endpoint cannot change may keep the default.
    fn connection_matches(&self, _device: &Self::Device, _found: &Self::Found) -> bool {
        true
    }

    /// Open a camera a pass found.
    async fn connect(&self, found: &Self::Found) -> Result<Self::Device, GenicamError>;
}

/// How long a lost camera's tasks get to let go of it — the frame loop
/// stopped, the stream closed, acquisition stopped — before they are aborted.
const RETIRE_TIMEOUT: Duration = Duration::from_secs(10);

/// A camera being served: the device, the sender that tells its tasks to
/// stop, and the tasks.
struct Served<D> {
    device: Arc<D>,
    stop: watch::Sender<bool>,
    tasks: Vec<JoinHandle<()>>,
}

impl<D> Served<D> {
    /// Tell the camera's tasks to stop, and wait until they have: when this
    /// returns nothing publishes under the camera's id any more. A task that
    /// outlasts [`RETIRE_TIMEOUT`] is aborted; aborting is never the first
    /// resort, since an aborted acquisition task cannot stop its frame loop
    /// (Codex review of #203).
    async fn retire(self, device_id: &str) {
        let _ = self.stop.send(true);
        for task in self.tasks {
            let abort = task.abort_handle();
            if tokio::time::timeout(RETIRE_TIMEOUT, task).await.is_err() {
                warn!(device_id, "a camera task did not stop in time; aborting it");
                abort.abort();
            }
        }
    }
}

/// Run the service until `shutdown` turns true: every `interval`, discover;
/// connect and serve each new camera, publishing its status and announcing
/// it; keep announcing the cameras served; and for each one a successful
/// pass no longer finds, or whose transport says it is gone
/// ([`DeviceOps::is_lost`](crate::DeviceOps::is_lost)), stop its tasks and
/// wait for them, stop announcing it and publish it disconnected — in that
/// order, so nothing is published under its id afterwards, and a camera
/// found again is served afresh. Waits for every camera's tasks before
/// returning.
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
    let mut active: HashMap<String, Served<D::Device>> = HashMap::new();
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
        if let Err(e) = &pass {
            error!(error = %e, "discovery failed");
        }
        let found: Vec<(DeviceAnnounce, &D::Found)> = pass
            .as_ref()
            .map(|found| found.iter().map(|f| (discovery.announce(f), f)).collect())
            .unwrap_or_default();
        let present: HashSet<String> = found.iter().map(|(a, _)| a.id.clone()).collect();

        // Lost cameras: those a successful pass did not find, and those the
        // transport reports gone — a camera unplugged and plugged back in
        // within one interval keeps its id, but not its connection. The same
        // applies when its endpoint or selected host interface changed.
        let mut lost = lost_cameras(pass.is_ok().then_some(&present), active.keys());
        let gone: Vec<String> = active
            .iter()
            .filter(|(id, served)| {
                !lost.contains(id)
                    && (served.device.is_lost()
                        || found.iter().any(|(announce, found)| {
                            announce.id == **id
                                && !discovery.connection_matches(&served.device, found)
                        }))
            })
            .map(|(id, _)| id.clone())
            .collect();
        lost.extend(gone);
        for device_id in lost {
            warn!(device_id, "camera lost, cleaning up");
            announces.lock().await.remove(&device_id);
            if let Some(served) = active.remove(&device_id) {
                served.retire(&device_id).await;
            }
            status::publish_disconnected(&session, &device_id, "device lost").await;
        }

        for (announce, found) in found {
            let device_id = announce.id.clone();
            if active.contains_key(&device_id) {
                // A camera's address or name can change between passes; the
                // announce loop sends what was found last.
                announces.lock().await.insert(device_id, announce);
                continue;
            }
            info!(device_id, "new camera, connecting");
            match discovery.connect(found).await {
                Ok(device) => {
                    let device = Arc::new(device);
                    let (stop, stopped) = watch::channel(false);
                    let tasks = serve_device(session.clone(), device.clone(), stopped).await;
                    active.insert(
                        device_id.clone(),
                        Served {
                            device,
                            stop,
                            tasks,
                        },
                    );
                    // Announced at once, then by the announce loop.
                    put_announce(&session, &announce).await;
                    announces.lock().await.insert(device_id.clone(), announce);
                    info!(device_id, "serving");
                }
                Err(e) => error!(device_id, error = %e, "failed to connect"),
            }
        }

        tokio::select! {
            _ = tokio::time::sleep(interval) => {}
            _ = stopped(&mut shutdown) => {
                info!("discovery loop shutting down");
                break;
            }
        }
    }

    let _ = announcing.await;
    for (device_id, served) in active.drain() {
        info!(device_id, "waiting for the camera's tasks to finish");
        served.retire(&device_id).await;
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
    // nusb logs its own event-loop thread at info.
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new(format!("{default_level},nusb=warn"))
    });
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

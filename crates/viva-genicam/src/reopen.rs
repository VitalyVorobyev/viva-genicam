//! Opening a GigE Vision camera again after it went away (backlog API-21).
//!
//! A camera that loses its link, or its power, comes back as a new session:
//! the old one's control privilege may have expired with its heartbeat, its
//! stream channel may have closed, and volatile settings may have reset.
//! [`reopen_gige`] finds it again by its MAC address, waits for it to answer
//! and to be free, connects afresh, and puts a saved configuration back.

use std::time::{Duration, Instant};

use tracing::{debug, info, warn};

use crate::genapi::{Configuration, LoadReport};
use crate::gige::{self, DeviceInfo, DiscoveryOptions};
use crate::{Access, Camera, GenicamError, GigeRegisterIo, connect_gige_as};

/// How [`reopen_gige`] looks for a camera that went away, and what it puts
/// back.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Reopen {
    /// How the camera is opened again; [`Access::Control`] by default.
    pub access: Access,
    /// How long to keep trying before giving up; 30 s by default, enough for
    /// a camera's heartbeat to expire and for one to boot.
    pub patience: Duration,
    /// How long each discovery waits for an answer; 500 ms by default.
    pub discovery_timeout: Duration,
    /// The host interface to look on when the camera no longer answers at
    /// its old address, by OS name; every interface when `None`.
    pub interface: Option<String>,
    /// A configuration to restore once the camera is open, such as one
    /// [`Camera::save_features`] saved before it went away.
    pub configuration: Option<Configuration>,
}

impl Default for Reopen {
    fn default() -> Self {
        Self {
            access: Access::Control,
            patience: Duration::from_secs(30),
            discovery_timeout: Duration::from_millis(500),
            interface: None,
            configuration: None,
        }
    }
}

impl Reopen {
    /// Reopen with control, for 30 s, restoring nothing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Open the camera again with `access`.
    pub fn access(mut self, access: Access) -> Self {
        self.access = access;
        self
    }

    /// Keep trying for `patience`.
    pub fn patience(mut self, patience: Duration) -> Self {
        self.patience = patience;
        self
    }

    /// Wait `timeout` for each discovery answer.
    pub fn discovery_timeout(mut self, timeout: Duration) -> Self {
        self.discovery_timeout = timeout;
        self
    }

    /// Look on the host interface named `name` (`en0`, `eth0`).
    pub fn interface(mut self, name: impl Into<String>) -> Self {
        self.interface = Some(name.into());
        self
    }

    /// Restore `configuration` once the camera is open.
    pub fn configuration(mut self, configuration: Configuration) -> Self {
        self.configuration = Some(configuration);
        self
    }
}

/// A camera [`reopen_gige`] opened again.
#[non_exhaustive]
pub struct Reopened {
    /// The camera, in a new session.
    pub camera: Camera<GigeRegisterIo>,
    /// The GenApi XML it serves now, fetched again: a firmware update while
    /// it was away may have changed it.
    pub xml: String,
    /// Its Discovery ACK now: the address may differ from the one it had.
    pub device: DeviceInfo,
    /// What restoring [`Reopen::configuration`] did, when one was given.
    pub restored: Option<LoadReport>,
    /// How many connection attempts it took, the successful one included.
    pub attempts: u32,
}

/// Open again a GigE Vision camera that went away: unplugged, power-cycled,
/// or with its heartbeat expired.
///
/// `previous` is the camera's Discovery ACK from before. The camera is
/// looked for by its MAC address — at its old address first, then by
/// broadcast on [`Reopen::interface`] (or every interface) in case it came
/// back with another — and then opened afresh with [`connect_gige_as`],
/// which fetches its XML again. Every failure is retried, with a growing
/// pause, until [`Reopen::patience`] runs out: a camera that is still
/// booting does not answer, and one whose old session's heartbeat has not
/// yet expired refuses the claim. Once open, [`Reopen::configuration`] is
/// restored, if given; a value that could not be written is in
/// [`Reopened::restored`], not an error.
///
/// A record without a MAC address ([`DeviceInfo::from_ip`]) matches whatever
/// camera answers at its address.
///
/// The old [`Camera`] should be dropped first, or the claim waits for its
/// session, still being kept alive, to give control up.
///
/// The patience bounds discovery, connection attempts and their retry pauses.
/// Configuration restoration follows a successful open and uses the camera's
/// ordinary register timeouts; its failures are reported, never retried as a
/// new connection. Cancelling an open sends a best-effort release for any control
/// it already claimed; a lost release datagram leaves the camera to expire it.
///
/// Returns the last error when no further attempt fits, or [`GenicamError::Timeout`]
/// if an in-flight attempt consumes the remaining patience.
pub async fn reopen_gige(
    previous: &DeviceInfo,
    options: &Reopen,
) -> Result<Reopened, GenicamError> {
    let started = Instant::now();
    let mut attempts = 0u32;
    let mut pause = Duration::from_millis(250);
    loop {
        let Some(remaining) = options.patience.checked_sub(started.elapsed()) else {
            return Err(GenicamError::Timeout(options.patience));
        };
        if remaining.is_zero() {
            return Err(GenicamError::Timeout(options.patience));
        }
        attempts = attempts.saturating_add(1);
        let attempt = async {
            let device = find(previous, options)
                .await?
                .ok_or(GenicamError::Timeout(options.discovery_timeout))?;
            let (camera, xml) = connect_gige_as(&device, options.access).await?;
            Ok::<_, GenicamError>((device, camera, xml))
        };
        let error = match tokio::time::timeout(remaining, attempt).await {
            Ok(Ok((device, mut camera, xml))) => {
                let restored = options
                    .configuration
                    .as_ref()
                    .map(|configuration| camera.load_configuration(configuration));
                info!(
                    ip = %device.ip,
                    attempts,
                    waited = ?started.elapsed(),
                    "camera reopened"
                );
                return Ok(Reopened {
                    camera,
                    xml,
                    device,
                    restored,
                    attempts,
                });
            }
            Ok(Err(error)) => error,
            Err(_) => return Err(GenicamError::Timeout(options.patience)),
        };
        let waited = started.elapsed();
        if waited + pause >= options.patience {
            warn!(attempts, ?waited, %error, "the camera did not come back");
            return Err(error);
        }
        debug!(attempts, %error, ?pause, "camera not back yet; trying again");
        tokio::time::sleep(pause).await;
        pause = (pause * 2).min(Duration::from_secs(2));
    }
}

/// The camera `previous` describes, as it answers now, or `None`.
async fn find(previous: &DeviceInfo, options: &Reopen) -> Result<Option<DeviceInfo>, GenicamError> {
    let unknown_mac = previous.mac == [0; 6];
    let same = |found: &DeviceInfo| unknown_mac || found.mac == previous.mac;
    let discovery = |source| GenicamError::Gige {
        during: "look for the camera again",
        source,
    };
    match gige::discover_ip(previous.ip, options.discovery_timeout).await {
        Ok(Some(found)) if same(&found) => return Ok(Some(found)),
        Err(error) if unknown_mac => return Err(discovery(error)),
        Err(error) => debug!(%error, "old address unreachable; looking for the camera by MAC"),
        Ok(_) => {}
    }
    if unknown_mac {
        return Ok(None);
    }
    let mut broadcast = DiscoveryOptions::new().allow_broadcast_ack(true);
    if let Some(name) = &options.interface {
        broadcast = broadcast.interface(name);
    }
    let found = gige::discover_with(options.discovery_timeout, &broadcast)
        .await
        .map_err(discovery)?;
    Ok(found.into_iter().find(|found| found.mac == previous.mac))
}

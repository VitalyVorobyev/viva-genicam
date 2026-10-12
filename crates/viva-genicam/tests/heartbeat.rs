//! The control channel survives an idle period (SR-05).
//!
//! A GigE Vision device revokes control privilege if it receives no GVCP command
//! within `GevHeartbeatTimeout`, and GVSP image traffic does not count. Before
//! SR-05 the library did none of this: `connect_gige` claimed privilege and
//! nothing refreshed it, so an idle session — a Python prompt, a stream running
//! with no register access — silently lost control and the next write failed.
//! Three consumers had each grown their own copy of the same loop; the library
//! owns it now.
//!
//! These two tests are a pair. The negative control proves the fake camera
//! really does expire privilege, so the positive test cannot pass by the fake
//! being permissive.
//!
//! ```sh
//! cargo test -p viva-genicam --test heartbeat
//! ```

mod common;

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use viva_genicam::gige::consts as gvcp_consts;
use viva_genicam::{connect_gige, gige};

/// Heartbeat window the fake reports for these tests. Short enough that waiting
/// one out does not dominate the suite; long enough that the derived keepalive
/// period (a quarter of it) is comfortably above the library's 100 ms floor.
const HEARTBEAT_TIMEOUT_MS: u32 = 800;

/// How long to stay idle. Well past the window, so a loaded runner cannot make
/// the "privilege expired" case look like the "privilege held" case.
const IDLE: Duration = Duration::from_millis(2_400);

async fn start_camera() -> common::TestCamera {
    common::TestCamera::start_with(|builder| {
        builder
            .enforce_heartbeat(true)
            .heartbeat_timeout_ms(HEARTBEAT_TIMEOUT_MS)
    })
    .await
}

async fn discover_fake() -> gige::DeviceInfo {
    let devices = gige::discover_all(Duration::from_secs(2))
        .await
        .expect("discovery failed");
    devices
        .into_iter()
        .find(|d| d.ip.is_loopback())
        .expect("fake camera not found on loopback")
}

/// Negative control: a raw [`GigeDevice`](gige::GigeDevice) has no keepalive.
///
/// This is the world before SR-05, and it is what makes the next test mean
/// something: without it, a fake that never expired privilege would report
/// success either way.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_keepalive_the_device_takes_control_privilege_back() {
    let _cam = start_camera().await;
    let info = discover_fake().await;

    let addr = SocketAddr::new(IpAddr::V4(info.ip), gige::GVCP_PORT);
    let mut device = gige::GigeDevice::open(addr).await.expect("open control");
    device.claim_control().await.expect("claim control");
    assert!(
        device.ping_control_channel().await.expect("ping"),
        "privilege should be held immediately after claiming it"
    );

    tokio::time::sleep(IDLE).await;

    assert!(
        !device.ping_control_channel().await.expect("ping"),
        "the device should have revoked privilege after {IDLE:?} of silence"
    );
}

/// A `Camera` built by `connect_gige` keeps its privilege across an idle period.
///
/// The caller does nothing: holding the camera is enough, because the keepalive
/// belongs to `GigeRegisterIo` and lives exactly as long as it does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_transport_keepalive_holds_control_privilege_through_an_idle_period() {
    let _cam = start_camera().await;
    let info = discover_fake().await;

    let camera = connect_gige(&info).await.expect("connect failed");

    tokio::time::sleep(IDLE).await;

    // This read is itself the probe: the fake applies the heartbeat rule when the
    // command arrives, so if the keepalive had been silent it would revoke
    // privilege now and answer 0. Read through the device rather than the nodemap
    // to keep the assertion about CCP alone.
    let privilege = tokio::task::spawn_blocking(move || {
        let handle = tokio::runtime::Handle::current();
        let mut device = camera.transport().lock_device().expect("lock device");
        handle
            .block_on(device.read_register(gvcp_consts::CONTROL_CHANNEL_PRIVILEGE as u32))
            .expect("read CCP")
    })
    .await
    .expect("join");

    assert_ne!(
        privilege & gvcp_consts::CCP_CONTROLLER_BITS,
        0,
        "control privilege was lost across {IDLE:?} of application inactivity \
         (ccp=0x{privilege:08x}); the transport keepalive is not running"
    );
}

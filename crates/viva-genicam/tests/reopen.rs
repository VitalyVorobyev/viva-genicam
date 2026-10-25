//! A camera that went away and came back, against the in-process fake
//! (backlog API-21): what the session reports while it is gone, and
//! `reopen_gige` opening it again.
//!
//! The fake's link goes down with `FakeCamera::unplug`, as the Manta G-125B's
//! did when the host's adapter was switched off for 10 s: nothing it is sent
//! is answered, nothing it sends arrives, and its heartbeat runs out, so it
//! comes back with nobody in control.

mod common;

use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use viva_fake_gige::{FAKE_MAC, REG_BLACK_LEVEL};
use viva_genicam::gige::{self, DeviceInfo};
use viva_genicam::{
    Access, Camera, ControlState, GigeRegisterIo, Reopen, connect_gige, reopen_gige,
};

/// The fake's heartbeat window: short, so a test outlasts it quickly.
const HEARTBEAT_MS: u32 = 600;

async fn discovered() -> DeviceInfo {
    gige::discover_ip(Ipv4Addr::LOCALHOST, Duration::from_millis(500))
        .await
        .expect("discovery")
        .expect("the fake answers")
}

fn local_addr(camera: &Camera<GigeRegisterIo>) -> std::net::SocketAddr {
    camera
        .transport()
        .lock_device()
        .expect("device")
        .local_addr()
        .expect("local address")
}

async fn black_level(fake: &viva_fake_gige::FakeCamera) -> u32 {
    let bytes = fake.peek(REG_BLACK_LEVEL, 4).await;
    u32::from_be_bytes(bytes.try_into().expect("four bytes"))
}

/// While the link is down the session cannot tell; once it is back the
/// camera's heartbeat has run out, and the session reports the loss.
#[tokio::test(flavor = "multi_thread")]
async fn a_link_that_comes_back_after_the_heartbeat_finds_control_lost() {
    let cam = common::TestCamera::start_with(|b| {
        b.enforce_heartbeat(true).heartbeat_timeout_ms(HEARTBEAT_MS)
    })
    .await;
    let camera = connect_gige(&discovered().await).await.expect("open");
    let mut changes = camera.transport().watch_control();

    cam.fake().unplug();
    tokio::time::sleep(Duration::from_millis(u64::from(HEARTBEAT_MS) * 3)).await;
    assert_eq!(
        camera.transport().control_state(),
        ControlState::Held,
        "an unanswered keepalive is not a loss: it may be one dropped datagram"
    );
    cam.fake().plug_in();
    tokio::time::timeout(
        Duration::from_secs(5),
        changes.wait_for(|state| *state == ControlState::Lost),
    )
    .await
    .expect("the loss is reported once the camera answers again")
    .expect("the session is alive");
    assert_eq!(cam.fake().controller().await, None);
    assert!(cam.fake().heartbeat_expiries() >= 1);
}

/// `reopen_gige` waits for the camera to come back, opens it in a new
/// session, and puts back the configuration saved before it went away.
#[tokio::test(flavor = "multi_thread")]
async fn a_camera_that_went_away_is_reopened_with_its_configuration() {
    let cam = common::TestCamera::start_with(|b| {
        b.enforce_heartbeat(true).heartbeat_timeout_ms(HEARTBEAT_MS)
    })
    .await;
    let previous = discovered().await;
    assert_eq!(previous.mac, FAKE_MAC);
    let mut camera = connect_gige(&previous).await.expect("open");
    camera.set("BlackLevel", "7").expect("set BlackLevel");
    let saved = camera
        .save_features(&mut Vec::new())
        .expect("save the configuration");
    camera.set("BlackLevel", "3").expect("set BlackLevel");
    assert_eq!(black_level(cam.fake()).await, 3);

    cam.fake().unplug();
    // The application lets the old session go; its release is lost.
    drop(camera);
    let fake = cam.fake();
    let back = async {
        tokio::time::sleep(Duration::from_millis(1500)).await;
        fake.plug_in();
    };
    let options = Reopen::new()
        .patience(Duration::from_secs(15))
        .configuration(saved);
    let reopen = reopen_gige(&previous, &options);
    let ((), reopened) = tokio::join!(back, reopen);
    let reopened = reopened.expect("the camera is reopened once it is back");

    assert!(
        reopened.attempts > 1,
        "it was not back at the first attempt"
    );
    assert_eq!(reopened.device.mac, FAKE_MAC);
    assert!(!reopened.xml.is_empty());
    assert_eq!(
        reopened.camera.transport().control_state(),
        ControlState::Held
    );
    assert_eq!(
        cam.fake().controller().await,
        Some(local_addr(&reopened.camera)),
        "the camera's controller is the new session"
    );
    let restored = reopened.restored.expect("a configuration was given");
    assert!(restored.is_complete(), "{:?}", restored.failures);
    assert_eq!(
        black_level(cam.fake()).await,
        7,
        "the saved value is on the device again"
    );
}

/// A camera that stays away is given up on once the patience is spent.
#[tokio::test(flavor = "multi_thread")]
async fn reopening_gives_up_when_the_camera_stays_away() {
    let cam = common::TestCamera::start().await;
    let previous = discovered().await;
    cam.fake().unplug();
    let started = Instant::now();
    let result = reopen_gige(
        &previous,
        &Reopen::new()
            .patience(Duration::from_secs(2))
            .discovery_timeout(Duration::from_millis(200)),
    )
    .await;
    assert!(result.is_err(), "nothing answered, so nothing was opened");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "gave up after {:?}",
        started.elapsed()
    );
    assert_eq!(cam.fake().controller().await, None);
}

/// A camera found at its old address but with another MAC is not the one
/// that went away: it is not opened.
#[tokio::test(flavor = "multi_thread")]
async fn another_camera_at_the_old_address_is_not_opened() {
    let cam = common::TestCamera::start().await;
    let mut previous = discovered().await;
    previous.mac = [0x02, 0, 0, 0, 0, 0x01];
    let result = reopen_gige(
        &previous,
        &Reopen::new()
            .access(Access::Control)
            .patience(Duration::from_secs(1))
            .discovery_timeout(Duration::from_millis(200)),
    )
    .await;
    assert!(result.is_err());
    assert_eq!(
        cam.fake().controller().await,
        None,
        "the camera at that address was not claimed"
    );
}

/// A discovery timeout longer than the entire retry budget must not turn
/// patience into a per-attempt timeout.
#[tokio::test(flavor = "multi_thread")]
async fn patience_bounds_an_inflight_discovery() {
    let cam = common::TestCamera::start().await;
    let previous = discovered().await;
    cam.fake().unplug();
    let started = Instant::now();
    let patience = Duration::from_millis(100);
    let result = reopen_gige(
        &previous,
        &Reopen::new()
            .patience(patience)
            .discovery_timeout(Duration::from_secs(10)),
    )
    .await;
    assert!(matches!(result, Err(viva_genicam::GenicamError::Timeout(d)) if d == patience));
    assert!(started.elapsed() < Duration::from_secs(1));
}

/// Cancellation after claiming control, while XML is still being fetched,
/// sends release on that socket rather than abandoning privilege until expiry.
#[tokio::test(flavor = "multi_thread")]
async fn cancelling_an_open_releases_its_claim() {
    const XML_BASE: u64 = 0x1_0000;
    let cam = common::TestCamera::start_with(|b| {
        b.enforce_heartbeat(true)
            .delay_ack(XML_BASE, Duration::from_secs(5))
    })
    .await;
    let previous = discovered().await;
    let opening = tokio::spawn(async move { reopen_gige(&previous, &Reopen::new()).await });
    tokio::time::timeout(Duration::from_secs(3), async {
        while cam
            .fake()
            .commands()
            .at(viva_fake_gige::GvcpCommand::ReadMem, XML_BASE)
            == 0
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the XML fetch started");
    assert!(cam.fake().controller().await.is_some());
    opening.abort();
    assert!(opening.await.is_err_and(|error| error.is_cancelled()));
    tokio::time::timeout(Duration::from_secs(1), async {
        while cam.fake().controller().await.is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the cancelled connection released its control privilege");
}

/// A restore failure is a report about an otherwise usable new session,
/// not a reason to claim the camera repeatedly.
#[tokio::test(flavor = "multi_thread")]
async fn a_restore_failure_returns_the_new_camera_and_report() {
    let _cam = common::TestCamera::start().await;
    let previous = discovered().await;
    let mut configuration = viva_genicam::genapi::Configuration::default();
    configuration
        .values
        .push(viva_genicam::genapi::persistence::SavedValue {
            feature: "FeatureRemovedByFirmware".into(),
            selectors: Vec::new(),
            value: "1".into(),
        });
    let reopened = reopen_gige(&previous, &Reopen::new().configuration(configuration))
        .await
        .expect("the connection succeeded");
    assert_eq!(reopened.attempts, 1);
    let report = reopened.restored.expect("restore report");
    assert_eq!(report.failures.len(), 1);
    assert_eq!(report.failures[0].value.feature, "FeatureRemovedByFirmware");
    assert_eq!(reopened.camera.get_integer("Width").expect("Width"), 640);
}

#[tokio::test]
async fn zero_patience_does_not_start_discovery() {
    let previous = DeviceInfo::from_ip(Ipv4Addr::LOCALHOST);
    let result = reopen_gige(&previous, &Reopen::new().patience(Duration::ZERO)).await;
    assert!(matches!(result, Err(viva_genicam::GenicamError::Timeout(d)) if d.is_zero()));
}

/// A camera may have applied CCP before its acknowledgement arrives. That
/// uncertainty is still this socket's claim, and cancellation releases it.
#[tokio::test(flavor = "multi_thread")]
async fn cancelling_while_the_claim_ack_is_pending_releases_control() {
    let cam = common::TestCamera::start_with(|b| {
        b.enforce_heartbeat(true)
            .delay_ack(0x0a00, Duration::from_secs(5))
    })
    .await;
    let previous = discovered().await;
    let opening = tokio::spawn(async move { reopen_gige(&previous, &Reopen::new()).await });
    tokio::time::timeout(Duration::from_secs(3), async {
        while cam.fake().controller().await.is_none() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the device took the claim");
    opening.abort();
    assert!(opening.await.is_err_and(|error| error.is_cancelled()));
    tokio::time::timeout(Duration::from_secs(1), async {
        while cam.fake().controller().await.is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the cancelled claim was released");
}

/// The old address may stop answering entirely. Discovery on the selected
/// interface still finds the same MAC at the address it uses now.
#[tokio::test(flavor = "multi_thread")]
async fn a_camera_at_another_address_is_found_by_mac() {
    let _cam = common::TestCamera::start().await;
    let mut previous = discovered().await;
    previous.ip = Ipv4Addr::new(127, 0, 0, 2);
    let interface = gige::Iface::from_ipv4(Ipv4Addr::LOCALHOST).expect("loopback");
    let reopened = reopen_gige(
        &previous,
        &Reopen::new()
            .interface(interface.name())
            .discovery_timeout(Duration::from_millis(100)),
    )
    .await
    .expect("rediscovered by MAC");
    assert_eq!(reopened.device.mac, previous.mac);
    assert_eq!(reopened.device.ip, Ipv4Addr::LOCALHOST);
}

/// The camera's dedicated runtime is also retired when a current-thread
/// caller cancels a connection whose XML fetch already holds control.
#[tokio::test]
async fn current_thread_cancellation_releases_the_claim_without_panicking() {
    const XML_BASE: u64 = 0x1_0000;
    let cam = common::TestCamera::start_with(|b| {
        b.enforce_heartbeat(true)
            .delay_ack(XML_BASE, Duration::from_secs(5))
    })
    .await;
    let previous = discovered().await;
    let opening = tokio::spawn(async move { reopen_gige(&previous, &Reopen::new()).await });
    tokio::time::timeout(Duration::from_secs(3), async {
        while cam
            .fake()
            .commands()
            .at(viva_fake_gige::GvcpCommand::ReadMem, XML_BASE)
            == 0
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the XML fetch started");
    opening.abort();
    assert!(opening.await.is_err_and(|error| error.is_cancelled()));
    tokio::time::timeout(Duration::from_secs(1), async {
        while cam.fake().controller().await.is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the retired runtime released control");
}

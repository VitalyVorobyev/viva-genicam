//! Integration tests for `NodeMap::is_implemented` / `is_available` /
//! `effective_access_mode` / `available_enum_entries` against the in-process
//! fake GigE Vision camera.
//!
//! The fake camera's XML wires realistic predicates on real features:
//!   * `ExposureTime.pIsLocked` ← `ExposureAuto != Off`
//!   * `Gain.pIsLocked` ← `GainAuto != Off`
//!   * `AcquisitionFrameRate.pIsAvailable` ← `AcquisitionFrameRateEnable`
//!   * `PixelFormat` entry `pIsImplemented` ← `SensorType` (Monochrome /
//!     BayerRG / Color)
//!
//! Each test flips one driver feature via `Camera::set` and verifies the
//! NodeMap predicate methods reflect the new state end-to-end.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use viva_genapi::AccessMode;
use viva_genapi::RegisterIo;
use viva_genicam::{Camera, GigeRegisterIo, connect_gige, gige};

async fn discover_fake() -> gige::DeviceInfo {
    let devices = gige::discover_all(Duration::from_secs(2))
        .await
        .expect("discovery failed");
    devices
        .into_iter()
        .find(|d| d.ip.is_loopback())
        .expect("fake camera not found on loopback")
}

async fn connect_fake() -> Arc<Mutex<Camera<GigeRegisterIo>>> {
    let device = discover_fake().await;
    let camera = connect_gige(&device).await.expect("connect failed");
    Arc::new(Mutex::new(camera))
}

async fn set_feature(
    camera: &Arc<Mutex<Camera<GigeRegisterIo>>>,
    name: &'static str,
    value: String,
) {
    let cam = camera.clone();
    tokio::task::spawn_blocking(move || {
        let mut cam = cam.lock().unwrap();
        cam.set(name, &value)
            .unwrap_or_else(|e| panic!("set {name}: {e}"));
    })
    .await
    .unwrap();
}

async fn get_feature(camera: &Arc<Mutex<Camera<GigeRegisterIo>>>, name: &'static str) -> String {
    let cam = camera.clone();
    tokio::task::spawn_blocking(move || {
        let cam = cam.lock().unwrap();
        cam.get(name).unwrap_or_else(|e| panic!("get {name}: {e}"))
    })
    .await
    .unwrap()
}

async fn read_predicate<F, T>(camera: &Arc<Mutex<Camera<GigeRegisterIo>>>, f: F) -> T
where
    F: FnOnce(&Camera<GigeRegisterIo>) -> T + Send + 'static,
    T: Send + 'static,
{
    let cam = camera.clone();
    tokio::task::spawn_blocking(move || {
        let cam = cam.lock().unwrap();
        f(&cam)
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn test_exposure_time_locked_by_exposure_auto() {
    let _cam = common::TestCamera::start().await;
    let camera = connect_fake().await;

    // ExposureAuto=Off → ExposureTime is RW.
    set_feature(&camera, "ExposureAuto", "Off".into()).await;
    let mode = read_predicate(&camera, |cam| {
        cam.nodemap()
            .effective_access_mode("ExposureTime", cam.transport())
            .unwrap()
    })
    .await;
    assert_eq!(mode, AccessMode::RW, "ExposureAuto=Off → RW");

    // ExposureAuto=Continuous → pIsLocked truthy → RW downgrades to RO.
    set_feature(&camera, "ExposureAuto", "Continuous".into()).await;
    let mode = read_predicate(&camera, |cam| {
        cam.nodemap()
            .effective_access_mode("ExposureTime", cam.transport())
            .unwrap()
    })
    .await;
    assert_eq!(mode, AccessMode::RO, "ExposureAuto=Continuous → RO");
}

#[tokio::test]
async fn test_gain_locked_by_gain_auto() {
    let _cam = common::TestCamera::start().await;
    let camera = connect_fake().await;

    set_feature(&camera, "GainAuto", "Off".into()).await;
    let mode = read_predicate(&camera, |cam| {
        cam.nodemap()
            .effective_access_mode("Gain", cam.transport())
            .unwrap()
    })
    .await;
    assert_eq!(mode, AccessMode::RW, "GainAuto=Off → RW");

    set_feature(&camera, "GainAuto", "Continuous".into()).await;
    let mode = read_predicate(&camera, |cam| {
        cam.nodemap()
            .effective_access_mode("Gain", cam.transport())
            .unwrap()
    })
    .await;
    assert_eq!(mode, AccessMode::RO, "GainAuto=Continuous → RO");
}

#[tokio::test]
async fn test_acquisition_frame_rate_gated_by_enable() {
    let _cam = common::TestCamera::start().await;
    let camera = connect_fake().await;

    // Enabled at boot.
    let available = read_predicate(&camera, |cam| {
        cam.nodemap()
            .is_available("AcquisitionFrameRate", cam.transport())
            .unwrap()
    })
    .await;
    assert!(available, "AcquisitionFrameRateEnable=1 → available");

    set_feature(&camera, "AcquisitionFrameRateEnable", "0".into()).await;
    let available = read_predicate(&camera, |cam| {
        cam.nodemap()
            .is_available("AcquisitionFrameRate", cam.transport())
            .unwrap()
    })
    .await;
    assert!(!available, "AcquisitionFrameRateEnable=0 → unavailable");

    set_feature(&camera, "AcquisitionFrameRateEnable", "1".into()).await;
    let available = read_predicate(&camera, |cam| {
        cam.nodemap()
            .is_available("AcquisitionFrameRate", cam.transport())
            .unwrap()
    })
    .await;
    assert!(available, "re-enabled → available again");
}

#[tokio::test]
async fn test_available_enum_entries_filters_pixel_format_by_sensor_type() {
    let _cam = common::TestCamera::start().await;
    let camera = connect_fake().await;

    // Monochrome sensor → Mono8 + Mono16 only.
    set_feature(&camera, "SensorType", "Monochrome".into()).await;
    let entries = read_predicate(&camera, |cam| {
        cam.nodemap()
            .available_enum_entries("PixelFormat", cam.transport())
            .unwrap()
    })
    .await;
    assert!(entries.contains(&"Mono8".to_string()));
    assert!(entries.contains(&"Mono16".to_string()));
    assert!(!entries.contains(&"BayerRG8".to_string()));
    assert!(!entries.contains(&"RGB8".to_string()));

    // Bayer sensor → only BayerRG8.
    set_feature(&camera, "SensorType", "BayerRG".into()).await;
    let entries = read_predicate(&camera, |cam| {
        cam.nodemap()
            .available_enum_entries("PixelFormat", cam.transport())
            .unwrap()
    })
    .await;
    assert_eq!(entries, vec!["BayerRG8".to_string()]);

    // Color sensor → only RGB8.
    set_feature(&camera, "SensorType", "Color".into()).await;
    let entries = read_predicate(&camera, |cam| {
        cam.nodemap()
            .available_enum_entries("PixelFormat", cam.transport())
            .unwrap()
    })
    .await;
    assert_eq!(entries, vec!["RGB8".to_string()]);
}

/// The #45 scenario, end to end: a write the camera's own XML forbids must be
/// refused locally and name the lock, rather than reaching the wire and coming
/// back as a bare device status.
///
/// The reporter's FLIR declared `ExposureTime` with no `<AccessMode>` — so it
/// defaulted to `RW` — and put the whole restriction in `pIsLocked`. The fake's
/// XML wires the same shape onto the same real features.
#[tokio::test]
async fn writing_a_locked_exposure_time_is_refused_before_the_wire() {
    let _cam = common::TestCamera::start().await;
    let camera = connect_fake().await;

    // ExposureAuto=Off → the lock is clear and the write lands.
    set_feature(&camera, "ExposureAuto", "Off".into()).await;
    let wrote = {
        let cam = camera.clone();
        tokio::task::spawn_blocking(move || {
            let mut cam = cam.lock().unwrap();
            cam.set_exposure_time_us(10_000.0)
        })
        .await
        .unwrap()
    };
    assert!(wrote.is_ok(), "unlocked write should succeed: {wrote:?}");

    // ExposureAuto=Continuous → the device reports the node locked.
    set_feature(&camera, "ExposureAuto", "Continuous".into()).await;
    let err = {
        let cam = camera.clone();
        tokio::task::spawn_blocking(move || {
            let mut cam = cam.lock().unwrap();
            cam.set_exposure_time_us(10_000.0)
        })
        .await
        .unwrap()
    }
    .expect_err("a locked ExposureTime must not be written");

    // The message has to be actionable: it names the node and the feature
    // holding the lock. "access denied" alone is what sent #45's reporter to
    // the issue tracker.
    let msg = err.to_string();
    assert!(
        msg.contains("ExposureTime") && msg.contains("ExposureAutoActive"),
        "error should name the node and its lock, got: {msg}"
    );
}

/// `Camera::execute_command` drives a `<Command>` that reaches its register
/// through `<pValue>`, and the device state actually changes.
///
/// Both halves matter. All 432 `<Command>` nodes in the vendor XML corpus use
/// `<pValue>`; until `UserSetLoad` was added, all three of the fake's commands
/// used a bare `<Address>`, so the integration suite exercised only the path no
/// real camera takes (backlog `GA-10`).
///
/// And the assertion is on the camera, not on our own return value: a command
/// that merely acknowledges a write can be "verified" by a test that proves
/// nothing. Here the exposure is moved away from the boot default first, so a
/// no-op execute fails the test.
#[tokio::test]
async fn execute_command_through_pvalue_changes_device_state() {
    let _cam = common::TestCamera::start().await;
    let camera = connect_fake().await;

    set_feature(&camera, "ExposureTime", "20000.0".into()).await;
    let moved = get_feature(&camera, "ExposureTime").await;
    assert!(
        moved.starts_with("20000"),
        "precondition: exposure should have moved off the default, got {moved}"
    );

    set_feature(&camera, "UserSetSelector", "Default".into()).await;

    {
        let cam = camera.clone();
        tokio::task::spawn_blocking(move || {
            let mut cam = cam.lock().unwrap();
            cam.execute_command("UserSetLoad")
        })
        .await
        .unwrap()
        .expect("UserSetLoad should execute");
    }

    // Both reads are asserted, and they prove different things:
    //
    //  * The transport read asserts what the *camera* did, not what our
    //    nodemap believes. A command that only returns `Ok` can be "verified"
    //    by a test that proves nothing.
    //  * The `Camera::get` read asserts that our cache learned about it.
    //    Nothing we wrote touched `ExposureTime`; what makes it stale is the
    //    `<pInvalidator>` on its register, which names `UserSetLoad`'s pValue
    //    register the way FLIR's XML does (backlog `GA-24`).
    let raw = {
        let cam = camera.clone();
        tokio::task::spawn_blocking(move || {
            let cam = cam.lock().unwrap();
            cam.transport()
                .read(viva_fake_gige::registers::REG_EXPOSURE_TIME, 8)
        })
        .await
        .unwrap()
        .expect("read exposure register")
    };
    let restored = f64::from_be_bytes(raw.try_into().expect("8 bytes"));
    assert_eq!(
        restored,
        viva_fake_gige::registers::DEFAULT_EXPOSURE_US,
        "UserSetLoad should have restored the default exposure on the device"
    );

    let reported = get_feature(&camera, "ExposureTime").await;
    let reported: f64 = reported
        .parse()
        .unwrap_or_else(|_| panic!("ExposureTime should be a float, got {reported}"));
    assert_eq!(
        reported,
        viva_fake_gige::registers::DEFAULT_EXPOSURE_US,
        "Camera::get should re-read ExposureTime after UserSetLoad, not answer from a stale cache"
    );
}

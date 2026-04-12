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

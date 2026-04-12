//! Integration tests for `NodeMap::is_implemented` / `is_available` /
//! `effective_access_mode` / `available_enum_entries` against the in-process
//! fake GigE Vision camera.
//!
//! The fake camera's XML includes a `TestControlReg` bitfield whose bits
//! toggle predicate outcomes (see `crates/viva-fake-gige/src/registers.rs`).
//! These tests flip bits via `set_integer` and verify the predicates reflect
//! the new state.

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

async fn set_test_ctrl(camera: &Arc<Mutex<Camera<GigeRegisterIo>>>, value: i64) {
    let cam = camera.clone();
    tokio::task::spawn_blocking(move || {
        let mut cam = cam.lock().unwrap();
        cam.set("TestControlReg", &value.to_string())
            .expect("set TestControlReg");
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
async fn test_is_implemented_reflects_gate_bit() {
    let _cam = common::TestCamera::start().await;
    let camera = connect_fake().await;

    // Bit 0 cleared → TestGatedFeature not implemented.
    set_test_ctrl(&camera, 0).await;
    let implemented = read_predicate(&camera, |cam| {
        cam.nodemap()
            .is_implemented("TestGatedFeature", cam.transport())
            .unwrap()
    })
    .await;
    assert!(!implemented, "bit 0 clear → not implemented");

    // Bit 0 set → TestGatedFeature implemented.
    set_test_ctrl(&camera, 0b0001).await;
    let implemented = read_predicate(&camera, |cam| {
        cam.nodemap()
            .is_implemented("TestGatedFeature", cam.transport())
            .unwrap()
    })
    .await;
    assert!(implemented, "bit 0 set → implemented");
}

#[tokio::test]
async fn test_effective_access_mode_locked_downgrades() {
    let _cam = common::TestCamera::start().await;
    let camera = connect_fake().await;

    // Bit 0 set, bit 1 clear → implemented and unlocked → RW.
    set_test_ctrl(&camera, 0b0001).await;
    let mode = read_predicate(&camera, |cam| {
        cam.nodemap()
            .effective_access_mode("TestGatedFeature", cam.transport())
            .unwrap()
    })
    .await;
    assert_eq!(mode, AccessMode::RW);

    // Bit 0 and bit 1 both set → implemented but locked → RO.
    set_test_ctrl(&camera, 0b0011).await;
    let mode = read_predicate(&camera, |cam| {
        cam.nodemap()
            .effective_access_mode("TestGatedFeature", cam.transport())
            .unwrap()
    })
    .await;
    assert_eq!(mode, AccessMode::RO);
}

#[tokio::test]
async fn test_available_enum_entries_filters_pixel_format() {
    let _cam = common::TestCamera::start().await;
    let camera = connect_fake().await;

    // Clear the Mono8/BayerRG8 gate bits but leave Mono16/RGB8 alone.
    // Default boot value is 0x33 (all bits set).
    set_test_ctrl(&camera, 0b0011).await;

    let entries = read_predicate(&camera, |cam| {
        cam.nodemap()
            .available_enum_entries("PixelFormat", cam.transport())
            .unwrap()
    })
    .await;
    assert!(
        entries.contains(&"Mono16".to_string()),
        "always-available entry present"
    );
    assert!(
        entries.contains(&"RGB8".to_string()),
        "always-available entry present"
    );
    assert!(
        !entries.contains(&"Mono8".to_string()),
        "gated-out entry hidden"
    );
    assert!(
        !entries.contains(&"BayerRG8".to_string()),
        "gated-out entry hidden"
    );

    // Flip bit 4 back on → Mono8 returns.
    set_test_ctrl(&camera, 0b0001_0011).await;
    let entries = read_predicate(&camera, |cam| {
        cam.nodemap()
            .available_enum_entries("PixelFormat", cam.transport())
            .unwrap()
    })
    .await;
    assert!(entries.contains(&"Mono8".to_string()));
}

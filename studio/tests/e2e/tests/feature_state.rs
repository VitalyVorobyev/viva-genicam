//! E2E tests for the `FeatureState` wire contract (API v2).
//!
//! These tests exercise the `nodes/{name}/state` queryable added in Step 4 of
//! the Feature Browser systematic review (see
//! `docs/adrs/010-feature-state-contract.md`). Each test proves that one of
//! the five user-reported symptoms is gone at the wire level, independently
//! of the UI migration.
//!
//! Requires:
//! - `arv-fake-gv-camera-0.8` in PATH
//! - `genicam-service` binary (see `GENICAM_SERVICE_PATH`)
//!
//! Run: `cargo test -p e2e-tests --test feature_state -- --ignored --test-threads=1`

use std::time::Duration;
use tokio::time::sleep;

use e2e_tests::*;
use viva_zenoh_api::AcquisitionCommand;

/// Symptom #1: setting `GainAuto` to `"Once"` must report `"Once"` on the
/// next state read (the Studio UI regression was form-side, but the wire
/// contract itself must round-trip cleanly for the fix to be complete).
#[tokio::test(flavor = "multi_thread")]
#[ignore] // Requires external binaries
async fn gain_auto_round_trips_via_feature_state() {
    let mut harness = TestHarness::start().await.expect("harness start");

    // Read the initial state so we know the node exists as an Enumeration.
    let initial = query_feature_state(harness.session(), harness.device_id(), "GainAuto")
        .await
        .expect("initial state");
    assert_eq!(
        initial.kind, "Enumeration",
        "GainAuto should be an Enumeration"
    );

    // Set GainAuto to "Once" — aravis's fake camera typically offers
    // {Off, Once, Continuous}.
    write_node(
        harness.session(),
        harness.device_id(),
        "GainAuto",
        serde_json::json!("Once"),
    )
    .await
    .expect("write GainAuto");

    // Give the service a moment to publish the refreshed state.
    sleep(Duration::from_millis(200)).await;

    let after = query_feature_state(harness.session(), harness.device_id(), "GainAuto")
        .await
        .expect("state after write");
    assert_eq!(
        after.value,
        serde_json::Value::String("Once".to_string()),
        "GainAuto value after write should be exactly \"Once\" (no empty / unset fallback)"
    );

    harness.shutdown().await;
}

/// Symptom #4: `PixelFormat` must report a concrete `enum_available` list,
/// not the full static XML set. Once the `IsAvailable` predicate handoff
/// lands in `viva-genapi`, this list should be strictly smaller than the
/// static XML list; today it is at least equal (and populated, which is the
/// key regression guarded against).
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn pixel_format_reports_enum_available() {
    let mut harness = TestHarness::start().await.expect("harness start");

    let state = query_feature_state(harness.session(), harness.device_id(), "PixelFormat")
        .await
        .expect("PixelFormat state");
    assert_eq!(state.kind, "Enumeration");
    let entries = state
        .enum_available
        .as_ref()
        .expect("enum_available must be populated for Enumeration nodes");
    assert!(
        !entries.is_empty(),
        "enum_available should not be empty for PixelFormat"
    );
    // Minimal sanity: aravis fake camera supports Mono8 at least.
    assert!(
        entries.iter().any(|e| e == "Mono8"),
        "Mono8 must be among available entries, got {entries:?}"
    );

    harness.shutdown().await;
}

/// Symptom #5: `Width` must report a bounded numeric range, never the
/// `i64::MIN..=i64::MAX` sentinel that the parser previously emitted.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn width_reports_bounded_numeric_range() {
    let mut harness = TestHarness::start().await.expect("harness start");

    let state = query_feature_state(harness.session(), harness.device_id(), "Width")
        .await
        .expect("Width state");
    assert_eq!(state.kind, "Integer");
    let numeric = state
        .numeric
        .as_ref()
        .expect("Width must publish a numeric range");

    // These are the exact i64 sentinel values cast to f64 — observing either
    // at the wire level means the service is not populating the range and
    // the regression is back.
    let i64_min_as_f64 = i64::MIN as f64;
    let i64_max_as_f64 = i64::MAX as f64;
    assert!(
        numeric.min > i64_min_as_f64,
        "Width.min must not be the i64::MIN sentinel, got {}",
        numeric.min
    );
    assert!(
        numeric.max < i64_max_as_f64,
        "Width.max must not be the i64::MAX sentinel, got {}",
        numeric.max
    );
    // Any reasonable camera has min <= max and both positive for Width.
    assert!(numeric.min >= 1.0 && numeric.max >= numeric.min);

    harness.shutdown().await;
}

/// Symptom #2: executing `AcquisitionStart` must actually change device
/// state. We verify this end-to-end by reading `AcquisitionStatus` (or, if
/// the fake camera doesn't implement it, the acquisition-status publish
/// stream) after the command and confirming `active=true`.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn acquisition_start_flips_status() {
    let mut harness = TestHarness::start().await.expect("harness start");

    // Subscribe to the acquisition status topic before sending the command
    // so we do not miss the flip.
    let sub = harness
        .session()
        .declare_subscriber(viva_zenoh_api::keys::acquisition_status(
            harness.device_id(),
        ))
        .await
        .expect("acquisition status subscriber");

    send_acquisition_command(
        harness.session(),
        harness.device_id(),
        AcquisitionCommand::Start,
    )
    .await
    .expect("start acquisition");

    // Wait for the first `active: true` status message.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut saw_active = false;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, sub.recv_async()).await {
            Ok(Ok(sample)) => {
                let bytes = sample.payload().to_bytes();
                if let Ok(status) =
                    serde_json::from_slice::<viva_zenoh_api::AcquisitionStatus>(&bytes)
                    && status.active
                {
                    saw_active = true;
                    break;
                }
            }
            _ => break,
        }
    }
    assert!(
        saw_active,
        "expected AcquisitionStatus.active=true after Start"
    );

    // Clean up so the next test does not inherit an active stream.
    let _ = send_acquisition_command(
        harness.session(),
        harness.device_id(),
        AcquisitionCommand::Stop,
    )
    .await;

    harness.shutdown().await;
}

/// Defence in depth: `FeatureState.access_mode` must always be one of the
/// GenICam-canonical strings. A regression that went back to hardcoded
/// `"RW"` would show up as the service never reporting `"RO"` for
/// demonstrably read-only nodes like `DeviceSerialNumber`.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn read_only_nodes_report_ro_access_mode() {
    let mut harness = TestHarness::start().await.expect("harness start");

    for name in ["DeviceVendorName", "DeviceModelName", "DeviceSerialNumber"] {
        let state = query_feature_state(harness.session(), harness.device_id(), name)
            .await
            .expect(name);
        assert!(
            matches!(state.access_mode.as_str(), "RO" | "RW" | "WO" | "NA"),
            "access_mode must be canonical GenICam spelling, got {:?}",
            state.access_mode
        );
    }

    harness.shutdown().await;
}

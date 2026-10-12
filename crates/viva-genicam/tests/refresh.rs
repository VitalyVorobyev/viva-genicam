//! Values the camera changes on its own, against the in-process fake
//! (GA-05, GA-40, ADR-0029).
//!
//! The fake's `DeviceTemperature` rises with time, as a sensor warms, and is
//! declared the way FLIR's BFS declares its own: `NoCache`, polled every
//! 500 ms. Its `GevTimestampValue` is latched by `TimestampLatch` with nothing
//! in the XML linking the two. Every assertion compares a read with the
//! fake's own register, past the protocol.

mod common;

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use viva_fake_gige::registers::{REG_DEVICE_TEMPERATURE, REG_TIMESTAMP_VALUE};
use viva_genicam::genapi::ManualClock;
use viva_genicam::gige::DeviceInfo;
use viva_genicam::{Camera, GigeRegisterIo, connect_gige};

async fn open() -> Camera<GigeRegisterIo> {
    connect_gige(&DeviceInfo::from_ip(Ipv4Addr::LOCALHOST))
        .await
        .expect("open the fake")
}

/// The fake's temperature now, in °C, read past the protocol.
async fn temperature(cam: &common::TestCamera) -> f64 {
    let raw = cam.fake().peek(REG_DEVICE_TEMPERATURE, 4).await;
    f64::from(i32::from_be_bytes(raw.try_into().unwrap())) / 256.0
}

/// GA-05: a `NoCache` value is read from the camera every time. Before, the
/// first reading was cached until something invalidated it — and nothing
/// does for a value the camera moves by itself.
#[tokio::test(flavor = "multi_thread")]
async fn a_temperature_the_camera_changes_on_its_own_is_read_fresh() {
    let cam = common::TestCamera::start().await;
    let camera = open().await;
    let first = camera.get_float("DeviceTemperature").expect("first read");

    let waited = Instant::now();
    while temperature(&cam).await == first {
        assert!(
            waited.elapsed() < Duration::from_secs(3),
            "the fake's temperature never moved"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let before = temperature(&cam).await;
    let read = camera.get_float("DeviceTemperature").expect("second read");
    let after = temperature(&cam).await;
    assert_ne!(read, first, "the first reading, from a cache");
    assert!(
        read == before || read == after,
        "read {read} °C; the camera held {before} then {after}"
    );
}

/// GA-40: a value the camera changed in a way its XML does not describe is
/// read again after `invalidate` — or `invalidate_all`.
#[tokio::test(flavor = "multi_thread")]
async fn invalidate_forgets_a_value_the_xml_does_not_link() {
    let cam = common::TestCamera::start().await;
    let mut camera = open().await;
    let latched = || async {
        let raw = cam.fake().peek(REG_TIMESTAMP_VALUE, 8).await;
        i64::try_from(u64::from_be_bytes(raw.try_into().unwrap())).unwrap()
    };

    camera.execute_command("TimestampLatch").expect("latch");
    let first = camera.get_integer("GevTimestampValue").expect("read");
    assert_eq!(first, latched().await);

    tokio::time::sleep(Duration::from_millis(5)).await;
    camera
        .execute_command("TimestampLatch")
        .expect("latch again");
    assert_ne!(latched().await, first, "the camera latched a later time");
    camera.invalidate("GevTimestampValue");
    assert_eq!(
        camera.get_integer("GevTimestampValue").expect("read"),
        latched().await
    );

    tokio::time::sleep(Duration::from_millis(5)).await;
    camera
        .execute_command("TimestampLatch")
        .expect("latch a third time");
    camera.invalidate_all();
    assert_eq!(
        camera.get_integer("GevTimestampValue").expect("read"),
        latched().await
    );
}

/// GA-40: a viewer learns what to read again without callbacks: `poll`
/// names the temperature once its polling time is over, and
/// `changed_since` reports it from the generation the viewer last saw.
#[tokio::test(flavor = "multi_thread")]
async fn poll_and_changed_since_name_the_temperature_when_it_is_due() {
    let _cam = common::TestCamera::start().await;
    let mut camera = open().await;
    let clock = Arc::new(ManualClock::new());
    camera.nodemap_mut().set_clock(clock.clone());

    let seen = camera.generation();
    camera.get_float("DeviceTemperature").expect("read");
    assert!(camera.poll().is_empty(), "not due yet");
    let (changed, now) = camera.changed_since(seen);
    assert!(changed.is_empty(), "a read is not a change: {changed:?}");

    clock.advance(Duration::from_millis(500));
    let due = camera.poll();
    assert!(
        due.iter().any(|name| name == "DeviceTemperature"),
        "the feature, not only its register: {due:?}"
    );
    let (changed, later) = camera.changed_since(now);
    assert!(changed.iter().any(|name| name == "DeviceTemperature"));
    assert!(later > now);
}

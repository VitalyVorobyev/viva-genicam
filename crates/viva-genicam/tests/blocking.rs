//! API-16: a GigE camera from a program with no async runtime of its own,
//! and from a current-thread runtime, against the in-process fake camera.
//!
//! The fake runs on a runtime of its own here, as a camera runs on its own
//! hardware: the test thread that drives the blocking API is outside it.

mod common;

use std::net::Ipv4Addr;
use std::time::Duration;

use viva_genicam::gige::{DeviceInfo, DiscoveryOptions};
use viva_genicam::{StreamOptions, blocking, connect_gige};

fn fake_info() -> DeviceInfo {
    DeviceInfo::from_ip(Ipv4Addr::LOCALHOST)
}

/// Discover, open, read, write, execute, stream and receive frames, all on
/// a plain thread; dropping the camera gives control back.
#[test]
fn a_program_without_a_runtime_does_everything_through_the_blocking_api() {
    let fake_runtime = tokio::runtime::Runtime::new().expect("runtime for the fake");
    let cam = fake_runtime.block_on(common::TestCamera::start());

    let found = blocking::discover_with(
        Duration::from_millis(300),
        &DiscoveryOptions::new().include_loopback(true),
    )
    .expect("discover");
    assert!(
        found.iter().any(|device| device.ip == Ipv4Addr::LOCALHOST),
        "{found:?}"
    );

    let mut camera = blocking::connect(&fake_info()).expect("open");
    assert_eq!(camera.get_integer("Width").expect("Width"), 640);
    camera.set_integer("Width", 320).expect("set Width");
    assert_eq!(camera.get_integer("Width").expect("Width"), 320);
    camera
        .execute_and_wait_blocking("UserSetLoad", Duration::from_secs(2))
        .expect("execute and wait");

    let mut frames = camera.open_stream(StreamOptions::new()).expect("stream");
    camera.acquisition_start().expect("start");
    let frame = frames
        .next_frame_blocking(Duration::from_secs(2))
        .expect("a frame")
        .expect("the stream is open");
    assert_eq!(frame.width, 320);
    camera.acquisition_stop().expect("stop");
    drop(frames);

    drop(camera);
    assert_eq!(
        fake_runtime.block_on(cam.fake().controller()),
        None,
        "dropping the camera released control"
    );
}

/// A camera opened from a current-thread runtime reads and writes from
/// that runtime's own thread, where blocking in place is not possible.
#[tokio::test]
async fn a_camera_opened_on_a_current_thread_runtime_reads_and_writes() {
    let fake_runtime = tokio::runtime::Runtime::new().expect("runtime for the fake");
    let cam = fake_runtime
        .spawn(common::TestCamera::start())
        .await
        .expect("start the fake");

    let mut camera = connect_gige(&fake_info()).await.expect("open");
    assert_eq!(camera.get_integer("Width").expect("Width"), 640);
    camera.set_integer("Width", 320).expect("set Width");
    assert_eq!(camera.get_integer("Width").expect("Width"), 320);
    drop(camera);

    let controller = fake_runtime
        .spawn(async move { cam.fake().controller().await })
        .await
        .expect("ask the fake");
    assert_eq!(controller, None, "dropping the camera released control");
    fake_runtime.shutdown_background();
}

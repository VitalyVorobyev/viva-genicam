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

/// Codex on #199: a connect made from a current-thread runtime runs on a
/// runtime of the camera's own, and cancelling it — a timeout around it —
/// dropped that runtime inside the async context, where Tokio panics.
#[tokio::test]
async fn a_connect_cancelled_on_a_current_thread_runtime_does_not_panic() {
    let _silent = common::SilentCamera::bind().await;
    let attempt =
        tokio::time::timeout(Duration::from_millis(100), connect_gige(&fake_info())).await;
    assert!(
        attempt.is_err(),
        "a camera that never answers cannot be opened"
    );
}

/// `PayloadSize` is the size of the blocks the camera sends — the image
/// alone, then the image with its chunks once chunk mode is on — which a
/// host that sizes its buffers by it, a GenTL consumer among them, relies on.
#[test]
fn payload_size_is_the_size_of_each_block() {
    let fake_runtime = tokio::runtime::Runtime::new().expect("runtime for the fake");
    let _cam = fake_runtime.block_on(common::TestCamera::start());
    let mut camera = blocking::connect(&fake_info()).expect("open");
    for chunks in [false, true] {
        if chunks {
            camera.set("ChunkSelector", "FrameID").expect("select");
            camera.set_bool("ChunkEnable", true).expect("enable");
            camera
                .set_bool("ChunkModeActive", true)
                .expect("chunk mode");
        }
        let payload = camera.get_integer("PayloadSize").expect("PayloadSize");
        let mut frames = camera.open_stream(StreamOptions::new()).expect("stream");
        camera.acquisition_start().expect("start");
        let frame = frames
            .next_frame_blocking(Duration::from_secs(2))
            .expect("a frame")
            .expect("the stream is open");
        camera.acquisition_stop().expect("stop");
        drop(frames);
        assert_eq!(
            frame.raw_payload.len() as i64,
            payload,
            "chunk mode {chunks}"
        );
    }
}

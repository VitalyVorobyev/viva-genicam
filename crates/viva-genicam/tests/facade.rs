//! The `Camera` facade's standard-feature helpers, against the in-process
//! fake camera in both of its XML profiles: SFNC 2 names, and the older
//! names (`ExposureTimeAbs`, `GainRaw`, ...) the Basler acA1600 declares.
//!
//! Every assertion about a write reads the fake's registers past the
//! protocol, so the client's own account of what it wrote proves nothing.

mod common;

use std::net::Ipv4Addr;
use std::time::Duration;

use viva_fake_gige::XmlProfile;
use viva_fake_gige::registers::{
    REG_ACQ_FRAME_RATE, REG_ACQ_FRAME_RATE_ENABLE, REG_BINNING_HORIZONTAL, REG_BINNING_VERTICAL,
    REG_EXPOSURE_TIME, REG_GAIN_RAW, REG_HEIGHT, REG_OFFSET_X, REG_OFFSET_Y, REG_USER_SET_DEFAULT,
    REG_WIDTH,
};
use viva_genicam::gige::DeviceInfo;
use viva_genicam::{Camera, GenicamError, GigeRegisterIo, Roi, StreamOptions, connect_gige};

async fn open() -> Camera<GigeRegisterIo> {
    connect_gige(&DeviceInfo::from_ip(Ipv4Addr::LOCALHOST))
        .await
        .expect("open the fake")
}

async fn legacy() -> common::TestCamera {
    common::TestCamera::start_with(|b| b.profile(XmlProfile::Legacy)).await
}

fn word(bytes: Vec<u8>) -> u32 {
    u32::from_be_bytes(bytes.try_into().expect("four bytes"))
}

/// API-14: a camera with `ExposureTimeAbs` and `GainRaw` instead of the
/// SFNC 2 names is driven by the same helpers, which say which feature they
/// wrote; the gain lands on the control `GainSelector` selects.
#[tokio::test(flavor = "multi_thread")]
async fn exposure_and_gain_fall_back_to_the_names_older_cameras_use() {
    let cam = legacy().await;
    let mut camera = open().await;

    let exposure = camera.set_exposure_time(7_500.0).expect("exposure");
    assert_eq!(exposure.name, "ExposureTimeAbs");
    assert_eq!(exposure.unit.as_deref(), Some("us"));
    let stored = cam.fake().peek(REG_EXPOSURE_TIME, 8).await;
    assert_eq!(f64::from_be_bytes(stored.try_into().unwrap()), 7_500.0);

    camera.select_gain("Tap1").expect("select Tap1");
    let gain = camera.set_gain(12.0).expect("gain");
    assert_eq!((gain.name, gain.kind), ("GainRaw", "Integer"));
    assert_eq!(word(cam.fake().peek(REG_GAIN_RAW + 4, 4).await), 12);
    assert_eq!(
        word(cam.fake().peek(REG_GAIN_RAW, 4).await),
        0,
        "All untouched"
    );
}

/// The SFNC 2 names come first where a camera has them.
#[tokio::test(flavor = "multi_thread")]
async fn exposure_and_gain_use_the_sfnc_names_first() {
    let _cam = common::TestCamera::start().await;
    let mut camera = open().await;
    assert_eq!(
        camera.set_exposure_time(2_000.0).expect("exposure").name,
        "ExposureTime"
    );
    assert_eq!(camera.exposure_time().expect("read back"), 2_000.0);
    assert_eq!(camera.set_gain(6.0).expect("gain").name, "Gain");
    assert_eq!(camera.gain().expect("read back"), 6.0);
}

/// API-15: starting acquisition sets `TLParamsLocked`, which locks the
/// features that size the payload; stopping clears it. An `<Integer>` in
/// the SFNC profile, a `<Boolean>` with a `<Value>` in the legacy one.
#[tokio::test(flavor = "multi_thread")]
async fn acquisition_locks_the_transport_parameters_until_it_stops() {
    for profile in [XmlProfile::Sfnc, XmlProfile::Legacy] {
        let cam = common::TestCamera::start_with(|b| b.profile(profile)).await;
        let mut camera = open().await;

        camera.acquisition_start().expect("start");
        assert!(
            camera.set_integer("Width", 320).is_err(),
            "{profile:?}: Width is locked while acquiring"
        );
        assert_eq!(word(cam.fake().peek(REG_WIDTH, 4).await), 640);

        camera.acquisition_stop().expect("stop");
        camera.set_integer("Width", 320).expect("unlocked again");
        assert_eq!(word(cam.fake().peek(REG_WIDTH, 4).await), 320);
        drop(camera);
    }
}

/// API-14: with the `FrameStart` trigger on and its source `Software`, the
/// camera sends nothing until it is triggered, then exactly one frame.
#[tokio::test(flavor = "multi_thread")]
async fn a_software_trigger_starts_exactly_one_frame() {
    let cam = common::TestCamera::start().await;
    let mut camera = open().await;
    let mut frames = camera.open_stream(StreamOptions::new()).expect("stream");
    camera
        .enable_trigger("FrameStart", "Software")
        .expect("trigger on");
    camera.acquisition_start().expect("start");

    let quiet = Duration::from_millis(300);
    assert!(
        matches!(
            frames.next_frame_blocking(quiet),
            Err(GenicamError::Timeout(_))
        ),
        "no frame before the trigger"
    );
    camera.trigger_software().expect("trigger");
    let frame = frames
        .next_frame_blocking(Duration::from_secs(2))
        .expect("the triggered frame")
        .expect("the stream is open");
    assert_eq!(frame.width, 640);
    assert!(
        matches!(
            frames.next_frame_blocking(quiet),
            Err(GenicamError::Timeout(_))
        ),
        "one trigger, one frame"
    );
    assert_eq!(cam.fake().software_triggers().await, 1);

    camera.disable_trigger("FrameStart").expect("trigger off");
    frames
        .next_frame_blocking(Duration::from_secs(2))
        .expect("free-running again")
        .expect("the stream is open");
    camera.acquisition_stop().expect("stop");
}

/// API-14: a region the current offsets would block is still reached — the
/// offsets go to zero first, then the size, then the offsets — whichever way
/// the region moves.
#[tokio::test(flavor = "multi_thread")]
async fn a_region_of_interest_is_reached_whichever_way_it_moves() {
    let cam = common::TestCamera::start().await;
    let mut camera = open().await;
    let regions = [
        Roi {
            x: 3000,
            y: 3000,
            width: 1000,
            height: 1000,
        },
        // Width first would exceed 4096 - 3000.
        Roi {
            x: 0,
            y: 0,
            width: 4000,
            height: 4000,
        },
        // OffsetX first would exceed 4096 - 4000.
        Roi {
            x: 100,
            y: 200,
            width: 320,
            height: 240,
        },
    ];
    for roi in regions {
        camera
            .set_roi(roi)
            .unwrap_or_else(|err| panic!("{roi:?}: {err}"));
        let stored = Roi {
            x: i64::from(word(cam.fake().peek(REG_OFFSET_X, 4).await)),
            y: i64::from(word(cam.fake().peek(REG_OFFSET_Y, 4).await)),
            width: i64::from(word(cam.fake().peek(REG_WIDTH, 4).await)),
            height: i64::from(word(cam.fake().peek(REG_HEIGHT, 4).await)),
        };
        assert_eq!(stored, roi);
        assert_eq!(camera.roi().expect("read back"), roi);
    }
}

/// API-14: binning in both directions.
#[tokio::test(flavor = "multi_thread")]
async fn binning_is_written_in_both_directions() {
    let cam = common::TestCamera::start().await;
    let mut camera = open().await;
    camera.set_binning(2, 4).expect("binning");
    assert_eq!(word(cam.fake().peek(REG_BINNING_HORIZONTAL, 4).await), 2);
    assert_eq!(word(cam.fake().peek(REG_BINNING_VERTICAL, 4).await), 4);
    assert_eq!(camera.binning().expect("read back"), (2, 4));
}

/// API-14: the frame rate is switched on before it is written — the
/// feature is unavailable until then — through `AcquisitionFrameRateAbs` on
/// the legacy camera; disabling switches it off again.
#[tokio::test(flavor = "multi_thread")]
async fn the_frame_rate_is_enabled_then_written() {
    let cam = legacy().await;
    let mut camera = open().await;
    camera.disable_frame_rate().expect("disable");
    assert_eq!(word(cam.fake().peek(REG_ACQ_FRAME_RATE_ENABLE, 4).await), 0);

    let rate = camera.set_frame_rate(25.0).expect("frame rate");
    assert_eq!(rate.name, "AcquisitionFrameRateAbs");
    assert_eq!(word(cam.fake().peek(REG_ACQ_FRAME_RATE_ENABLE, 4).await), 1);
    let stored = cam.fake().peek(REG_ACQ_FRAME_RATE, 4).await;
    assert_eq!(f32::from_be_bytes(stored.try_into().unwrap()), 25.0);
    assert_eq!(camera.frame_rate().expect("read back"), 25.0);
}

/// API-14: user sets are saved and loaded by name, waiting for each
/// command to finish, and the power-up set is chosen through
/// `UserSetDefault` or, on the legacy camera, `UserSetDefaultSelector`.
#[tokio::test(flavor = "multi_thread")]
async fn user_sets_are_saved_loaded_and_chosen_for_power_up() {
    for (profile, default_name) in [
        (XmlProfile::Sfnc, "UserSetDefault"),
        (XmlProfile::Legacy, "UserSetDefaultSelector"),
    ] {
        let cam = common::TestCamera::start_with(|b| b.profile(profile)).await;
        let mut camera = open().await;
        assert_eq!(
            camera.user_sets().expect("sets"),
            ["Default", "UserSet1", "UserSet2"]
        );

        camera.set_exposure_time(1_234.0).expect("exposure");
        camera.save_user_set("UserSet2").expect("save");
        assert_eq!(cam.fake().saved_exposure(2).await, Some(1_234.0));
        if profile == XmlProfile::Sfnc {
            // Gain declares no invalidator: only forgetting every cached
            // value after the load reads what the load restored.
            camera.set_gain(6.0).expect("gain");
        }
        camera.load_user_set("Default").expect("load Default");
        assert_eq!(camera.exposure_time().expect("default exposure"), 5_000.0);
        if profile == XmlProfile::Sfnc {
            assert_eq!(camera.gain().expect("default gain"), 0.0);
        }
        camera.load_user_set("UserSet2").expect("load UserSet2");
        assert_eq!(camera.exposure_time().expect("saved exposure"), 1_234.0);

        let default = camera.set_default_user_set("UserSet1").expect("default");
        assert_eq!(default.name, default_name, "{profile:?}");
        assert_eq!(word(cam.fake().peek(REG_USER_SET_DEFAULT, 4).await), 1);
        drop(camera);
    }
}

/// API-14: the acquisition mode, by entry name.
#[tokio::test(flavor = "multi_thread")]
async fn the_acquisition_mode_is_read_and_written() {
    let _cam = common::TestCamera::start().await;
    let mut camera = open().await;
    assert_eq!(camera.acquisition_mode().expect("mode"), "Continuous");
    camera
        .set_acquisition_mode("SingleFrame")
        .expect("set mode");
    assert_eq!(camera.acquisition_mode().expect("mode"), "SingleFrame");
}

//! GA-14: saving a camera's configuration and restoring it, against the
//! in-process fake camera.
//!
//! Every assertion reads the fake's registers past the protocol: a restore is
//! proved by the device's state, not by the report the client wrote itself.
//! The fake scales its region of interest when the binning changes, so a
//! restore that writes `Width` before `BinningHorizontal` leaves a width the
//! file did not ask for.

mod common;

use std::net::Ipv4Addr;

use viva_fake_gige::{
    REG_BINNING_HORIZONTAL, REG_EXPOSURE_AUTO, REG_EXPOSURE_TIME, REG_OFFSET_X, REG_TRIGGER_MODE,
    REG_TRIGGER_SELECTOR, REG_TRIGGER_SOURCE, REG_WIDTH, TRIGGER_SOURCE_LINE1,
    TRIGGER_SOURCE_SOFTWARE,
};
use viva_genicam::gige::DeviceInfo;
use viva_genicam::{Camera, GigeRegisterIo, connect_gige};

async fn open() -> Camera<GigeRegisterIo> {
    connect_gige(&DeviceInfo::from_ip(Ipv4Addr::LOCALHOST))
        .await
        .expect("open the fake")
}

fn word(bytes: Vec<u8>) -> u32 {
    u32::from_be_bytes(bytes.try_into().expect("four bytes"))
}

fn double(bytes: Vec<u8>) -> f64 {
    f64::from_be_bytes(bytes.try_into().expect("eight bytes"))
}

/// What the device holds, read past the protocol.
async fn device_state(cam: &common::TestCamera) -> Vec<(&'static str, u64)> {
    let fake = cam.fake();
    vec![
        (
            "binning",
            u64::from(word(fake.peek(REG_BINNING_HORIZONTAL, 4).await)),
        ),
        ("width", u64::from(word(fake.peek(REG_WIDTH, 4).await))),
        (
            "offset x",
            u64::from(word(fake.peek(REG_OFFSET_X, 4).await)),
        ),
        (
            "trigger selector",
            u64::from(word(fake.peek(REG_TRIGGER_SELECTOR, 4).await)),
        ),
        (
            "frame start mode",
            u64::from(word(fake.peek(REG_TRIGGER_MODE, 4).await)),
        ),
        (
            "acquisition start mode",
            u64::from(word(fake.peek(REG_TRIGGER_MODE + 4, 4).await)),
        ),
        (
            "frame start source",
            u64::from(word(fake.peek(REG_TRIGGER_SOURCE, 4).await)),
        ),
        (
            "exposure auto",
            u64::from(word(fake.peek(REG_EXPOSURE_AUTO, 4).await)),
        ),
        (
            "exposure",
            double(fake.peek(REG_EXPOSURE_TIME, 8).await) as u64,
        ),
    ]
}

/// Save a configuration, change everything it holds, load it back: the
/// device is where it was, the selected trigger features under both entries
/// of `TriggerSelector` included.
#[tokio::test(flavor = "multi_thread")]
async fn a_saved_configuration_restores_the_camera_it_came_from() {
    let cam = common::TestCamera::start().await;
    let mut camera = open().await;
    camera.set("BinningHorizontal", "2").expect("binning");
    camera.set("Width", "200").expect("width");
    camera.set("OffsetX", "16").expect("offset");
    camera
        .set("TriggerSelector", "AcquisitionStart")
        .expect("selector");
    camera.set("TriggerMode", "On").expect("mode");
    camera
        .set("TriggerSelector", "FrameStart")
        .expect("selector");
    camera.set("TriggerSource", "Software").expect("source");
    camera.set("ExposureTime", "1234").expect("exposure");
    let saved_state = device_state(&cam).await;

    let mut file = Vec::new();
    let saved = camera.save_features(&mut file).expect("save");
    let text = String::from_utf8(file.clone()).expect("text");
    assert!(
        text.starts_with("# {05D8C294-F295-4dfb-9D01-096BD04049F4}\n"),
        "{text}"
    );
    assert!(
        text.contains("TriggerMode\t{TriggerSelector=AcquisitionStart}\tOn\n"),
        "{text}"
    );
    assert!(text.contains("# ModelName = VivaCam_Fake\n"), "{text}");
    assert!(!saved.values.is_empty());

    camera.set("BinningHorizontal", "1").expect("binning");
    camera.set("OffsetX", "0").expect("offset");
    camera.set("Width", "640").expect("width");
    camera
        .set("TriggerSelector", "AcquisitionStart")
        .expect("selector");
    camera.set("TriggerMode", "Off").expect("mode");
    camera
        .set("TriggerSelector", "FrameStart")
        .expect("selector");
    camera.set("TriggerSource", "Line1").expect("source");
    camera.set("ExposureTime", "5000").expect("exposure");
    camera.set("ExposureAuto", "Continuous").expect("auto");
    assert_ne!(device_state(&cam).await, saved_state);

    let report = camera.load_features(&mut file.as_slice()).expect("load");
    assert!(report.is_complete(), "{:?}", report.failures);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(device_state(&cam).await, saved_state);
    assert_eq!(
        word(cam.fake().peek(REG_TRIGGER_SOURCE, 4).await),
        TRIGGER_SOURCE_SOFTWARE
    );
}

/// A file — edited by hand, or from another tool — that lists `Width`
/// before the `BinningHorizontal` that rescales it, and `ExposureTime`
/// before the `ExposureAuto` that locks it. The load writes the binning and
/// the auto mode first, so the device ends where the file says.
#[tokio::test(flavor = "multi_thread")]
async fn a_load_writes_in_the_order_the_camera_needs_not_the_files() {
    let cam = common::TestCamera::start().await;
    let mut camera = open().await;
    camera.set("ExposureAuto", "Continuous").expect("auto");
    let file = "# {05D8C294-F295-4dfb-9D01-096BD04049F4}\n\
                Width\t300\n\
                ExposureTime\t2500\n\
                TriggerSource\t{TriggerSelector=FrameStart}\tSoftware\n\
                BinningHorizontal\t2\n\
                ExposureAuto\tOff\n";
    let report = camera.load_features(&mut file.as_bytes()).expect("load");
    assert!(report.is_complete(), "{:?}", report.failures);
    assert_eq!(
        report.written_on_retry, 0,
        "nothing waited for a second pass"
    );
    assert_eq!(word(cam.fake().peek(REG_BINNING_HORIZONTAL, 4).await), 2);
    assert_eq!(word(cam.fake().peek(REG_WIDTH, 4).await), 300);
    assert_eq!(double(cam.fake().peek(REG_EXPOSURE_TIME, 8).await), 2500.0);
    assert_eq!(word(cam.fake().peek(REG_EXPOSURE_AUTO, 4).await), 0);
    assert_eq!(
        word(cam.fake().peek(REG_TRIGGER_SOURCE, 4).await),
        TRIGGER_SOURCE_SOFTWARE
    );
    assert_ne!(TRIGGER_SOURCE_SOFTWARE, TRIGGER_SOURCE_LINE1);
}

/// A configuration from another model loads with a warning; a feature this
/// camera does not have, or a value out of range, is reported and the rest
/// is written.
#[tokio::test(flavor = "multi_thread")]
async fn a_load_reports_what_it_could_not_write_and_where_the_file_came_from() {
    let cam = common::TestCamera::start().await;
    let mut camera = open().await;
    let file = "# {05D8C294-F295-4dfb-9D01-096BD04049F4}\n\
                # VendorName = Acme\n\
                # ModelName = Other\n\
                NoSuchFeature\t1\n\
                OffsetX\t100000\n\
                ExposureTime\t3000\n";
    let report = camera.load_features(&mut file.as_bytes()).expect("load");
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    assert!(
        report.warnings[0].contains("Other"),
        "{:?}",
        report.warnings
    );
    let failed: Vec<&str> = report
        .failures
        .iter()
        .map(|f| f.value.feature.as_str())
        .collect();
    assert_eq!(failed, ["NoSuchFeature", "OffsetX"]);
    assert_eq!(double(cam.fake().peek(REG_EXPOSURE_TIME, 8).await), 3000.0);
    assert!(
        camera.load_features(&mut "Width 640\n".as_bytes()).is_err(),
        "a line without a tab is not a configuration"
    );
}

/// While the camera acquires, `TLParamsLocked` locks the image format: a
/// save leaves it out and says why, rather than saving a value the restore
/// could not write.
#[tokio::test(flavor = "multi_thread")]
async fn a_save_while_acquiring_says_what_the_acquisition_locks() {
    let _cam = common::TestCamera::start().await;
    let mut camera = open().await;
    camera.acquisition_start().expect("start");
    let mut file = Vec::new();
    let saved = camera.save_features(&mut file).expect("save");
    camera.acquisition_stop().expect("stop");
    let width = saved
        .omitted
        .iter()
        .find(|o| o.feature == "Width")
        .expect("Width left out");
    assert_eq!(width.reason, "locked by TLParamsLocked");
    let text = String::from_utf8(file).unwrap();
    assert!(
        text.contains("# not saved: Width: locked by TLParamsLocked\n"),
        "{text}"
    );
}

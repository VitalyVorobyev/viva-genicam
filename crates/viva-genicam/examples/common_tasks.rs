//! The everyday tasks — typed features, exposure and gain, triggers, region
//! of interest, binning, frame rate, user sets, saving and restoring a
//! configuration — against the fake GigE Vision camera, no hardware required.
//!
//! ```bash
//! cargo run -p viva-genicam --example common_tasks
//! ```
//!
//! `--legacy` serves the older feature names instead (`ExposureTimeAbs`,
//! `GainRaw`, ...), which the same calls handle.

use viva_fake_gige::{FakeCamera, XmlProfile};
use viva_genicam::gige::DeviceInfo;
use viva_genicam::{Roi, StandardFeature, StreamOptions, connect_gige};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    let profile = if std::env::args().any(|arg| arg == "--legacy") {
        XmlProfile::Legacy
    } else {
        XmlProfile::Sfnc
    };
    let _fake = FakeCamera::builder()
        .bind_ip([127, 0, 0, 1].into())
        .port(3956)
        .profile(profile)
        .build()
        .await?;
    let mut camera = connect_gige(&DeviceInfo::from_ip([127, 0, 0, 1].into())).await?;

    // ANCHOR: typed
    // Each feature as its own type; `get` and `set` remain for text.
    let width: i64 = camera.get_integer("Width")?;
    camera.set_integer("Width", width / 2)?;
    let format: String = camera.get_enum("PixelFormat")?;
    println!(
        "Width {} -> {}, {format}",
        width,
        camera.get_integer("Width")?
    );
    // ANCHOR_END: typed

    // ANCHOR: exposure_gain
    // ExposureTime, else ExposureTimeAbs, else ExposureTimeRaw: the helper
    // says which it wrote, and in which unit.
    let exposure = camera.set_exposure_time(10_000.0)?;
    println!(
        "exposure: {} = {} {}",
        exposure.name,
        camera.exposure_time()?,
        exposure.unit.as_deref().unwrap_or("(device units)")
    );
    // Gain, else GainRaw, on the control GainSelector selects where there is one.
    if camera.resolve(StandardFeature::GainSelector).is_ok() {
        camera.select_gain("All")?;
    }
    let gain = camera.set_gain(6.0)?;
    println!("gain: {} = {}", gain.name, camera.gain()?);
    // ANCHOR_END: exposure_gain

    // ANCHOR: roi_binning
    // Offsets to zero, then the size, then the offsets: the order that
    // reaches any region the sensor allows.
    camera.set_roi(Roi {
        x: 64,
        y: 32,
        width: 320,
        height: 240,
    })?;
    println!("roi: {:?}", camera.roi()?);
    camera.set_binning(1, 1)?;
    // ANCHOR_END: roi_binning

    // ANCHOR: frame_rate
    // Turns AcquisitionFrameRateEnable on first, where the camera has one.
    let rate = camera.set_frame_rate(25.0)?;
    println!("frame rate: {} = {} Hz", rate.name, camera.frame_rate()?);
    // ANCHOR_END: frame_rate

    // ANCHOR: user_sets
    // Select, execute, and wait for the camera to finish.
    camera.save_user_set("UserSet1")?;
    camera.load_user_set("Default")?;
    camera.set_default_user_set("UserSet1")?;
    // ANCHOR_END: user_sets

    let path = std::env::temp_dir().join("viva-camera.txt");
    // ANCHOR: save_load
    // Every feature the camera's XML marks streamable, each selected one
    // under every entry of its selector; the file's comments say what was
    // left out and why.
    let saved = camera.save_features(&mut std::fs::File::create(&path)?)?;
    println!("saved {} values to {}", saved.values.len(), path.display());

    // Later — on this camera or another of the same model:
    let report = camera.load_features(&mut std::fs::File::open(&path)?)?;
    for warning in &report.warnings {
        eprintln!("warning: {warning}"); // saved from another model, say
    }
    for failure in &report.failures {
        eprintln!("not written: {failure}"); // `Feature[Selector=Entry] = value: why`
    }
    // ANCHOR_END: save_load

    // ANCHOR: software_trigger
    let mut frames = camera.open_stream(StreamOptions::new())?;
    camera.enable_trigger("FrameStart", "Software")?;
    // Sets TLParamsLocked too: Width and PixelFormat are locked until the stop.
    camera.acquisition_start()?;
    for _ in 0..3 {
        camera.trigger_software()?;
        let frame = frames.next_frame().await?.ok_or("the stream ended")?;
        println!("triggered frame {}x{}", frame.width, frame.height);
    }
    camera.acquisition_stop()?;
    camera.disable_trigger("FrameStart")?;
    // ANCHOR_END: software_trigger

    Ok(())
}

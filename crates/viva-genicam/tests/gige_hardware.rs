//! A smoke suite for a real GigE Vision camera: the parts of the
//! [hardware test plan](../../../docs/hardware-test-plan.md) a test can
//! judge. Every test is ignored: it needs a camera, which CI does not have.
//!
//! ```bash
//! VIVA_GIGE_IP=192.168.2.25 VIVA_GIGE_IFACE=en10 \
//!   cargo test -p viva-genicam --test gige_hardware -- --ignored --test-threads=1
//! ```
//!
//! `VIVA_GIGE_IP` names the camera; `VIVA_GIGE_IFACE` (optional) the host
//! interface, by name or IPv4 address, else the one routed to the camera.
//! One test at a time: each opens the camera, and a camera takes one
//! controller. Each test saves the camera's configuration first and loads it
//! back at the end, and writes nothing non-volatile unless
//! `VIVA_GIGE_ALLOW_PERSISTENT=1`. A feature the camera lacks is skipped with
//! a message, not failed. Point the camera at a lit scene to see the pixel
//! range the stream tests print. Written against an Allied Vision Manta
//! G-125B.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::time::{Duration, Instant, SystemTime};

use viva_genicam::genapi::{FeatureValue, Node, RegisterIo};
use viva_genicam::gige::{self, DeviceInfo, DiscoveryOptions, GigeDevice, Iface, IfaceSelector};
use viva_genicam::{
    Access, Camera, ChunkConfig, ControlState, Frame, FrameStream, GigeRegisterIo, Roi,
    StandardFeature, StreamOptions, connect_gige_as,
};

type GigeCamera = Camera<GigeRegisterIo>;

const NEEDS: &str = "needs a GigE Vision camera: set VIVA_GIGE_IP";
const CCP: u64 = 0x0a00;

/// Names whose writes may be stored in the camera's flash, written only
/// with `VIVA_GIGE_ALLOW_PERSISTENT=1`.
const PERSISTENT: &[&str] = &[
    "Persistent",
    "UserSetDefault",
    "ConfigFilePowerUp",
    "DeviceUserID",
    "IPConfiguration",
    "UserSetSave",
    "LUTSave",
];

fn camera_ip() -> Ipv4Addr {
    std::env::var("VIVA_GIGE_IP")
        .expect(NEEDS)
        .parse()
        .expect("VIVA_GIGE_IP is an IPv4 address")
}

fn allow_persistent() -> bool {
    std::env::var("VIVA_GIGE_ALLOW_PERSISTENT").is_ok_and(|v| v == "1")
}

fn is_persistent(name: &str) -> bool {
    PERSISTENT.iter().any(|word| name.contains(word))
}

/// The host interface: `VIVA_GIGE_IFACE`, else the one routed to the camera.
fn iface() -> Iface {
    match std::env::var("VIVA_GIGE_IFACE") {
        Ok(text) => text
            .parse::<IfaceSelector>()
            .expect("VIVA_GIGE_IFACE names an interface")
            .resolve()
            .expect("VIVA_GIGE_IFACE is an interface of this host"),
        Err(_) => Iface::from_remote_ipv4(camera_ip()).expect("an interface routed to the camera"),
    }
}

async fn device() -> DeviceInfo {
    gige::discover_ip(camera_ip(), Duration::from_secs(1))
        .await
        .expect("unicast discovery")
        .unwrap_or_else(|| panic!("no camera answered at {}", camera_ip()))
}

async fn open(access: Access) -> GigeCamera {
    let info = device().await;
    connect_gige_as(&info, access)
        .await
        .unwrap_or_else(|err| panic!("open as {access:?}: {err}"))
        .0
}

/// A second application's view of the camera: a raw control channel of its
/// own, claiming nothing.
async fn bystander() -> GigeDevice {
    GigeDevice::open((camera_ip(), gige::GVCP_PORT).into())
        .await
        .expect("a second control channel")
}

/// The camera's configuration now, to load back at the end of a test.
fn snapshot(camera: &mut GigeCamera) -> String {
    let mut file = Vec::new();
    camera.save_features(&mut file).expect("save");
    String::from_utf8(file).expect("utf-8")
}

fn restore(camera: &mut GigeCamera, saved: &str) {
    let report = camera
        .load_features(&mut saved.as_bytes())
        .expect("load the saved configuration");
    for failure in &report.failures {
        println!("  not restored: {failure}");
    }
    assert!(
        report.failures.is_empty(),
        "the configuration did not load back"
    );
}

/// The value lines of a saved configuration, without its comments.
fn values(saved: &str) -> Vec<&str> {
    saved
        .lines()
        .filter(|line| !line.starts_with('#'))
        .collect()
}

/// Free-running acquisition: FrameStart's trigger off, or, on a camera whose
/// FrameStart trigger is never off (the Manta's `TriggerMode` has `On`
/// alone), its source `Freerun`.
fn free_running(camera: &mut GigeCamera) {
    if camera.set("TriggerSelector", "FrameStart").is_err() {
        return;
    }
    let offered = |camera: &GigeCamera, feature: &str| {
        camera
            .feature_state(feature)
            .ok()
            .and_then(|state| state.enum_entries)
            .unwrap_or_default()
    };
    if offered(camera, "TriggerMode").iter().any(|e| e == "Off") {
        camera.set("TriggerMode", "Off").expect("TriggerMode Off");
    } else if offered(camera, "TriggerSource")
        .iter()
        .any(|e| e == "Freerun")
    {
        camera
            .set("TriggerSource", "Freerun")
            .expect("TriggerSource Freerun");
    }
}

async fn frames(stream: &mut FrameStream, n: usize, timeout: Duration) -> Vec<Frame> {
    let mut got = Vec::with_capacity(n);
    for i in 0..n {
        match tokio::time::timeout(timeout, stream.next_frame()).await {
            Ok(Ok(Some(frame))) => got.push(frame),
            Ok(Ok(None)) => panic!("the stream ended after {i} frames"),
            Ok(Err(error)) => panic!("frame {i}: {error}"),
            Err(_) => panic!("no frame {i} within {timeout:?}"),
        }
    }
    got
}

fn open_stream(camera: &mut GigeCamera) -> FrameStream {
    camera
        .open_stream(StreamOptions::new().iface(iface()))
        .expect("open the stream")
}

/// Every feature the category tree lists, in document order.
fn features(camera: &GigeCamera) -> Vec<String> {
    let nodemap = camera.nodemap();
    let listed: std::collections::HashSet<&str> = nodemap
        .categories()
        .into_iter()
        .flat_map(|(_, children)| children.iter().map(String::as_str))
        .collect();
    nodemap
        .node_names()
        .filter(|name| listed.contains(name))
        .filter(|name| !matches!(nodemap.node(name), Some(Node::Category(_))))
        .map(str::to_string)
        .collect()
}

/// Read CCP through `device`, as another application would.
async fn ccp_seen_by(device: &mut GigeDevice) -> Result<u32, gige::GigeError> {
    device.read_register(CCP as u32).await
}

fn ccp_of(camera: &GigeCamera) -> u32 {
    let bytes = camera.transport().read(CCP, 4).expect("read CCP");
    u32::from_be_bytes(bytes.try_into().expect("a word"))
}

// ---------------------------------------------------------------------------
// A. Discovery
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GigE Vision camera: VIVA_GIGE_IP"]
async fn discovery_finds_the_camera_by_broadcast_and_by_address() {
    let unicast = device().await;
    println!(
        "unicast: {} {} {:?} {:?}, subnet {:?}",
        unicast.ip,
        unicast.mac_string(),
        unicast.manufacturer,
        unicast.model,
        unicast.subnet
    );
    let iface = iface();
    for allow in [false, true] {
        let options = DiscoveryOptions::new()
            .interface(iface.name())
            .allow_broadcast_ack(allow);
        let found = gige::discover_with(Duration::from_secs(1), &options)
            .await
            .expect("broadcast discovery");
        let ours: Vec<_> = found.iter().filter(|d| d.mac == unicast.mac).collect();
        println!(
            "broadcast on {} (allow broadcast ACK {allow}): {} answers, {} from the camera",
            iface.name(),
            found.len(),
            ours.len()
        );
        assert_eq!(ours.len(), 1, "the camera answers once");
        assert_eq!(ours[0].ip, unicast.ip);
        assert_eq!(ours[0].model, unicast.model);
    }
}

// ---------------------------------------------------------------------------
// B. Control channel and session
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GigE Vision camera: VIVA_GIGE_IP"]
async fn control_is_held_while_open_and_released_on_drop() {
    let mut other = bystander().await;
    let camera = open(Access::Control).await;
    assert_eq!(camera.transport().control_state(), ControlState::Held);
    let held = ccp_seen_by(&mut other).await.expect("CCP");
    println!("CCP while held, read by another application: {held:#x}");
    assert_ne!(held & 0x3, 0, "the camera says it is controlled");
    drop(camera);
    let released = ccp_seen_by(&mut other).await.expect("CCP");
    println!("CCP after the drop: {released:#x}");
    assert_eq!(released, 0, "released on drop");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GigE Vision camera: VIVA_GIGE_IP"]
async fn a_monitor_reads_alongside_a_controller_and_claims_nothing() {
    let mut other = bystander().await;
    let mut alone = open(Access::Monitor).await;
    assert_eq!(alone.transport().control_state(), ControlState::Monitor);
    assert_eq!(
        ccp_seen_by(&mut other).await.expect("CCP"),
        0,
        "nothing claimed"
    );
    let width = alone.get("Width").expect("a monitor reads");
    assert!(
        alone.set("Width", &width).is_err(),
        "a monitor's write is refused before the wire"
    );
    drop(alone);

    let controller = open(Access::Control).await;
    let monitor = open(Access::Monitor).await;
    assert_eq!(monitor.get("Width").expect("read"), width);
    assert_eq!(controller.transport().control_state(), ControlState::Held);
    drop(monitor);
    assert_ne!(ccp_of(&controller) & 0x3, 0, "the controller kept control");
}

/// GT-07: exclusive access is CCP `0x1`; the camera's answer to another
/// application's read is printed.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GigE Vision camera: VIVA_GIGE_IP"]
async fn exclusive_access_is_granted() {
    let camera = open(Access::Exclusive).await;
    let own = ccp_of(&camera);
    println!("CCP read by the exclusive holder: {own:#x}");
    assert_eq!(own & 0x1, 0x1, "exclusive access granted");
    let mut other = bystander().await;
    println!(
        "another application's READREG of CCP: {:?}",
        ccp_seen_by(&mut other).await
    );
    println!(
        "another application's READMEM of 0x0000: {:?}",
        other.read_mem(0, 8).await
    );
    let monitor = connect_gige_as(&device().await, Access::Monitor).await;
    println!(
        "a monitor opening meanwhile: {}",
        monitor
            .as_ref()
            .map_or_else(|e| e.to_string(), |_| "opened".into())
    );
    drop(camera);
    assert_eq!(ccp_seen_by(&mut other).await.expect("CCP"), 0, "released");
}

/// TC-38: a second application is refused control, and refused a write,
/// while the first holds it; the first keeps control.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GigE Vision camera: VIVA_GIGE_IP"]
async fn a_second_application_is_refused_while_one_holds_control() {
    let mut first = open(Access::Control).await;
    let second = connect_gige_as(&device().await, Access::Control).await;
    let refusal = second.err().expect("a second controller is refused");
    println!("second application: {refusal}");
    let mut other = bystander().await;
    let write = other.write_register(0x0938, 3000).await;
    println!("its raw write of GevHeartbeatTimeout: {write:?}");
    assert!(write.is_err(), "a write without control is refused");
    let width = first.get("Width").expect("Width");
    first.set("Width", &width).expect("the first still writes");
    assert_eq!(first.transport().control_state(), ControlState::Held);
    drop(first);
    let unclaimed = other.write_register(0x0938, 3000).await;
    println!("a write while nobody holds control: {unclaimed:?}");
}

// ---------------------------------------------------------------------------
// C. GenApi across the node map
// ---------------------------------------------------------------------------

/// Every implemented, available feature reads; registers that do not are
/// listed (a camera refuses those behind a feature it does not implement).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GigE Vision camera: VIVA_GIGE_IP"]
async fn every_readable_node_reads() {
    let camera = open(Access::Monitor).await;
    let features: std::collections::HashSet<String> = features(&camera).into_iter().collect();
    let (mut read, mut unimplemented, mut failed) = (0, 0, Vec::new());
    let mut refused_registers = Vec::new();
    for name in camera.nodemap().node_names() {
        match camera.feature_state(name) {
            Ok(state) if !state.is_implemented => unimplemented += 1,
            Ok(state) if state.value != FeatureValue::None => read += 1,
            Ok(_) => {}
            Err(error) if features.contains(name) => failed.push(format!("{name}: {error}")),
            Err(error) => refused_registers.push(format!("{name}: {error}")),
        }
    }
    println!(
        "{read} values read, {unimplemented} nodes not implemented, {} skipped by the node map",
        camera.nodemap().skipped().len()
    );
    for line in &refused_registers {
        println!("  register not read: {line}");
    }
    for line in &failed {
        println!("  FEATURE NOT READ: {line}");
    }
    assert!(failed.is_empty(), "{} features did not read", failed.len());
}

/// Every feature the camera lets us write now takes its own value back.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GigE Vision camera: VIVA_GIGE_IP"]
async fn every_writable_feature_takes_its_own_value_back() {
    let mut camera = open(Access::Control).await;
    let saved = snapshot(&mut camera);
    let (mut written, mut failed) = (0, Vec::new());
    for name in features(&camera) {
        if is_persistent(&name) && !allow_persistent() {
            println!("  skipped (may be stored in flash): {name}");
            continue;
        }
        let Ok(state) = camera.feature_state(&name) else {
            continue;
        };
        let writable = state.is_available
            && state.access == Some(viva_genicam::genapi::AccessMode::RW)
            && state.value != FeatureValue::None;
        if !writable || matches!(camera.nodemap().node(&name), Some(Node::Command(_))) {
            continue;
        }
        let before = camera.get(&name).expect("read");
        match camera.set(&name, &before).and_then(|()| camera.get(&name)) {
            Ok(after) if after == before => written += 1,
            Ok(after) => failed.push(format!("{name}: wrote {before}, read {after}")),
            Err(error) => failed.push(format!("{name}: {error}")),
        }
    }
    restore(&mut camera, &saved);
    println!("{written} features took their own value back");
    for line in &failed {
        println!("  {line}");
    }
    assert!(failed.is_empty(), "{} features did not", failed.len());
}

/// Every entry an enumeration offers is accepted and reads back; an entry it
/// declares but does not offer now is refused.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GigE Vision camera: VIVA_GIGE_IP"]
async fn every_enumeration_entry_is_accepted() {
    let mut camera = open(Access::Control).await;
    let saved = snapshot(&mut camera);
    let (mut accepted, mut refused, mut failed) = (0, 0, Vec::new());
    for name in features(&camera) {
        if !matches!(camera.nodemap().node(&name), Some(Node::Enum(_))) {
            continue;
        }
        if is_persistent(&name) && !allow_persistent() {
            println!("  skipped (may be stored in flash): {name}");
            continue;
        }
        let Ok(state) = camera.feature_state(&name) else {
            continue;
        };
        if !state.is_available || state.access != Some(viva_genicam::genapi::AccessMode::RW) {
            continue;
        }
        let original = camera.get_enum(&name).expect("read");
        let offered = state.enum_entries.unwrap_or_default();
        for entry in &offered {
            match camera
                .set_enum(&name, entry)
                .and_then(|()| camera.get_enum(&name))
            {
                Ok(now) if now == *entry => accepted += 1,
                Ok(now) => failed.push(format!("{name}={entry}: reads {now}")),
                Err(error) => failed.push(format!("{name}={entry}: {error}")),
            }
        }
        if let Err(error) = camera.set_enum(&name, &original) {
            failed.push(format!("{name}: putting {original} back: {error}"));
        }
        let declared = camera.enum_entries(&name).unwrap_or_default();
        if let Some(missing) = declared.iter().find(|e| !offered.contains(e)) {
            match camera.set_enum(&name, missing) {
                Err(_) => refused += 1,
                Ok(()) => {
                    failed.push(format!("{name}={missing}: accepted, though not offered"));
                    let _ = camera.set_enum(&name, &original);
                }
            }
        }
    }
    restore(&mut camera, &saved);
    println!("{accepted} entries accepted, {refused} not offered and refused");
    for line in &failed {
        println!("  {line}");
    }
    assert!(failed.is_empty(), "{} entries misbehaved", failed.len());
}

/// GA-44: a command reports done. `UserSetLoad` loads the set the camera
/// powers up with, and the configuration is loaded back after.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GigE Vision camera: VIVA_GIGE_IP"]
async fn commands_report_done() {
    let mut camera = open(Access::Control).await;
    let saved = snapshot(&mut camera);
    let power_up = camera.get("UserSetDefaultSelector").ok();
    let selector = camera.get("UserSetSelector").ok();
    for name in [
        "TriggerSoftware",
        "GevTimestampControlLatch",
        "TimestampLatch",
        "UserSetLoad",
    ] {
        let Ok(state) = camera.feature_state(name) else {
            println!("{name}: not in this camera's XML");
            continue;
        };
        if !state.is_implemented || !state.is_available {
            println!("{name}: not implemented or not available");
            continue;
        }
        if name == "UserSetLoad" {
            match &power_up {
                Some(set) if camera.set("UserSetSelector", set).is_ok() => {}
                _ => {
                    println!("UserSetLoad: no power-up set to load");
                    continue;
                }
            }
        }
        let started = Instant::now();
        camera
            .execute_and_wait(name, Duration::from_secs(5))
            .await
            .unwrap_or_else(|err| panic!("{name}: {err}"));
        println!("{name}: done after {:?}", started.elapsed());
    }
    if let Some(selector) = selector {
        let _ = camera.set("UserSetSelector", &selector);
    }
    restore(&mut camera, &saved);
}

// ---------------------------------------------------------------------------
// D. Standard-feature helpers
// ---------------------------------------------------------------------------

/// API-19: the helpers resolve the names this camera declares, and a value
/// written through them reads back within the feature's resolution.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GigE Vision camera: VIVA_GIGE_IP"]
async fn the_standard_helpers_resolve_and_write() {
    let mut camera = open(Access::Control).await;
    let saved = snapshot(&mut camera);
    for feature in [
        StandardFeature::ExposureTime,
        StandardFeature::Gain,
        StandardFeature::FrameRate,
        StandardFeature::Width,
        StandardFeature::BinningHorizontal,
        StandardFeature::UserSetSelector,
    ] {
        match camera.resolve(feature) {
            Ok(resolved) => println!(
                "{}: {} ({}, unit {:?})",
                feature.description(),
                resolved.name,
                resolved.kind,
                resolved.unit
            ),
            Err(error) => println!("{}: {error}", feature.description()),
        }
    }

    let exposure = camera.exposure_time().expect("exposure time");
    for target in [exposure * 2.0 + 100.0, 1234.5, exposure] {
        let resolved = camera.set_exposure_time(target).expect("set exposure");
        let now = camera.exposure_time().expect("read exposure");
        println!("{} {target} -> reads {now}", resolved.name);
        assert!((now - target).abs() <= 1.0, "{now} for {target}");
    }
    match camera.gain() {
        Ok(gain) => {
            let target = gain + 1.0;
            camera.set_gain(target).expect("set gain");
            let now = camera.gain().expect("gain");
            println!("gain {gain} -> {target} -> reads {now}");
            camera.set_gain(gain).expect("put back");
        }
        Err(error) => println!("gain: {error}"),
    }
    println!("frame rate: {:?}", camera.frame_rate());
    println!("user sets: {:?}", camera.user_sets());
    restore(&mut camera, &saved);
}

/// The region of interest shrinks and grows back, and binning and
/// decimation change the image size; `PayloadSize` follows each.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GigE Vision camera: VIVA_GIGE_IP"]
async fn geometry_follows_roi_binning_and_decimation() {
    let mut camera = open(Access::Control).await;
    let saved = snapshot(&mut camera);
    let full = camera.roi().expect("roi");
    let payload = |camera: &GigeCamera| camera.get_integer("PayloadSize").expect("PayloadSize");
    let full_payload = payload(&camera);
    let half = Roi {
        x: (full.width / 4) & !7,
        y: (full.height / 4) & !7,
        width: (full.width / 2) & !7,
        height: (full.height / 2) & !7,
    };
    camera.set_roi(half).expect("shrink");
    assert_eq!(camera.roi().expect("roi"), half);
    println!(
        "ROI {full:?} -> {half:?}: PayloadSize {full_payload} -> {}",
        payload(&camera)
    );
    assert!(payload(&camera) < full_payload);
    camera.set_roi(full).expect("grow back");
    assert_eq!(camera.roi().expect("roi"), full);
    assert_eq!(payload(&camera), full_payload);

    match camera.set_binning(2, 2) {
        Ok(()) => {
            let binned = camera.roi().expect("roi");
            println!(
                "binning 2x2: {}x{}, PayloadSize {}",
                binned.width,
                binned.height,
                payload(&camera)
            );
            assert_eq!(camera.binning().expect("binning"), (2, 2));
            assert!(binned.width <= full.width / 2 + 1);
            camera.set_binning(1, 1).expect("binning back");
        }
        Err(error) => println!("binning: {error}"),
    }
    restore(&mut camera, &saved);
    for name in ["DecimationHorizontal", "DecimationVertical"] {
        let Ok(before) = camera.get_integer(name) else {
            println!("{name}: not on this camera");
            continue;
        };
        let max = |c: &GigeCamera| {
            c.get_integer("WidthMax").unwrap_or(0) * c.get_integer("HeightMax").unwrap_or(0)
        };
        let area = max(&camera);
        match camera.set_integer(name, 2) {
            Ok(()) => {
                println!("{name} 2: WidthMax x HeightMax {area} -> {}", max(&camera));
                assert!(max(&camera) < area);
                camera.set_integer(name, before).expect("put back");
            }
            Err(error) => println!("{name} 2: {error}"),
        }
    }
    restore(&mut camera, &saved);
}

/// One frame per software trigger, twenty at 10 Hz, and none untriggered.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GigE Vision camera: VIVA_GIGE_IP"]
async fn a_software_trigger_takes_one_frame() {
    let mut camera = open(Access::Control).await;
    let saved = snapshot(&mut camera);
    if let Err(error) = camera.enable_trigger("FrameStart", "Software") {
        println!("software trigger: {error}");
        restore(&mut camera, &saved);
        return;
    }
    let mut stream = open_stream(&mut camera);
    camera.acquisition_start().expect("start");
    let mut ids = Vec::new();
    for _ in 0..20 {
        let triggered = Instant::now();
        camera.trigger_software().expect("trigger");
        ids.push(frames(&mut stream, 1, Duration::from_secs(3)).await[0].block_id);
        if let Some(rest) = Duration::from_millis(100).checked_sub(triggered.elapsed()) {
            tokio::time::sleep(rest).await;
        }
    }
    let extra = tokio::time::timeout(Duration::from_millis(500), stream.next_frame()).await;
    camera.acquisition_stop().expect("stop");
    let stats = stream.stats();
    drop(stream);
    camera.disable_trigger("FrameStart").expect("trigger off");
    restore(&mut camera, &saved);
    println!("twenty triggers at 10 Hz, blocks {ids:?}");
    assert!(extra.is_err(), "a frame nobody triggered");
    for pair in ids.windows(2) {
        assert_eq!(pair[1], pair[0] + 1, "one frame per trigger");
    }
    assert_eq!((stats.frames_incomplete, stats.frames_missed), (0, 0));
}

// ---------------------------------------------------------------------------
// E. Streaming
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GigE Vision camera: VIVA_GIGE_IP"]
async fn a_stream_delivers_frames_in_every_pixel_format() {
    let mut camera = open(Access::Control).await;
    let saved = snapshot(&mut camera);
    free_running(&mut camera);
    let formats = camera
        .feature_state("PixelFormat")
        .expect("PixelFormat")
        .enum_entries
        .unwrap_or_default();
    for format in formats {
        camera.set("PixelFormat", &format).expect("set the format");
        let payload = camera.get_integer("PayloadSize").expect("PayloadSize") as usize;
        let mut stream = open_stream(&mut camera);
        camera.acquisition_start().expect("start");
        let got = frames(&mut stream, 3, Duration::from_secs(5)).await;
        camera.acquisition_stop().expect("stop");
        let stats = stream.stats();
        drop(stream);
        let frame = &got[0];
        let (min, max) = (frame.payload.iter().min(), frame.payload.iter().max());
        println!(
            "{format}: {}x{} {:?}, {} bytes of {payload}, stride {:?}, bytes {min:?}..{max:?}, \
             blocks {}..{}, incomplete {}",
            frame.width,
            frame.height,
            frame.pixel_format,
            frame.payload.len(),
            frame.row_stride(),
            got[0].block_id,
            got[2].block_id,
            stats.frames_incomplete
        );
        for frame in &got {
            assert!(frame.is_complete(), "{format}: an incomplete frame");
            assert_eq!(
                frame.pixel_format.to_string(),
                format,
                "the leader's format"
            );
            assert!(frame.payload.len() <= payload, "{format}: payload size");
        }
    }
    restore(&mut camera, &saved);
}

/// Acquisition stops and starts again a hundred times in one stream (SR-23);
/// every frame of every round arrives complete, and no block id is skipped.
/// The block ids across a restart and the resend counters are printed: the
/// Manta G-125B's ids run on across a restart rather than starting again
/// at 1.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GigE Vision camera: VIVA_GIGE_IP"]
async fn a_stream_survives_acquisition_restarts() {
    let mut camera = open(Access::Control).await;
    let saved = snapshot(&mut camera);
    free_running(&mut camera);
    let mut stream = open_stream(&mut camera);
    println!("packet resend enabled: {}", stream.resend_enabled());
    let mut firsts = Vec::new();
    let mut all = Vec::new();
    for _ in 0..100 {
        camera.acquisition_start().expect("start");
        let got = frames(&mut stream, 2, Duration::from_secs(5)).await;
        camera.acquisition_stop().expect("stop");
        firsts.push(got[0].block_id);
        all.extend(got.iter().map(|f| f.block_id));
        assert!(got.iter().all(Frame::is_complete));
        // What the camera sent before it stopped.
        while let Ok(Ok(Some(frame))) =
            tokio::time::timeout(Duration::from_millis(250), stream.next_frame()).await
        {
            assert!(frame.is_complete());
            all.push(frame.block_id);
        }
    }
    let stats = stream.stats();
    println!("first block of each round: {firsts:?}");
    println!(
        "{} frames; complete {}, incomplete {}, missed {}, resend requested {}, \
         recovered {}, unavailable {}",
        all.len(),
        stats.frames_complete,
        stats.frames_incomplete,
        stats.frames_missed,
        stats.resend_requested,
        stats.resend_recovered,
        stats.resend_unavailable
    );
    drop(stream);
    restore(&mut camera, &saved);
    assert_eq!((stats.frames_incomplete, stats.frames_missed), (0, 0));
    for pair in all.windows(2) {
        assert!(
            pair[1] == pair[0] + 1 || pair[1] == 1,
            "block {} after {}",
            pair[1],
            pair[0]
        );
    }
}

/// A packet asked for again comes back (SR-24, SR-25): a resend request for
/// a block just sent, from the stream's own socket, is answered with the
/// packets; one for a block the camera no longer holds is answered as the
/// camera answers it, which is printed. The Manta G-125B resent with status
/// `0x0000` and answered each packet it no longer held with a header-only
/// `0x800C`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GigE Vision camera: VIVA_GIGE_IP"]
async fn a_resend_request_from_the_stream_socket_is_answered() {
    use viva_genicam::gige::consts::{
        STREAM_CHANNEL_BASE, STREAM_DESTINATION_ADDRESS, STREAM_DESTINATION_PORT,
    };
    let mut camera = open(Access::Control).await;
    let caps = camera
        .transport()
        .read(viva_genicam::gige::consts::GVCP_CAPABILITY, 4)
        .expect("GVCP Capability");
    if caps[3] & 0x4 == 0 {
        println!("GVCP Capability {caps:02x?}: no packet resend");
        return;
    }
    let saved = snapshot(&mut camera);
    free_running(&mut camera);
    let local = iface().ipv4().expect("the interface has an IPv4 address");
    let socket = tokio::net::UdpSocket::bind((local, 0)).await.expect("bind");
    let port = socket.local_addr().expect("address").port();
    // The stream channel pointed at a socket of the test's own.
    let io = camera.transport();
    io.write(
        STREAM_CHANNEL_BASE + STREAM_DESTINATION_ADDRESS,
        &local.octets(),
    )
    .expect("GevSCDA");
    io.write(
        STREAM_CHANNEL_BASE + STREAM_DESTINATION_PORT,
        &u32::from(port).to_be_bytes(),
    )
    .expect("GevSCPHostPort");
    camera.acquisition_start().expect("start");
    // Watch the stream for a second: the packets of each block.
    let mut seen: BTreeMap<u16, u32> = BTreeMap::new();
    let mut buf = vec![0u8; 16_384];
    let deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < deadline {
        if let Ok(Ok(len)) =
            tokio::time::timeout(Duration::from_millis(100), socket.recv(&mut buf)).await
            && len >= 8
        {
            let block = u16::from_be_bytes([buf[2], buf[3]]);
            let packet = u32::from_be_bytes([0, buf[5], buf[6], buf[7]]);
            let last = seen.entry(block).or_default();
            *last = (*last).max(packet);
        }
    }
    let blocks: Vec<u16> = seen.keys().copied().collect();
    assert!(blocks.len() >= 3, "blocks streamed: {blocks:?}");
    let camera_gvcp = std::net::SocketAddr::from((camera_ip(), gige::GVCP_PORT));
    let ask = |block: u16| gige::encode_packet_resend(0x7001, 0, u64::from(block), 2, 3, false);
    async fn answers(
        socket: &tokio::net::UdpSocket,
        block: u16,
        buf: &mut [u8],
    ) -> Vec<(u32, u16, usize)> {
        let mut got = Vec::new();
        let deadline = Instant::now() + Duration::from_millis(300);
        while Instant::now() < deadline {
            if let Ok(Ok(len)) =
                tokio::time::timeout(Duration::from_millis(20), socket.recv(buf)).await
                && len >= 8
                && u16::from_be_bytes([buf[2], buf[3]]) == block
            {
                let status = u16::from_be_bytes([buf[0], buf[1]]);
                let packet = u32::from_be_bytes([0, buf[5], buf[6], buf[7]]);
                got.push((packet, status, len));
            }
        }
        got
    }
    let recent = blocks[blocks.len() - 2];
    socket
        .send_to(&ask(recent), camera_gvcp)
        .await
        .expect("send the request");
    let resent = answers(&socket, recent, &mut buf).await;
    let oldest = blocks[0];
    socket
        .send_to(&ask(oldest), camera_gvcp)
        .await
        .expect("send the request");
    let old = answers(&socket, oldest, &mut buf).await;
    camera.acquisition_stop().expect("stop");
    camera
        .transport()
        .write(STREAM_CHANNEL_BASE + STREAM_DESTINATION_PORT, &[0; 4])
        .expect("GevSCPHostPort");
    restore(&mut camera, &saved);
    println!("block {recent} (just sent), packets 2-3: (packet, status, bytes) {resent:x?}");
    println!("block {oldest} (a second old), packets 2-3: {old:x?}");
    let resent: Vec<u32> = resent.iter().map(|(packet, ..)| *packet).collect();
    assert!(
        resent.contains(&2) && resent.contains(&3),
        "the packets asked for came back"
    );
}

/// An odd width and an odd pixel count in a bit-packed format (PX-05): the
/// frames are `PayloadSize` long and convert. The leader's `padding_x` and
/// the size are printed: the Manta G-125B declared no padding and sent
/// ⌈pixels × 12 / 8⌉ bytes.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GigE Vision camera: VIVA_GIGE_IP"]
async fn an_odd_width_in_a_packed_format_converts() {
    let mut camera = open(Access::Control).await;
    let offered = camera
        .feature_state("PixelFormat")
        .ok()
        .and_then(|state| state.enum_entries)
        .unwrap_or_default();
    let packed: Vec<String> = offered
        .into_iter()
        .filter(|f| f.ends_with("Packed") || f.ends_with('p'))
        .collect();
    if packed.is_empty() {
        println!("no packed pixel format offered");
        return;
    }
    let saved = snapshot(&mut camera);
    free_running(&mut camera);
    let width = camera.get_integer("Width").expect("Width");
    let height = camera.get_integer("Height").expect("Height");
    let (width, height) = (width - 1 + width % 2, height - 1 + height % 2);
    camera.set("Width", &width.to_string()).expect("odd width");
    camera
        .set("Height", &height.to_string())
        .expect("odd height");
    for format in packed {
        camera.set("PixelFormat", &format).expect("format");
        let payload = camera.get_integer("PayloadSize").expect("PayloadSize") as usize;
        let mut stream = open_stream(&mut camera);
        camera.acquisition_start().expect("start");
        let got = frames(&mut stream, 2, Duration::from_secs(5)).await;
        camera.acquisition_stop().expect("stop");
        drop(stream);
        let frame = &got[0];
        let converted = frame.to_rgb8();
        println!(
            "{format} at {width}x{height}: PayloadSize {payload}, frame {} bytes, padding_x {}, \
             converts: {}",
            frame.payload.len(),
            frame.padding_x,
            converted
                .as_ref()
                .map_or_else(|e| e.to_string(), |_| "yes".into())
        );
        assert!(frame.is_complete());
        assert_eq!(
            frame.payload.len(),
            payload,
            "{format}: the frame is PayloadSize"
        );
        assert!(converted.is_ok(), "{format}: the frame converts");
    }
    restore(&mut camera, &saved);
}

// ---------------------------------------------------------------------------
// G. Chunks, H. events, J. time
// ---------------------------------------------------------------------------

/// TC-37: with chunk mode on, frames carry chunks that parse; the chunk
/// ids, the payload type and what `chunk_features` reads are printed.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GigE Vision camera: VIVA_GIGE_IP"]
async fn chunk_mode_puts_chunks_on_frames() {
    let mut camera = open(Access::Control).await;
    if camera.nodemap().node("ChunkModeActive").is_none() {
        println!("ChunkModeActive: not on this camera");
        return;
    }
    let saved = snapshot(&mut camera);
    free_running(&mut camera);
    let selectors: Vec<String> = camera
        .enum_entries("ChunkSelector")
        .map(|entries| entries.into_iter().take(1).collect())
        .unwrap_or_default();
    camera
        .configure_chunks(&ChunkConfig {
            selectors: selectors.clone(),
            active: true,
        })
        .expect("chunk mode on");
    let mut stream = open_stream(&mut camera);
    camera.acquisition_start().expect("start");
    let got = frames(&mut stream, 2, Duration::from_secs(5)).await;
    camera.acquisition_stop().expect("stop");
    drop(stream);
    for frame in &got {
        let ids = frame
            .chunks
            .as_ref()
            .map(|chunks| chunks.ids().collect::<Vec<_>>());
        println!(
            "block {}: payload type {:#06x}, {} image bytes, chunk ids {ids:x?}",
            frame.block_id,
            frame.payload_type,
            frame.payload.len()
        );
        for (name, value) in camera.chunk_features(frame) {
            println!("  {name} = {value:?}");
        }
        assert!(frame.chunks.is_some(), "the frame carries chunks");
    }
    camera
        .configure_chunks(&ChunkConfig {
            selectors,
            active: false,
        })
        .expect("chunk mode off");
    restore(&mut camera, &saved);
}

/// Events arrive on the message channel while the camera streams.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GigE Vision camera: VIVA_GIGE_IP"]
async fn events_arrive_while_streaming() {
    let mut camera = open(Access::Control).await;
    let saved = snapshot(&mut camera);
    free_running(&mut camera);
    let offered = camera.enum_entries("EventSelector").unwrap_or_default();
    let Some(event) = ["ExposureEnd", "FrameStart", "AcquisitionStart"]
        .into_iter()
        .find(|e| offered.iter().any(|o| o == e))
    else {
        println!("EventSelector offers none of ExposureEnd, FrameStart, AcquisitionStart");
        restore(&mut camera, &saved);
        return;
    };
    let local = iface().ipv4().expect("the interface has an IPv4 address");
    let port = 10_021;
    camera
        .configure_events(local, port, &[event])
        .await
        .expect("configure events");
    let events = camera
        .open_event_stream(local, port)
        .await
        .expect("event stream");
    let mut stream = open_stream(&mut camera);
    camera.acquisition_start().expect("start");
    let got = frames(&mut stream, 5, Duration::from_secs(5)).await;
    let mut received = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(3);
    while received.len() < 3 && Instant::now() < deadline {
        if let Ok(Ok(event)) = tokio::time::timeout(Duration::from_millis(500), events.next()).await
        {
            received.push(event);
        }
    }
    camera.acquisition_stop().expect("stop");
    drop(stream);
    for event in &received {
        println!(
            "event {:#06x}: block {}, ticks {}, {} data bytes; features {:?}",
            event.id,
            event.block_id,
            event.ts_dev,
            event.data.len(),
            camera.event_features(event)
        );
    }
    println!(
        "frames {:?}",
        got.iter()
            .map(|f| (f.block_id, f.ts_dev))
            .collect::<Vec<_>>()
    );
    for entry in offered {
        let _ = camera.set("EventSelector", &entry);
        let _ = camera.set("EventNotification", "Off");
    }
    restore(&mut camera, &saved);
    assert!(
        !received.is_empty(),
        "{event} events for the frames streamed"
    );
    let mut seen: Vec<_> = received.iter().map(|e| (e.id, e.block_id)).collect();
    seen.dedup();
    assert_eq!(seen.len(), received.len(), "an event delivered twice");
}

/// TC-42: an application that reads its events a second late sees each
/// once, and the camera sent each once: the message channel acknowledges as
/// messages arrive. Acknowledged at the read instead, the Manta G-125B sent
/// one event twelve times to such a reader.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GigE Vision camera: VIVA_GIGE_IP"]
async fn a_late_event_reader_sees_each_event_once() {
    let mut camera = open(Access::Control).await;
    let saved = snapshot(&mut camera);
    free_running(&mut camera);
    let offered = camera.enum_entries("EventSelector").unwrap_or_default();
    let Some(event) = ["ExposureEnd", "FrameStart"]
        .into_iter()
        .find(|e| offered.iter().any(|o| o == e))
    else {
        println!("EventSelector offers neither ExposureEnd nor FrameStart");
        restore(&mut camera, &saved);
        return;
    };
    let local = iface().ipv4().expect("the interface has an IPv4 address");
    let port = 10_022;
    camera
        .configure_events(local, port, &[event])
        .await
        .expect("configure events");
    let events = camera
        .open_event_stream(local, port)
        .await
        .expect("event stream");
    let mut stream = open_stream(&mut camera);
    camera.acquisition_start().expect("start");
    let got = frames(&mut stream, 5, Duration::from_secs(5)).await;
    camera.acquisition_stop().expect("stop");
    drop(stream);
    tokio::time::sleep(Duration::from_secs(1)).await;
    let mut received = Vec::new();
    while let Ok(Ok(event)) = tokio::time::timeout(Duration::from_millis(500), events.next()).await
    {
        received.push(event.block_id);
    }
    let stats = events.stats();
    let _ = camera.set("EventSelector", event);
    let _ = camera.set("EventNotification", "Off");
    restore(&mut camera, &saved);
    println!(
        "{event} for blocks {received:?} (frames {:?}); {stats:?}",
        got.iter().map(|f| f.block_id).collect::<Vec<_>>()
    );
    assert!(!received.is_empty(), "events arrived");
    let mut distinct = received.clone();
    distinct.dedup();
    assert_eq!(distinct, received, "an event delivered twice");
    assert_eq!(stats.retransmissions, 0, "the camera resent a message");
}

/// An action command gets an answer or silence within the timeout, never a
/// hang; what came back is printed. The Manta G-125B, without the action
/// capability, answered a unicast one with `NOT_IMPLEMENTED` and a
/// broadcast one with silence.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GigE Vision camera: VIVA_GIGE_IP"]
async fn an_action_command_is_answered_or_refused() {
    let params = gige::ActionParams {
        device_key: 0,
        group_key: 0,
        group_mask: 0,
        scheduled_time: None,
    };
    for destination in [
        std::net::IpAddr::V4(camera_ip()),
        Ipv4Addr::BROADCAST.into(),
    ] {
        let started = Instant::now();
        let summary = gige::send_action((destination, gige::GVCP_PORT).into(), &params, 300)
            .await
            .expect("send the action command");
        println!(
            "to {destination}: {} acknowledged, refused by {:?}, in {:?}",
            summary.acks,
            summary.refusals,
            started.elapsed()
        );
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}

/// The device clock restarts near zero on `time_reset`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GigE Vision camera: VIVA_GIGE_IP"]
async fn time_reset_restarts_the_device_clock() {
    let mut camera = open(Access::Control).await;
    let Some(latch) = ["GevTimestampControlLatch", "TimestampLatch"]
        .into_iter()
        .find(|name| camera.nodemap().node(name).is_some())
    else {
        println!("no timestamp latch");
        return;
    };
    let Some(value) = ["GevTimestampValue", "TimestampLatchValue"]
        .into_iter()
        .find(|name| camera.nodemap().node(name).is_some())
    else {
        println!("no timestamp value");
        return;
    };
    let read = |camera: &mut GigeCamera| {
        camera.execute_command(latch).expect("latch");
        camera.get_integer(value).expect("timestamp") as u64
    };
    let hz = camera
        .get_integer("GevTimestampTickFrequency")
        .unwrap_or(1_000_000_000) as f64;
    let before = read(&mut camera);
    camera.time_reset().expect("time_reset");
    let after = read(&mut camera);
    println!(
        "device clock {:.3} s before the reset, {:.3} ms after it",
        before as f64 / hz,
        after as f64 / hz * 1e3
    );
    assert!((after as f64 / hz) < 1.0, "the clock restarted");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GigE Vision camera: VIVA_GIGE_IP"]
async fn a_fitted_clock_gives_frames_host_time() {
    let mut camera = open(Access::Control).await;
    let saved = snapshot(&mut camera);
    free_running(&mut camera);
    camera.time_calibrate(8, 10).await.expect("time_calibrate");
    let mut stream = open_stream(&mut camera);
    camera.acquisition_start().expect("start");
    let got = frames(&mut stream, 3, Duration::from_secs(5)).await;
    camera.acquisition_stop().expect("stop");
    drop(stream);
    restore(&mut camera, &saved);
    for frame in got {
        let host = frame
            .ts_host
            .or_else(|| frame.ts_dev.and_then(|ts| camera.map_dev_ts(ts)))
            .expect("host time");
        let off = SystemTime::now()
            .duration_since(host)
            .unwrap_or_else(|e| e.duration());
        println!("block {}: host time {off:?} away", frame.block_id);
        assert!(off < Duration::from_secs(2));
    }
}

// ---------------------------------------------------------------------------
// K. Configuration files
// ---------------------------------------------------------------------------

/// Save, change a feature, load, save again: the two files hold the same
/// values, and the load reports no failure.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GigE Vision camera: VIVA_GIGE_IP"]
async fn a_saved_configuration_loads_back() {
    let mut camera = open(Access::Control).await;
    let first = snapshot(&mut camera);
    let exposure = camera.exposure_time().expect("exposure");
    camera
        .set_exposure_time(exposure * 2.0 + 100.0)
        .expect("change the exposure");
    restore(&mut camera, &first);
    let second = snapshot(&mut camera);
    let differ: BTreeMap<usize, (&str, &str)> = values(&first)
        .into_iter()
        .zip(values(&second))
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .collect();
    println!(
        "{} values saved; {} differ after the round trip",
        values(&first).len(),
        differ.len()
    );
    for (a, b) in differ.values() {
        println!("  {a}  ->  {b}");
    }
    assert_eq!(values(&first), values(&second));
}

/// Non-volatile state, written back unchanged: only with
/// `VIVA_GIGE_ALLOW_PERSISTENT=1`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GigE Vision camera: VIVA_GIGE_IP"]
// The device lock is held across the register I/O on purpose: nothing else
// in this test uses the camera meanwhile, and the keepalive waits for it.
#[allow(clippy::await_holding_lock)]
async fn persistent_settings_take_their_own_value_back() {
    if !allow_persistent() {
        println!("skipped: set VIVA_GIGE_ALLOW_PERSISTENT=1 to write non-volatile state");
        return;
    }
    let mut camera = open(Access::Control).await;
    match camera.get("UserSetDefaultSelector") {
        Ok(set) => {
            let resolved = camera.set_default_user_set(&set).expect("power-up set");
            assert_eq!(camera.get(resolved.name).expect("read"), set);
            println!("{} = {set}, written back", resolved.name);
        }
        Err(error) => println!("power-up user set: {error}"),
    }
    let persistent = {
        let mut device = camera.transport().lock_device().expect("device");
        let device: &mut GigeDevice = &mut device;
        let (ip, subnet, gateway) = device.read_persistent_ip().await.expect("read");
        device
            .write_persistent_ip(ip, subnet, gateway)
            .await
            .expect("write back");
        device.read_persistent_ip().await.expect("read again") == (ip, subnet, gateway)
    };
    assert!(persistent, "the persistent IP reads back");
}

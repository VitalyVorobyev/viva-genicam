//! CL-01: the field-engineer commands — `features`, `describe`, `dump`,
//! `save`/`load`, raw registers, `action`, and the USB3 Vision `execute` and
//! `bench` — against the in-process fake cameras.
//!
//! What a command writes is checked in the fake's own state, past the
//! protocol; what it reads, against the fake's own constants.

use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use viva_camctl::{cmd_action, cmd_execute, cmd_inspect, cmd_registers, cmd_usb};
use viva_fake_gige::FakeCamera;
use viva_fake_gige::registers::{HEARTBEAT_TIMEOUT, REG_BLACK_LEVEL, REG_WIDTH};
use viva_fake_gige::{FAKE_DEVICE_KEY, FAKE_GROUP_KEY, FAKE_GROUP_MASK};
use viva_fake_u3v::FakeU3vTransport;
use viva_genicam::gige::action::ActionParams;
use viva_genicam::gige::gvcp::GigeDevice;
use viva_genicam::gige::{DeviceInfo, GVCP_PORT};
use viva_genicam::{Camera, GigeRegisterIo, U3vRegisterIo, connect_gige, open_u3v_device};
use viva_u3v::device::U3vDevice;

/// One fake on UDP 3956 at a time.
static PORT: Mutex<()> = Mutex::new(());

/// A fake GigE Vision camera on its own runtime, as a camera runs on its own
/// hardware, so the CLI can build the runtime it always builds.
struct Fake {
    runtime: tokio::runtime::Runtime,
    camera: Option<FakeCamera>,
    _port: MutexGuard<'static, ()>,
}

impl Fake {
    fn start() -> Self {
        let port = PORT.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let runtime = tokio::runtime::Runtime::new().expect("runtime for the fake");
        let camera = runtime.block_on(async {
            loop {
                match FakeCamera::builder()
                    .bind_ip(Ipv4Addr::LOCALHOST)
                    .port(GVCP_PORT)
                    .width(640)
                    .height(480)
                    .build()
                    .await
                {
                    Ok(camera) => break camera,
                    Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                        tokio::time::sleep(Duration::from_millis(100)).await
                    }
                    Err(e) => panic!("start the fake: {e}"),
                }
            }
        });
        Fake {
            runtime,
            camera: Some(camera),
            _port: port,
        }
    }

    fn peek_word(&self, addr: u64) -> u32 {
        let fake = self.camera.as_ref().unwrap();
        let bytes = self.runtime.block_on(fake.peek(addr, 4));
        u32::from_be_bytes(bytes.try_into().unwrap())
    }

    fn controller(&self) -> Option<std::net::SocketAddr> {
        let fake = self.camera.as_ref().unwrap();
        self.runtime.block_on(fake.controller())
    }

    fn camera(&self) -> Camera<GigeRegisterIo> {
        self.runtime
            .block_on(connect_gige(&DeviceInfo::from_ip(Ipv4Addr::LOCALHOST)))
            .expect("open the fake")
    }

    fn control(&self) -> GigeDevice {
        self.runtime
            .block_on(GigeDevice::open((Ipv4Addr::LOCALHOST, GVCP_PORT).into()))
            .expect("open the control channel")
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        if let Some(camera) = self.camera.take() {
            self.runtime.block_on(camera.stop());
        }
    }
}

fn find<'a>(nodes: &'a [cmd_inspect::TreeNode], name: &str) -> Option<&'a cmd_inspect::TreeNode> {
    nodes.iter().find_map(|node| {
        if node.name == name {
            Some(node)
        } else {
            find(&node.children, name)
        }
    })
}

#[test]
fn features_prints_the_category_tree_with_each_value() {
    let fake = Fake::start();
    let camera = fake.camera();
    let tree = cmd_inspect::feature_tree(&camera, None, false);
    let width = find(&tree, "Width").expect("Width in the tree");
    assert_eq!(width.kind, "Integer");
    assert_eq!(width.value.as_deref(), Some("640"));
    assert_eq!(width.access.as_deref(), Some("RW"));
    let start = find(&tree, "AcquisitionStart").expect("a command");
    assert_eq!(
        (start.value.as_deref(), start.access.as_deref()),
        (None, Some("WO"))
    );
    assert!(find(&tree, "TLParamsLocked").is_none(), "invisible");
    assert!(
        find(
            &cmd_inspect::feature_tree(&camera, None, true),
            "TLParamsLocked"
        )
        .is_some()
    );

    let filtered = cmd_inspect::feature_tree(&camera, Some("WIDTH"), false);
    assert!(find(&filtered, "Width").is_some() && find(&filtered, "SensorWidth").is_some());
    assert!(find(&filtered, "Height").is_none());
    assert!(
        find(&filtered, "AcquisitionControl").is_none(),
        "no match below it"
    );

    let mut text = Vec::new();
    cmd_inspect::render_tree(&filtered, &mut text).unwrap();
    let text = String::from_utf8(text).unwrap();
    assert!(
        text.contains("\n    Integer       Width = 640  (RW)\n"),
        "{text}"
    );
}

#[test]
fn describe_says_what_a_feature_is_and_what_it_depends_on() {
    let fake = Fake::start();
    let camera = fake.camera();
    let width = cmd_inspect::describe(&camera, "Width").expect("describe");
    assert_eq!(width.kind, "Integer");
    assert_eq!(
        width.tooltip.as_deref(),
        Some("Width of the image in pixels")
    );
    assert_eq!((width.min, width.max), (Some(16.0), Some(4096.0)));
    assert!(width.streamable);
    for provider in [
        "WidthMaxNow",
        "WidthMin",
        "TLParamsLocked",
        "BinningHorizontal",
    ] {
        assert!(width.depends_on.iter().any(|d| d == provider), "{width:?}");
    }
    let selector = cmd_inspect::describe(&camera, "TriggerSelector").expect("describe");
    assert_eq!(
        selector.selects,
        ["TriggerMode", "TriggerSource", "TriggerSoftware"]
    );
    assert_eq!(selector.entries, ["FrameStart", "AcquisitionStart"]);
    let mode = cmd_inspect::describe(&camera, "TriggerMode").expect("describe");
    assert_eq!(mode.selected_by, ["TriggerSelector"]);
    assert!(cmd_inspect::describe(&camera, "NoSuchFeature").is_err());
}

#[test]
fn dump_lists_every_readable_value() {
    let fake = Fake::start();
    let camera = fake.camera();
    let values = cmd_inspect::dump(&camera);
    let value = |name: &str| {
        values
            .iter()
            .find(|v| v.name == name)
            .map(|v| v.value.as_str())
    };
    assert_eq!(value("Width"), Some("640"));
    assert_eq!(value("DeviceVendorName"), Some("vitavision.dev"));
    assert_eq!(
        value("WidthMax"),
        Some("4096"),
        "registers behind features too"
    );
    assert_eq!(value("AcquisitionStart"), None, "a command has no value");
    assert_eq!(value("ImageFormatControl"), None, "nor a category");
}

#[test]
fn save_and_load_go_through_the_command_line() {
    let fake = Fake::start();
    let dir = std::env::temp_dir().join(format!("viva-camctl-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("camera.txt");
    let path = file.to_str().unwrap();
    {
        let mut camera = fake.camera();
        camera.set("Width", "320").unwrap();
    }
    assert_eq!(
        viva_camctl::run(["viva-camctl", "save", "--ip", "127.0.0.1", "--out", path]),
        0
    );
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("\nWidth\t320\n"), "{text}");
    {
        let mut camera = fake.camera();
        camera.set("Width", "640").unwrap();
    }
    assert_eq!(
        viva_camctl::run(["viva-camctl", "load", "--ip", "127.0.0.1", "--file", path]),
        0
    );
    assert_eq!(fake.peek_word(REG_WIDTH), 320);

    std::fs::write(&file, "NoSuchFeature\t1\nWidth\t400\n").unwrap();
    assert_eq!(
        viva_camctl::run(["viva-camctl", "load", "--ip", "127.0.0.1", "--file", path]),
        1,
        "a value that could not be written fails the command"
    );
    assert_eq!(fake.peek_word(REG_WIDTH), 400, "and the rest is written");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn registers_are_read_and_written_past_the_xml() {
    let fake = Fake::start();
    let mut device = fake.control();
    let read = fake
        .runtime
        .block_on(cmd_registers::read_gige(&mut device, HEARTBEAT_TIMEOUT, 4))
        .expect("READREG");
    assert_eq!(
        u32::from_be_bytes(read.clone().try_into().unwrap()),
        fake.peek_word(HEARTBEAT_TIMEOUT)
    );
    let shown =
        cmd_registers::RegisterRead::new(HEARTBEAT_TIMEOUT, &read, cmd_registers::WordOrder::Big);
    assert_eq!(shown.value, Some(fake.peek_word(HEARTBEAT_TIMEOUT)));

    fake.runtime
        .block_on(cmd_registers::write_gige(&mut device, REG_BLACK_LEVEL, 7))
        .expect("WRITEREG");
    assert_eq!(fake.peek_word(REG_BLACK_LEVEL), 7);
    assert_eq!(
        fake.controller(),
        None,
        "control given back after the write"
    );
    let unaligned = fake.runtime.block_on(cmd_registers::read_gige(
        &mut device,
        REG_BLACK_LEVEL + 1,
        4,
    ));
    assert!(unaligned.unwrap_err().to_string().contains("multiple of 4"));
}

#[test]
fn an_action_command_is_acknowledged_by_the_camera_whose_keys_match() {
    let fake = Fake::start();
    let mut params = ActionParams {
        device_key: FAKE_DEVICE_KEY,
        group_key: FAKE_GROUP_KEY,
        group_mask: FAKE_GROUP_MASK,
        scheduled_time: None,
    };
    let acks = fake
        .runtime
        .block_on(cmd_action::send(Ipv4Addr::LOCALHOST, &params, 200))
        .expect("send");
    assert_eq!(acks.acks, 1);
    params.group_key = FAKE_GROUP_KEY + 1;
    let acks = fake
        .runtime
        .block_on(cmd_action::send(Ipv4Addr::LOCALHOST, &params, 200))
        .expect("send");
    assert_eq!(acks.acks, 0, "another group's action is ignored");
}

fn usb_camera() -> (
    Arc<FakeU3vTransport>,
    Camera<U3vRegisterIo<FakeU3vTransport>>,
) {
    let transport = Arc::new(FakeU3vTransport::new(64, 64, 0x0108_0001));
    let device = U3vDevice::open(Arc::clone(&transport), 0x81, 0x01, Some(0x82), None)
        .expect("open the fake USB3 Vision device");
    let (camera, _xml) = open_u3v_device(device).expect("open the camera");
    (transport, camera)
}

#[test]
fn usb3_vision_registers_are_little_endian() {
    let transport = Arc::new(FakeU3vTransport::new(64, 64, 0x0108_0001));
    let mut device = U3vDevice::open(Arc::clone(&transport), 0x81, 0x01, Some(0x82), None).unwrap();
    let read =
        cmd_registers::read_usb(&mut device, viva_fake_u3v::registers::REG_WIDTH, 4).expect("read");
    let shown = cmd_registers::RegisterRead::new(0, &read, cmd_registers::WordOrder::Little);
    assert_eq!(shown.value, Some(64));
    cmd_registers::write_usb(&mut device, viva_fake_u3v::registers::REG_WIDTH, 32).expect("write");
    assert_eq!(
        transport.peek(viva_fake_u3v::registers::REG_WIDTH, 4),
        32u32.to_le_bytes()
    );
}

#[test]
fn usb3_vision_executes_and_benchmarks() {
    let (transport, mut camera) = usb_camera();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let done = runtime
        .block_on(cmd_execute::execute(
            &mut camera,
            "AcquisitionStop",
            Some(Duration::from_secs(1)),
        ))
        .expect("execute");
    assert!(done, "a write-only command is done at once");
    assert!(
        transport
            .writes()
            .iter()
            .any(|(addr, _)| *addr == viva_fake_u3v::registers::REG_ACQ_STOP),
        "the command reached the device"
    );
    let report = cmd_usb::bench(&mut camera, Duration::from_millis(300)).expect("bench");
    assert!(report.frames > 0, "{report:?}");
    assert_eq!(report.bytes, report.frames * 64 * 64);
    assert!(!transport.acquiring(), "acquisition stopped");
}

#[test]
fn every_new_command_parses_with_either_transport() {
    use clap::Parser;
    use viva_camctl::cli::Cli;
    for args in [
        &[
            "viva-camctl",
            "features",
            "--ip",
            "10.0.0.2",
            "--filter",
            "Gain",
        ][..],
        &["viva-camctl", "features", "--usb", "0", "--all"],
        &["viva-camctl", "describe", "--usb", "0", "--name", "Width"],
        &["viva-camctl", "dump", "--index", "0"],
        &["viva-camctl", "save", "--usb", "0", "--out", "c.txt"],
        &["viva-camctl", "load", "--ip", "10.0.0.2", "--file", "c.txt"],
        &[
            "viva-camctl",
            "read-reg",
            "--usb",
            "0",
            "--addr",
            "0x938",
            "--len",
            "8",
        ],
        &[
            "viva-camctl",
            "write-reg",
            "--ip",
            "10.0.0.2",
            "--addr",
            "2360",
            "--value",
            "0xbb8",
        ],
        &["viva-camctl", "get", "--usb", "0", "--name", "Width"],
        &[
            "viva-camctl",
            "set",
            "--usb",
            "0",
            "--name",
            "Width",
            "--value",
            "64",
        ],
        &[
            "viva-camctl",
            "execute",
            "--usb",
            "0",
            "--name",
            "TriggerSoftware",
        ],
        &["viva-camctl", "bench", "--usb", "0", "--duration-s", "5"],
        &[
            "viva-camctl",
            "action",
            "--device-key",
            "0x42",
            "--group-key",
            "1",
            "--group-mask",
            "0xffffffff",
            "--scheduled-time",
            "1000",
        ],
    ] {
        assert!(Cli::try_parse_from(args).is_ok(), "{args:?}");
    }
    for args in [
        &[
            "viva-camctl",
            "get",
            "--ip",
            "10.0.0.2",
            "--usb",
            "0",
            "--name",
            "Width",
        ][..],
        &[
            "viva-camctl",
            "write-reg",
            "--usb",
            "0",
            "--addr",
            "0",
            "--value",
            "0x100000000",
        ],
        &["viva-camctl", "read-reg", "--usb", "0", "--addr", "twelve"],
    ] {
        assert!(Cli::try_parse_from(args).is_err(), "{args:?}");
    }
}

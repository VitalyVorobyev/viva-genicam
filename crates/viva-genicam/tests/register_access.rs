//! Which GVCP command a register access puts on the wire, asserted at the fake.
//!
//! READREG and READMEM return the same bytes for the same word, so a test that
//! checks only the value cannot tell them apart — which is how every register
//! access going out as READMEM went unnoticed until a Wireshark capture in
//! [#136](https://github.com/VitalyVorobyev/viva-genicam/issues/136). These
//! tests count commands at the fake instead (backlog TC-22, TC-23), and check
//! device state with `FakeCamera::peek` rather than with our own read-back.
//!
//! ```sh
//! cargo test -p viva-genicam --test register_access
//! ```

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use viva_fake_gige::registers::{
    CCP, FIRST_URL_REG, REG_ACQ_START, REG_EXPOSURE_TIME, REG_SOFTWARE_SIGNAL_PULSE, REG_WIDTH,
};
use viva_fake_gige::{FakeCamera, GvcpCommand, RegisterCommandRefusal};
use viva_genicam::genapi::GenApiError;
use viva_genicam::gencp::StatusCode;
use viva_genicam::gige::gvcp::GigeError;
use viva_genicam::{Camera, GenicamError, GigeRegisterIo, connect_gige, gige};

type SharedCamera = Arc<Mutex<Camera<GigeRegisterIo>>>;

async fn connect_fake() -> SharedCamera {
    let devices = gige::discover_all(Duration::from_secs(2))
        .await
        .expect("discovery failed");
    let device = devices
        .into_iter()
        .find(|d| d.ip.is_loopback())
        .expect("fake camera not found on loopback");
    let camera = connect_gige(&device).await.expect("connect failed");
    Arc::new(Mutex::new(camera))
}

/// Run `op` against the camera from the blocking pool, where `GigeRegisterIo`
/// is allowed to block.
async fn with_camera<R, F>(camera: &SharedCamera, op: F) -> R
where
    R: Send + 'static,
    F: FnOnce(&mut Camera<GigeRegisterIo>) -> R + Send + 'static,
{
    let camera = camera.clone();
    tokio::task::spawn_blocking(move || op(&mut camera.lock().unwrap()))
        .await
        .unwrap()
}

async fn peek_u32(fake: &FakeCamera, addr: u64) -> u32 {
    let bytes = fake.peek(addr, 4).await;
    u32::from_be_bytes(bytes.try_into().unwrap())
}

/// Read `Width` and write it back changed, returning the value written.
async fn read_and_change_width(camera: &SharedCamera) -> u32 {
    with_camera(camera, |cam| {
        let width: u32 = cam.get("Width").expect("read Width").parse().unwrap();
        let changed = if width == 320 { 256 } else { 320 };
        cam.set("Width", &changed.to_string()).expect("set Width");
        changed
    })
    .await
}

/// TC-22: a 4-byte feature goes out as READREG/WRITEREG; bulk access stays
/// READMEM.
#[tokio::test(flavor = "multi_thread")]
async fn single_register_features_use_readreg_and_writereg() {
    let cam = common::TestCamera::start().await;
    let camera = connect_fake().await;
    let commands = cam.fake().commands();

    // Connecting claimed CCP (one register) and fetched the XML (bulk).
    assert_eq!(commands.at(GvcpCommand::WriteReg, CCP), 1, "CCP claim");
    assert_eq!(commands.at(GvcpCommand::WriteMem, CCP), 0);
    assert!(
        commands.at(GvcpCommand::ReadMem, FIRST_URL_REG) >= 1,
        "the 512-byte URL register is read with READMEM"
    );
    assert_eq!(commands.at(GvcpCommand::ReadReg, FIRST_URL_REG), 0);

    commands.reset();
    let written = read_and_change_width(&camera).await;
    assert_eq!(commands.at(GvcpCommand::ReadReg, REG_WIDTH), 1);
    assert_eq!(commands.at(GvcpCommand::WriteReg, REG_WIDTH), 1);
    assert_eq!(commands.at(GvcpCommand::ReadMem, REG_WIDTH), 0);
    assert_eq!(commands.at(GvcpCommand::WriteMem, REG_WIDTH), 0);
    assert_eq!(peek_u32(cam.fake(), REG_WIDTH).await, written);

    // `ExposureTime` is an 8-byte float: one READMEM, never two READREGs.
    with_camera(&camera, |cam| cam.get("ExposureTime").expect("read")).await;
    assert_eq!(commands.at(GvcpCommand::ReadMem, REG_EXPOSURE_TIME), 1);
    assert_eq!(commands.at(GvcpCommand::ReadReg, REG_EXPOSURE_TIME), 0);
    assert_eq!(commands.at(GvcpCommand::ReadReg, REG_EXPOSURE_TIME + 4), 0);
}

/// Connect to a fake that refuses register commands and check the whole
/// session, from the CCP claim on, lands on the device through memory access.
async fn assert_falls_back_to_memory(refusal: RegisterCommandRefusal) {
    let cam = common::TestCamera::start_with(|b| b.refuse_register_commands(refusal)).await;
    let camera = connect_fake().await;
    let commands = cam.fake().commands();

    // The CCP claim was the first register command. It was refused, retried
    // as WRITEMEM, and the device holds privilege — not merely an `Ok`.
    assert!(
        commands.at(GvcpCommand::WriteReg, CCP) >= 1,
        "tried WRITEREG"
    );
    assert_eq!(commands.at(GvcpCommand::WriteMem, CCP), 1, "fell back");
    assert_eq!(peek_u32(cam.fake(), CCP).await, 0x2, "privilege held");
    let latched = with_camera(&camera, |cam| {
        cam.transport()
            .lock_device()
            .expect("device")
            .memory_access_only()
    })
    .await;
    assert!(latched, "the session is latched to memory access");

    // Everything after the latch goes straight to memory access.
    commands.reset();
    let written = read_and_change_width(&camera).await;
    assert_eq!(commands.at(GvcpCommand::ReadMem, REG_WIDTH), 1);
    assert_eq!(commands.at(GvcpCommand::WriteMem, REG_WIDTH), 1);
    assert_eq!(commands.total(GvcpCommand::ReadReg), 0);
    assert_eq!(commands.total(GvcpCommand::WriteReg), 0);
    assert_eq!(peek_u32(cam.fake(), REG_WIDTH).await, written);
}

/// TC-22 fallback: a device that answers READREG/WRITEREG `NOT_IMPLEMENTED`.
#[tokio::test(flavor = "multi_thread")]
async fn not_implemented_register_commands_fall_back_to_memory() {
    assert_falls_back_to_memory(RegisterCommandRefusal::NotImplemented).await;
}

/// TC-22 fallback: a device that ignores READREG/WRITEREG entirely.
///
/// Costs one full retry budget (four control timeouts) before the fallback, and
/// only once per session.
#[tokio::test(flavor = "multi_thread")]
async fn unanswered_register_commands_fall_back_to_memory() {
    assert_falls_back_to_memory(RegisterCommandRefusal::NoReply).await;
}

/// `GigeDevice::use_memory_access`, which `VIVA_GIGE_FORCE_READMEM` feeds at
/// open, sends every single-register access as memory access. The environment
/// variable itself is covered by a unit test of its parser: setting it here
/// would race every other test in this binary.
#[tokio::test(flavor = "multi_thread")]
async fn forced_memory_access_sends_no_register_commands() {
    let cam = common::TestCamera::start().await;
    let camera = connect_fake().await;
    with_camera(&camera, |cam| {
        cam.transport()
            .lock_device()
            .expect("device")
            .use_memory_access()
    })
    .await;

    let commands = cam.fake().commands();
    commands.reset();
    let written = read_and_change_width(&camera).await;
    assert_eq!(commands.at(GvcpCommand::ReadMem, REG_WIDTH), 1);
    assert_eq!(commands.at(GvcpCommand::WriteMem, REG_WIDTH), 1);
    assert_eq!(commands.at(GvcpCommand::ReadReg, REG_WIDTH), 0);
    assert_eq!(commands.at(GvcpCommand::WriteReg, REG_WIDTH), 0);
    assert_eq!(peek_u32(cam.fake(), REG_WIDTH).await, written);
}

/// GA-31 (#135): setting one bit of a write-only register must not read the
/// register first. The fake refuses that read — TC-23 — so before the fix this
/// failed with `ACCESS_DENIED` from the wire; now it is refused locally, with
/// an error that says why, and nothing reaches the device.
///
/// Only the cold-cache case is reachable here: a write-only bitfield's cache is
/// warmed only by a successful write of that node, which is exactly what a cold
/// cache refuses. The warm path is covered by the `viva-genapi` unit test.
#[tokio::test(flavor = "multi_thread")]
async fn masked_write_to_a_write_only_register_never_reads_it() {
    let cam = common::TestCamera::start().await;
    let camera = connect_fake().await;
    let commands = cam.fake().commands();
    commands.reset();

    let err = with_camera(&camera, |cam| cam.set("SoftwareSignal0Pulse", "1"))
        .await
        .expect_err("the other bits of a WO register cannot be read");
    assert!(
        matches!(
            err,
            GenicamError::GenApi(GenApiError::MaskedWriteUnreadable { ref name, address })
                if name == "SoftwareSignal0Pulse" && address == REG_SOFTWARE_SIGNAL_PULSE
        ),
        "got {err:?}"
    );
    for command in [
        GvcpCommand::ReadReg,
        GvcpCommand::ReadMem,
        GvcpCommand::WriteReg,
        GvcpCommand::WriteMem,
    ] {
        assert_eq!(
            commands.at(command, REG_SOFTWARE_SIGNAL_PULSE),
            0,
            "{command:?} reached the device"
        );
    }
}

/// TC-23: the fake refuses a read of a register its XML declares write-only,
/// through either read command, as a real device does. Before this, a READMEM
/// of a WO address returned the stored bytes.
///
/// The refusal is also the device's real answer about that register, so it
/// must not trip the fallback latch the way `NOT_IMPLEMENTED` does.
#[tokio::test]
async fn fake_refuses_reads_of_write_only_registers() {
    let _cam = common::TestCamera::start().await;
    let addr = std::net::SocketAddr::new([127, 0, 0, 1].into(), gige::GVCP_PORT);
    let mut device = gige::GigeDevice::open(addr).await.expect("open");

    let denied = |result: Result<_, GigeError>| {
        matches!(result, Err(GigeError::Status(StatusCode::AccessDenied)))
    };
    assert!(denied(device.read_mem(REG_ACQ_START, 4).await.map(drop)));
    assert!(denied(
        device
            .read_register(REG_SOFTWARE_SIGNAL_PULSE as u32)
            .await
            .map(drop)
    ));
    assert!(denied(
        device
            .read_register_or_mem(REG_SOFTWARE_SIGNAL_PULSE, 4)
            .await
            .map(drop)
    ));
    assert!(
        !device.memory_access_only(),
        "ACCESS_DENIED is an answer, not a missing command"
    );
}

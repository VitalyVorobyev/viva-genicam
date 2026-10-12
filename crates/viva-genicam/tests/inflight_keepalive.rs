//! The facade's device mutex cannot starve heartbeats during register I/O
//! (ADR-0023), against the fake camera: it enforces the heartbeat, answers a
//! slow command with `PENDING_ACK`, and acknowledges it only later.

mod common;

use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use viva_fake_gige::GvcpCommand;
use viva_fake_gige::{REG_BLACK_LEVEL, REG_GAIN_AUTO};
use viva_genicam::genapi::RegisterIo;
use viva_genicam::gige::DeviceInfo;
use viva_genicam::{Camera, GigeRegisterIo, connect_gige};

const CCP: u64 = 0x0a00;
const HEARTBEAT_TIMEOUT: u64 = 0x0938;

/// Open the fake and wait until the keepalive has read `GevHeartbeatTimeout`
/// and configured the in-flight probes from it: it holds the device mutex
/// across both, so a register access queued after the read sees the result.
async fn open_after_keepalive_start(cam: &common::TestCamera) -> Camera<GigeRegisterIo> {
    let camera = connect_gige(&DeviceInfo::from_ip(Ipv4Addr::LOCALHOST))
        .await
        .expect("open");
    let us = camera
        .transport()
        .lock_device()
        .unwrap()
        .local_addr()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !cam
        .fake()
        .commands()
        .from(us)
        .iter()
        .any(|&(_, addr)| addr == HEARTBEAT_TIMEOUT)
    {
        assert!(Instant::now() < deadline, "the keepalive never started");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    camera
}

/// CCP reads `peer` sent while a register access held the device mutex: the
/// idle keepalive cannot run then, so each is an in-flight probe.
fn probes(cam: &common::TestCamera, peer: std::net::SocketAddr) -> usize {
    cam.fake()
        .commands()
        .from(peer)
        .iter()
        .filter(|&&(command, addr)| {
            addr == CCP && matches!(command, GvcpCommand::ReadReg | GvcpCommand::ReadMem)
        })
        .count()
}

/// A read the camera takes 900 ms to answer, three heartbeat windows: the
/// probes sent during it keep control, so the write after it is accepted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_long_register_read_keeps_control_while_holding_the_device_mutex() {
    let cam = common::TestCamera::start_with(|b| {
        b.enforce_heartbeat(true)
            .heartbeat_timeout_ms(300)
            .delay_ack(REG_BLACK_LEVEL, Duration::from_millis(900))
    })
    .await;
    let camera = open_after_keepalive_start(&cam).await;
    let us = camera
        .transport()
        .lock_device()
        .unwrap()
        .local_addr()
        .unwrap();
    cam.fake().commands().reset();

    let camera = tokio::task::spawn_blocking(move || {
        camera
            .transport()
            .read(REG_BLACK_LEVEL, 4)
            .expect("the slow read");
        camera
            .transport()
            .write(REG_GAIN_AUTO, &1u32.to_be_bytes())
            .expect("control was kept, so the write is accepted");
        camera
    })
    .await
    .unwrap();

    assert_eq!(
        cam.fake()
            .commands()
            .at(GvcpCommand::ReadReg, REG_BLACK_LEVEL),
        1,
        "the slow read is not repeated"
    );
    assert!(
        probes(&cam, us) >= 3,
        "{:?}",
        cam.fake().commands().from(us)
    );
    assert_eq!(cam.fake().heartbeat_expiries(), 0);
    assert_eq!(cam.fake().controller().await, Some(us));
    drop(camera);
}

/// TC-26: a camera reporting a heartbeat timeout of 0 does not switch the
/// in-flight probes off. The idle keepalive reads 0 as the assumed 3 000 ms
/// window, and the probes during a long wait follow the same reading.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_heartbeat_timeout_of_zero_keeps_the_in_flight_probes() {
    let cam = common::TestCamera::start_with(|b| {
        b.heartbeat_timeout_ms(0)
            .delay_ack(REG_BLACK_LEVEL, Duration::from_millis(2000))
    })
    .await;
    let camera = open_after_keepalive_start(&cam).await;
    let us = camera
        .transport()
        .lock_device()
        .unwrap()
        .local_addr()
        .unwrap();
    cam.fake().commands().reset();

    let camera = tokio::task::spawn_blocking(move || {
        camera
            .transport()
            .read(REG_BLACK_LEVEL, 4)
            .expect("the slow read");
        camera
    })
    .await
    .unwrap();
    // A 750 ms period, as for an assumed 3 000 ms window: two probes inside
    // the two-second wait.
    assert!(
        probes(&cam, us) >= 2,
        "{:?}",
        cam.fake().commands().from(us)
    );
    drop(camera);
}

//! The GigE session lifecycle, against the in-process fake camera: who holds
//! control privilege and when it is given up, read-only monitors, losing
//! privilege, `PENDING_ACK`, discovery, and multicast monitors.
//!
//! The fake keeps control privilege per client address and port, refuses a
//! write from anyone else with `ACCESS_DENIED`, and gives privilege up on a CCP
//! write of 0, from its own reading of the bootstrap registers (ADR-0019).

mod common;

use std::net::{Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use viva_fake_gige::registers::REG_BLACK_LEVEL;
use viva_fake_gige::{FAKE_MAC, FAKE_SERIAL, GvcpCommand};
use viva_genicam::genapi::RegisterIo;
use viva_genicam::gige::{self, DeviceInfo, DiscoveryOptions, GigeDevice};
use viva_genicam::{
    Access, Camera, ControlState, GigeRegisterIo, StreamOptions, connect_gige, connect_gige_as,
};

const CCP: u64 = 0x0a00;
const GVCP_CONFIGURATION: u64 = 0x0954;
const PENDING_ACK_ENABLE: u32 = 0x2;
/// The discovery command's *allow broadcast acknowledge* flag.
const ALLOW_BROADCAST_ACK: u8 = 0x10;

fn fake_info() -> DeviceInfo {
    DeviceInfo::from_ip(Ipv4Addr::LOCALHOST)
}

fn fake_addr() -> SocketAddr {
    (Ipv4Addr::LOCALHOST, gige::GVCP_PORT).into()
}

/// The address the camera knows this session by.
fn client_addr(camera: &Camera<GigeRegisterIo>) -> SocketAddr {
    camera
        .transport()
        .lock_device()
        .expect("device")
        .local_addr()
        .expect("local address")
}

async fn open(access: Access) -> Camera<GigeRegisterIo> {
    connect_gige_as(&fake_info(), access)
        .await
        .map(|(camera, _xml)| camera)
        .unwrap_or_else(|err| panic!("open as {access:?}: {err}"))
}

fn is_write(command: GvcpCommand) -> bool {
    matches!(command, GvcpCommand::WriteReg | GvcpCommand::WriteMem)
}

// ---------------------------------------------------------------------------
// Releasing control (TC-28)
// ---------------------------------------------------------------------------

/// Dropping a camera gives its control privilege up, so the next open does
/// not wait out the heartbeat timeout — which this fake, not enforcing one,
/// would make forever.
#[tokio::test(flavor = "multi_thread")]
async fn dropping_a_camera_releases_control_for_the_next_open() {
    let cam = common::TestCamera::start().await;
    let first = connect_gige(&fake_info()).await.expect("first open");
    let first_addr = client_addr(&first);
    assert_eq!(cam.fake().controller().await, Some(first_addr));

    drop(first);
    assert_eq!(
        cam.fake().controller().await,
        None,
        "CCP is released when the camera is dropped"
    );

    let started = Instant::now();
    let second = connect_gige(&fake_info())
        .await
        .expect("a reopen is not refused");
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(cam.fake().controller().await, Some(client_addr(&second)));
}

/// An explicit release says whether the camera took it, and the session says
/// it no longer holds control.
#[tokio::test(flavor = "multi_thread")]
async fn release_control_gives_control_up_and_says_so() {
    let cam = common::TestCamera::start().await;
    let camera = open(Access::Control).await;
    camera
        .transport()
        .release_control()
        .expect("the camera acknowledges the release");
    assert_eq!(cam.fake().controller().await, None);
    assert_eq!(camera.transport().control_state(), ControlState::Released);
}

// ---------------------------------------------------------------------------
// Monitors and losing control (TC-29)
// ---------------------------------------------------------------------------

/// A monitor opens while another application controls the camera, reads, and
/// sends nothing that could disturb it: no claim, no write, no heartbeat. A
/// write is refused before it reaches the wire, and says why.
#[tokio::test(flavor = "multi_thread")]
async fn a_monitor_claims_nothing_writes_nothing_and_sends_no_heartbeat() {
    let cam = common::TestCamera::start_with(|b| b.heartbeat_timeout_ms(400)).await;
    let controller = open(Access::Control).await;
    let controller_addr = client_addr(&controller);

    let (mut monitor, _xml) = connect_gige_as(&fake_info(), Access::Monitor)
        .await
        .expect("a monitor opens while another application controls the camera");
    let monitor_addr = client_addr(&monitor);
    assert_eq!(monitor.transport().control_state(), ControlState::Monitor);
    assert_eq!(monitor.get("Width").expect("a monitor reads"), "640");

    let err = monitor
        .set("Width", "320")
        .expect_err("a monitor does not write");
    assert!(err.to_string().contains("monitor"), "{err}");

    // Longer than the heartbeat window, so a keepalive (every 100 ms here)
    // would have pinged several times.
    tokio::time::sleep(Duration::from_millis(600)).await;
    let sent = cam.fake().commands().from(monitor_addr);
    assert!(
        !sent.iter().any(|&(command, _)| is_write(command)),
        "a monitor wrote: {sent:x?}"
    );
    assert!(
        !sent.iter().any(|&(_, addr)| addr == CCP),
        "a monitor touched CCP: {sent:x?}"
    );
    assert_eq!(cam.fake().controller().await, Some(controller_addr));
    assert_eq!(cam.fake().peek(0x20000, 4).await, 640u32.to_be_bytes());
}

/// Privilege the camera took back — here as an expired heartbeat does — is
/// reported as a state change, and an explicit reclaim takes it again.
#[tokio::test(flavor = "multi_thread")]
async fn losing_control_is_reported_and_can_be_reclaimed() {
    let cam = common::TestCamera::start_with(|b| b.heartbeat_timeout_ms(400)).await;
    let camera = open(Access::Control).await;
    let mut changes = camera.transport().watch_control();
    assert_eq!(*changes.borrow(), ControlState::Held);

    cam.fake().revoke_control().await;
    tokio::time::timeout(
        Duration::from_secs(2),
        changes.wait_for(|state| *state == ControlState::Lost),
    )
    .await
    .expect("the loss is reported")
    .expect("the session is alive");
    assert_eq!(camera.transport().control_state(), ControlState::Lost);

    camera
        .transport()
        .reclaim_control()
        .expect("nobody else holds it, so it can be reclaimed");
    assert_eq!(camera.transport().control_state(), ControlState::Held);
    assert_eq!(cam.fake().controller().await, Some(client_addr(&camera)));
}

/// Another application took control in between: CCP still reads as
/// controlled, so the heartbeat cannot tell, but the camera refuses the next
/// write with `ACCESS_DENIED` — and that marks the session as having lost
/// control. Reclaiming fails while the other application holds it.
#[tokio::test(flavor = "multi_thread")]
async fn a_write_refused_for_lack_of_privilege_marks_control_lost() {
    let cam = common::TestCamera::start().await;
    let mut camera = open(Access::Control).await;

    cam.fake().revoke_control().await;
    let mut other = GigeDevice::open(fake_addr()).await.expect("open");
    other
        .claim_control()
        .await
        .expect("the other application claims");

    camera
        .set("Width", "320")
        .expect_err("the camera refuses a write from a non-controller");
    assert_eq!(camera.transport().control_state(), ControlState::Lost);
    assert!(
        camera.transport().reclaim_control().is_err(),
        "another application holds control"
    );
    other.release_control().await.expect("release");
}

/// `ACCESS_DENIED` for the register itself — a read of a write-only one —
/// is the register's answer, not a lost privilege.
#[tokio::test(flavor = "multi_thread")]
async fn a_register_refusing_an_access_does_not_mark_control_lost() {
    let _cam = common::TestCamera::start().await;
    let camera = open(Access::Control).await;
    let write_only = viva_fake_gige::registers::WRITE_ONLY_REGISTERS[0];
    camera
        .transport()
        .read(write_only, 4)
        .expect_err("a write-only register refuses a read");
    assert_eq!(camera.transport().control_state(), ControlState::Held);
}

// ---------------------------------------------------------------------------
// PENDING_ACK (TC-30)
// ---------------------------------------------------------------------------

/// A camera whose GVCP Capability offers `PENDING_ACK` gets it enabled at
/// open, so a slow command is answered with one and never retried.
#[tokio::test(flavor = "multi_thread")]
async fn pending_ack_is_enabled_where_the_capability_says_so() {
    let cam = common::TestCamera::start_with(|b| {
        b.delay_ack(REG_BLACK_LEVEL, Duration::from_millis(1200))
    })
    .await;
    let camera = open(Access::Control).await;
    let config = cam.fake().peek(GVCP_CONFIGURATION, 4).await;
    assert_eq!(
        u32::from_be_bytes(config.try_into().unwrap()) & PENDING_ACK_ENABLE,
        PENDING_ACK_ENABLE,
        "PENDING_ACK enable bit in GVCP Configuration"
    );

    let commands = cam.fake().commands();
    commands.reset();
    camera
        .transport()
        .read(REG_BLACK_LEVEL, 4)
        .expect("the slow read completes");
    let sent = commands.at(GvcpCommand::ReadReg, REG_BLACK_LEVEL)
        + commands.at(GvcpCommand::ReadMem, REG_BLACK_LEVEL);
    assert_eq!(sent, 1, "a PENDING_ACKed command is not sent again");
}

/// Without the capability bit nothing is written to GVCP Configuration.
#[tokio::test(flavor = "multi_thread")]
async fn pending_ack_is_left_alone_without_the_capability() {
    let cam = common::TestCamera::start_with(|b| b.pending_ack(false)).await;
    let _camera = open(Access::Control).await;
    assert_eq!(cam.fake().peek(GVCP_CONFIGURATION, 4).await, [0, 0, 0, 0]);
    assert_eq!(
        cam.fake()
            .commands()
            .at(GvcpCommand::WriteReg, GVCP_CONFIGURATION),
        0
    );
}

// ---------------------------------------------------------------------------
// WRITEMEM (TC-32)
// ---------------------------------------------------------------------------

/// A write shorter than a word, or off a word boundary, goes out as the
/// whole words around it, the bytes it does not cover read from the device
/// first, so the neighbours keep their values.
#[tokio::test(flavor = "multi_thread")]
async fn an_unaligned_memory_write_goes_out_as_whole_words() {
    let cam = common::TestCamera::start().await;
    let mut device = GigeDevice::open(fake_addr()).await.expect("open");
    device.claim_control().await.expect("claim");
    device
        .write_register(REG_BLACK_LEVEL as u32, 0x1122_3344)
        .await
        .expect("set the word");

    device
        .write_mem(REG_BLACK_LEVEL + 2, &[0xAB, 0xCD])
        .await
        .expect("a two-byte write at an odd half-word");
    assert_eq!(
        cam.fake().peek(REG_BLACK_LEVEL, 4).await,
        [0x11, 0x22, 0xAB, 0xCD]
    );
    let commands = cam.fake().commands();
    assert_eq!(commands.at(GvcpCommand::WriteMem, REG_BLACK_LEVEL + 2), 0);
    assert_eq!(commands.at(GvcpCommand::WriteMem, REG_BLACK_LEVEL), 1);
    device.release_control().await.expect("release");
}

// ---------------------------------------------------------------------------
// Discovery (TC-31)
// ---------------------------------------------------------------------------

/// Discovery asks for a unicast acknowledgement; the broadcast one is opt-in.
#[tokio::test(flavor = "multi_thread")]
async fn discovery_asks_for_a_unicast_acknowledgement_unless_told_otherwise() {
    let cam = common::TestCamera::start().await;
    gige::discover_all(Duration::from_millis(300))
        .await
        .expect("discovery");
    let asked = cam.fake().discovery_requests();
    assert!(!asked.is_empty(), "the fake heard no discovery");
    assert!(
        asked.iter().all(|r| r.flags & ALLOW_BROADCAST_ACK == 0),
        "{asked:?}"
    );

    let options = DiscoveryOptions::new()
        .include_loopback(true)
        .allow_broadcast_ack(true);
    gige::discover_with(Duration::from_millis(300), &options)
        .await
        .expect("discovery");
    let last = cam.fake().discovery_requests().pop().expect("asked again");
    assert_eq!(last.flags & ALLOW_BROADCAST_ACK, ALLOW_BROADCAST_ACK);
}

/// A camera named by address is asked directly, and answers with its
/// identity.
#[tokio::test(flavor = "multi_thread")]
async fn a_camera_answers_discovery_sent_to_its_address() {
    let cam = common::TestCamera::start().await;
    let info = gige::discover_ip(Ipv4Addr::LOCALHOST, Duration::from_secs(1))
        .await
        .expect("discovery")
        .expect("the camera answers");
    assert_eq!(info.ip, Ipv4Addr::LOCALHOST);
    assert_eq!(info.mac, FAKE_MAC);
    assert_eq!(info.serial.as_deref(), Some(FAKE_SERIAL));
    assert_eq!(info.local_ip, Some(Ipv4Addr::LOCALHOST));
    let asked = cam.fake().discovery_requests();
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert_eq!(asked[0].flags & ALLOW_BROADCAST_ACK, 0);
}

/// Subnet and gateway come back from the Discovery ACK, and a camera
/// configured for another network — it answers, but cannot be reached — is
/// recognisable as such.
#[tokio::test(flavor = "multi_thread")]
async fn discovery_reports_a_camera_configured_for_another_subnet() {
    let away = Ipv4Addr::new(192, 168, 77, 10);
    let _cam = common::TestCamera::start_with(|b| {
        b.current_ip(away)
            .subnet_mask(Ipv4Addr::new(255, 255, 255, 0))
            .gateway(Ipv4Addr::new(192, 168, 77, 1))
    })
    .await;
    let found = gige::discover_all(Duration::from_millis(500))
        .await
        .expect("discovery")
        .into_iter()
        .find(|d| d.mac == FAKE_MAC)
        .expect("the camera answers");
    assert_eq!(found.ip, away);
    assert_eq!(found.subnet, Some(Ipv4Addr::new(255, 255, 255, 0)));
    assert_eq!(found.gateway, Some(Ipv4Addr::new(192, 168, 77, 1)));
    assert_eq!(found.local_ip, Some(Ipv4Addr::LOCALHOST));
    assert_eq!(found.shares_subnet_with_host(), Some(false));
}

/// The usual case, for contrast: loopback's camera on loopback's `/8`.
#[tokio::test(flavor = "multi_thread")]
async fn discovery_recognises_a_camera_on_the_host_subnet() {
    let _cam = common::TestCamera::start().await;
    let found = gige::discover_all(Duration::from_millis(500))
        .await
        .expect("discovery")
        .into_iter()
        .find(|d| d.mac == FAKE_MAC)
        .expect("the camera answers");
    assert_eq!(found.subnet, Some(Ipv4Addr::new(255, 0, 0, 0)));
    assert_eq!(found.shares_subnet_with_host(), Some(true));
}

// ---------------------------------------------------------------------------
// Multicast (SR-20)
// ---------------------------------------------------------------------------

/// Without the SFNC nodes the destination port goes to the bootstrap
/// register directly — as a 32-bit word with the port in its low half.
#[tokio::test(flavor = "multi_thread")]
async fn the_raw_multicast_fallback_writes_the_port_into_the_low_half() {
    let cam = common::TestCamera::start().await;
    let mut camera = open(Access::Control).await;
    let group = Ipv4Addr::new(239, 192, 1, 20);
    camera
        .configure_stream_multicast(0, group, 50020)
        .expect("configure");
    assert_eq!(cam.fake().peek(0x0d18, 4).await, group.octets());
    assert_eq!(
        cam.fake().peek(0x0d00, 4).await,
        u32::from(50020u16).to_be_bytes()
    );
}

/// Whether this host delivers a multicast datagram sent on loopback to a
/// socket joined on loopback. macOS does; not every CI runner's loopback
/// carries multicast.
fn loopback_multicast_works() -> bool {
    use socket2::{Domain, Protocol, Socket, Type};
    let group = Ipv4Addr::new(239, 192, 1, 99);
    let probe = || -> std::io::Result<bool> {
        let rx = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        rx.set_reuse_address(true)?;
        rx.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)).into())?;
        rx.join_multicast_v4(&group, &Ipv4Addr::LOCALHOST)?;
        let port = rx.local_addr()?.as_socket().map_or(0, |a| a.port());
        let rx: std::net::UdpSocket = rx.into();
        rx.set_read_timeout(Some(Duration::from_millis(300)))?;
        let tx = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        tx.bind(&SocketAddr::from((Ipv4Addr::LOCALHOST, 0)).into())?;
        tx.set_multicast_if_v4(&Ipv4Addr::LOCALHOST)?;
        tx.set_multicast_loop_v4(true)?;
        tx.send_to(b"probe", &SocketAddr::from((group, port)).into())?;
        let mut buf = [0u8; 8];
        Ok(rx.recv_from(&mut buf).is_ok())
    };
    probe().unwrap_or(false)
}

/// A second application watches a multicast stream it did not configure:
/// it opens as a monitor, reads the group and port from the camera, joins
/// and receives frames, and never writes to the camera.
#[tokio::test(flavor = "multi_thread")]
async fn a_monitor_watches_a_multicast_stream_it_did_not_configure() {
    if !loopback_multicast_works() {
        eprintln!("skipped: loopback does not carry multicast on this host");
        return;
    }
    let cam = common::TestCamera::start().await;
    let lo = gige::nic::Iface::from_ipv4(Ipv4Addr::LOCALHOST).expect("loopback");
    let mut controller = open(Access::Control).await;
    let _own = controller
        .open_stream(
            StreamOptions::new()
                .iface(lo.clone())
                .multicast(Ipv4Addr::new(239, 192, 1, 21)),
        )
        .expect("the controller streams to a group");

    let mut monitor = open(Access::Monitor).await;
    let monitor_addr = client_addr(&monitor);
    let mut watched = monitor
        .open_stream(StreamOptions::monitor().iface(lo))
        .expect("a monitor joins the group");
    controller.acquisition_start().expect("start");

    let frame = tokio::time::timeout(Duration::from_secs(5), watched.next_frame())
        .await
        .expect("a frame within 5 s")
        .expect("stream error")
        .expect("stream ended");
    assert_eq!((frame.width, frame.height), (640, 480));
    controller.acquisition_stop().expect("stop");

    let sent = cam.fake().commands().from(monitor_addr);
    assert!(
        !sent.iter().any(|&(command, _)| is_write(command)),
        "a monitor wrote: {sent:x?}"
    );
}

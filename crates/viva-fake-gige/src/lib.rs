//! In-process fake GigE Vision camera for testing and demos.
//!
//! This crate provides a simulated GigE Vision camera that speaks real GVCP/GVSP
//! protocols over UDP on localhost. It is intended for integration testing and
//! demonstrations without requiring physical camera hardware or external tools
//! like aravis.
//!
//! # Example
//!
//! ```rust,no_run
//! use viva_fake_gige::FakeCamera;
//!
//! # async fn example() {
//! let camera = FakeCamera::builder()
//!     .width(640)
//!     .height(480)
//!     .fps(30)
//!     .bind_ip([127, 0, 0, 1].into())
//!     .build()
//!     .await
//!     .expect("failed to start fake camera");
//!
//! // Camera is now discoverable on the configured port.
//! // Use viva_genicam::gige::discover() to find it.
//!
//! // When done:
//! camera.stop().await;
//! # }
//! ```

mod gvcp_server;

pub use gvcp_server::{
    CommandCounters, FAKE_DEVICE_KEY, FAKE_GROUP_KEY, FAKE_GROUP_MASK, FAKE_MAC, FAKE_MANUFACTURER,
    FAKE_MODEL, FAKE_SERIAL, FAKE_USER_NAME, FAKE_VERSION, GvcpCommand, RegisterCommandRefusal,
};
pub mod gvsp;
mod gvsp_sender;
pub mod registers;
pub mod resend;

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub use gvsp::{PacketScript, ScriptStep};
pub use resend::ResendStats;

use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::UdpSocket;
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinHandle;
use tracing::info;

/// Builder for configuring and starting a fake GigE Vision camera.
pub struct FakeCameraBuilder {
    width: u32,
    height: u32,
    fps: u32,
    bind_ip: Ipv4Addr,
    port: u16,
    pixel_format: u32,
    zip_xml: bool,
    enforce_heartbeat: bool,
    heartbeat_timeout_ms: Option<u32>,
    max_packet_size: Option<u32>,
    max_on_wire: Option<u32>,
    refuse_register_commands: Option<RegisterCommandRefusal>,
    identity: gvcp_server::Identity,
    sender: gvsp_sender::SenderOptions,
    packet_resend: bool,
    resend_retention: usize,
    tick_frequency: Option<u64>,
}

/// PFNC pixel format codes.
pub const MONO8: u32 = 0x0108_0001;
pub const RGB8: u32 = 0x0218_0014;
/// GigE Vision `Mono12Packed`: two 12-bit pixels in three bytes.
pub const MONO12_PACKED: u32 = 0x010C_0006;
/// PFNC `Mono12p`: 12-bit pixels packed LSB-first.
pub const MONO12P: u32 = 0x010C_0047;

impl Default for FakeCameraBuilder {
    fn default() -> Self {
        Self {
            width: 640,
            height: 480,
            fps: 30,
            bind_ip: Ipv4Addr::LOCALHOST,
            port: 3956,
            pixel_format: MONO8,
            zip_xml: false,
            enforce_heartbeat: false,
            heartbeat_timeout_ms: None,
            max_packet_size: None,
            max_on_wire: None,
            refuse_register_commands: None,
            identity: gvcp_server::Identity::default(),
            sender: gvsp_sender::SenderOptions::default(),
            packet_resend: true,
            resend_retention: resend::DEFAULT_RETENTION,
            tick_frequency: None,
        }
    }
}

impl FakeCameraBuilder {
    /// Set the image width in pixels.
    pub fn width(mut self, width: u32) -> Self {
        self.width = width;
        self
    }

    /// Set the image height in pixels.
    pub fn height(mut self, height: u32) -> Self {
        self.height = height;
        self
    }

    /// Set the target frame rate.
    pub fn fps(mut self, fps: u32) -> Self {
        self.fps = fps;
        self
    }

    /// Set the IPv4 address to bind the GVCP socket to.
    pub fn bind_ip(mut self, ip: Ipv4Addr) -> Self {
        self.bind_ip = ip;
        self
    }

    /// Set the GVCP control port (default: 3956).
    pub fn port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Set the initial pixel format (PFNC code). Default: Mono8.
    ///
    /// Use [`MONO8`] or [`RGB8`] constants.
    pub fn pixel_format(mut self, code: u32) -> Self {
        self.pixel_format = code;
        self
    }

    /// Serve the GenApi XML as a ZIP archive (default: plain XML).
    ///
    /// Many real cameras (Basler, FLIR, Hikrobot, ...) publish their
    /// register-description XML zipped; enable this to exercise that path.
    pub fn zip_xml(mut self, enable: bool) -> Self {
        self.zip_xml = enable;
        self
    }

    /// Release control privilege when the controller stops sending GVCP commands
    /// for longer than `GevHeartbeatTimeout` (default: off).
    ///
    /// See [`registers::RegisterMap::enforce_heartbeat`] for why this is not the
    /// default even though every real device behaves this way.
    pub fn enforce_heartbeat(mut self, enable: bool) -> Self {
        self.enforce_heartbeat = enable;
        self
    }

    /// Clamp `GevSCPSPacketSize` to `max`, the way a real camera caps it.
    ///
    /// A request above `max` is acknowledged and silently reduced, so the
    /// register reads back lower than what was written. Off by default: the
    /// fake accepts any size, which is what every existing test expects.
    ///
    /// The fake accepted anything until 0.4.1, so it could not express the
    /// camera behind
    /// [#112](https://github.com/VitalyVorobyev/viva-genicam/issues/112) and no
    /// test could have caught that defect — the ADR-0019 failure mode of a fake
    /// that only ever agrees with its client.
    pub fn max_packet_size(mut self, max: u32) -> Self {
        self.max_packet_size = Some(max);
        self
    }

    /// Silently drop any GVSP datagram larger than `max`, as a network path
    /// with a smaller frame ceiling than either endpoint believes does.
    ///
    /// Different from [`FakeCameraBuilder::max_packet_size`], and the
    /// difference is the whole of
    /// [#112](https://github.com/VitalyVorobyev/viva-genicam/issues/112): that
    /// camera accepts and *stores* 16114, then streams nothing, because the
    /// link tops out at a 9216-byte frame. No register read can find that —
    /// only a test packet can.
    pub fn max_on_wire(mut self, max: u32) -> Self {
        self.max_on_wire = Some(max);
        self
    }

    /// Refuse every READREG and WRITEREG, as a device that implements only the
    /// memory commands would (default: serve them).
    ///
    /// The only way to exercise a controller's fallback from register to
    /// memory access without hardware that lacks the register commands. The
    /// refused command changes nothing on the device, so a test can tell a
    /// write that fell back from one that was silently lost.
    pub fn refuse_register_commands(mut self, refusal: RegisterCommandRefusal) -> Self {
        self.refuse_register_commands = Some(refusal);
        self
    }

    /// Send 20-byte extended-ID GVSP headers with 64-bit block ids and 32-bit
    /// packet ids (GigE Vision 2.0), as a LUCID TRT009S-E does (default: off).
    ///
    /// The stride shrinks by the 12 extra header bytes, so a receiver that
    /// assumes the 8-byte header places every packet after the first 12 bytes
    /// too far.
    pub fn extended_ids(mut self, enable: bool) -> Self {
        self.sender.extended_ids = enable;
        self
    }

    /// Put `payload_type` in every leader and trailer instead of `0x0001`
    /// (`0x4001` while chunk mode is active).
    pub fn payload_type(mut self, payload_type: u16) -> Self {
        self.sender.payload_type = Some(payload_type);
        self
    }

    /// Damage the stream deterministically: drop, repeat or reorder packets
    /// by block and packet id. Block ids count from 1 at each acquisition
    /// start.
    pub fn packet_script(mut self, script: PacketScript) -> Self {
        self.sender.script = script;
        self
    }

    /// Answer `PACKETRESEND_CMD`, and say so in the GVCP Capability register
    /// (`0x0934`, mask `0x4`) (default: on).
    ///
    /// Off, the capability bit is clear and requests are counted in
    /// [`FakeCamera::resends`] but never answered, as on a device without
    /// resend.
    pub fn packet_resend(mut self, enable: bool) -> Self {
        self.packet_resend = enable;
        self
    }

    /// Keep the packets of the last `frames` blocks for resending (default
    /// 4). A request for an older block is answered with a header-only GVSP
    /// packet carrying `PACKET_UNAVAILABLE` (`0x800C`).
    pub fn resend_retention(mut self, frames: usize) -> Self {
        self.resend_retention = frames;
        self
    }

    /// Report `hz` as the timestamp tick frequency (bootstrap `0x093C` and
    /// `0x0940`; default 1 GHz), the unit `GevSCPD` counts in. `0` is what a
    /// device without a timestamp clock reports.
    pub fn tick_frequency(mut self, hz: u64) -> Self {
        self.tick_frequency = Some(hz);
        self
    }

    /// Also send every block from a second socket bound to `addr`, ahead of
    /// the genuine one and with its image bytes inverted: valid GVSP from an
    /// address that is not the camera's, which a receiver must ignore.
    /// `addr` must be one of this host's addresses other than the camera's.
    pub fn spoof_source(mut self, addr: Ipv4Addr) -> Self {
        self.sender.spoof_source = Some(addr);
        self
    }

    /// Report a different `GevHeartbeatTimeout` than the 3 000 ms default.
    ///
    /// A shorter window keeps a test that has to wait one out from dominating
    /// the suite's runtime.
    pub fn heartbeat_timeout_ms(mut self, timeout_ms: u32) -> Self {
        self.heartbeat_timeout_ms = Some(timeout_ms);
        self
    }

    /// Report `mac` instead of [`FAKE_MAC`], in the Discovery ACK and the
    /// bootstrap MAC registers alike.
    ///
    /// Services key a GigE camera by its MAC, so two fakes that should appear
    /// as two cameras need two MACs, not only two serials.
    pub fn mac(mut self, mac: [u8; 6]) -> Self {
        self.identity.mac = mac;
        self
    }

    /// Report `model` instead of [`FAKE_MODEL`] in the Discovery ACK.
    ///
    /// The field is 32 bytes on the wire; a longer name is truncated.
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.identity.model = model.into();
        self
    }

    /// Report `serial` instead of [`FAKE_SERIAL`] in the Discovery ACK.
    ///
    /// The field is 16 bytes on the wire; a longer serial is truncated.
    pub fn serial(mut self, serial: impl Into<String>) -> Self {
        self.identity.serial = serial.into();
        self
    }

    /// Report `user_name` instead of [`FAKE_USER_NAME`] in the Discovery ACK.
    /// An empty string reports no user-defined name, as an unnamed camera does.
    ///
    /// The field is 16 bytes on the wire; a longer name is truncated.
    pub fn user_name(mut self, user_name: impl Into<String>) -> Self {
        self.identity.user_name = user_name.into();
        self
    }

    /// Start the fake camera and return a handle.
    pub async fn build(self) -> Result<FakeCamera, std::io::Error> {
        let mut register_map =
            registers::RegisterMap::new(self.width, self.height, self.pixel_format, self.zip_xml);
        if let Some(timeout_ms) = self.heartbeat_timeout_ms {
            register_map.set_heartbeat_timeout_ms(timeout_ms);
        }
        if let Some(max) = self.max_packet_size {
            register_map.set_max_packet_size(max);
        }
        if let Some(max) = self.max_on_wire {
            register_map.set_max_on_wire(max);
        }
        register_map.enforce_heartbeat(self.enforce_heartbeat);
        register_map.set_mac(self.identity.mac);
        register_map.set_packet_resend(self.packet_resend);
        if let Some(hz) = self.tick_frequency {
            register_map.set_tick_frequency(hz);
        }
        let regs = Arc::new(Mutex::new(register_map));

        let acq_start = Arc::new(Notify::new());
        let acq_stop_flag = Arc::new(AtomicBool::new(false));
        let counters = Arc::new(CommandCounters::default());
        let blocks_sent = Arc::new(AtomicU64::new(0));
        let spoofed = Arc::new(AtomicU64::new(0));
        let resends = Arc::new(resend::ResendCounters::default());
        let retention = Arc::new(std::sync::Mutex::new(resend::Retention::new(
            self.resend_retention,
        )));

        // Bind GVCP control socket. On macOS `SO_REUSEADDR` is a no-op for UDP,
        // so also set `SO_REUSEPORT` to let a fresh socket rebind the port while
        // the previous camera's tokio task is still shutting down (matters for
        // back-to-back module-scoped pytest fixtures).
        let bind_addr: std::net::SocketAddr =
            format!("{}:{}", self.bind_ip, self.port).parse().unwrap();
        let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        sock.set_reuse_address(true)?;
        #[cfg(unix)]
        sock.set_reuse_port(true)?;
        sock.set_nonblocking(true)?;
        sock.bind(&bind_addr.into())?;
        let std_sock: std::net::UdpSocket = sock.into();
        let socket = Arc::new(UdpSocket::from_std(std_sock)?);
        let local_addr = socket.local_addr()?;
        info!(%local_addr, "fake camera GVCP listening");

        // Spawn GVCP server task.
        let gvcp_handle = {
            let socket = socket.clone();
            let regs = regs.clone();
            let acq_start = acq_start.clone();
            let acq_stop = acq_stop_flag.clone();
            let bind_ip = self.bind_ip;
            let options = gvcp_server::ServerOptions {
                counters: counters.clone(),
                refuse_register_commands: self.refuse_register_commands,
                retention: retention.clone(),
                resend: resends.clone(),
            };
            let identity = self.identity;
            tokio::spawn(async move {
                gvcp_server::run(
                    socket, regs, acq_start, acq_stop, bind_ip, options, identity,
                )
                .await;
            })
        };

        // Spawn GVSP streaming task.
        let gvsp_handle = {
            let regs = regs.clone();
            let acq_start = acq_start.clone();
            let acq_stop = acq_stop_flag.clone();
            let fps = self.fps;
            let options = self.sender;
            let shared = gvsp_sender::SenderShared {
                blocks_sent: blocks_sent.clone(),
                spoofed: spoofed.clone(),
                retention,
                source_ip: self.bind_ip,
            };
            tokio::spawn(async move {
                gvsp_sender::run(regs, acq_start, acq_stop, fps, options, shared).await;
            })
        };

        Ok(FakeCamera {
            gvcp_handle: Some(gvcp_handle),
            gvsp_handle: Some(gvsp_handle),
            regs,
            counters,
            blocks_sent,
            spoofed,
            resends,
            local_addr,
        })
    }
}

/// Handle to a running fake GigE Vision camera.
///
/// The camera runs as background tokio tasks. Call [`stop`](FakeCamera::stop) or
/// drop the handle to shut down the camera.
pub struct FakeCamera {
    gvcp_handle: Option<JoinHandle<()>>,
    gvsp_handle: Option<JoinHandle<()>>,
    regs: Arc<Mutex<registers::RegisterMap>>,
    counters: Arc<CommandCounters>,
    blocks_sent: Arc<AtomicU64>,
    spoofed: Arc<AtomicU64>,
    resends: Arc<resend::ResendCounters>,
    local_addr: std::net::SocketAddr,
}

impl FakeCamera {
    /// Create a new builder.
    pub fn builder() -> FakeCameraBuilder {
        FakeCameraBuilder::default()
    }

    /// Start a fake camera with default settings on 127.0.0.1:3956.
    pub async fn start() -> Result<Self, std::io::Error> {
        Self::builder().build().await
    }

    /// The local address the GVCP socket is bound to.
    pub fn local_addr(&self) -> std::net::SocketAddr {
        self.local_addr
    }

    /// The port the GVCP socket is listening on.
    pub fn port(&self) -> u16 {
        self.local_addr.port()
    }

    /// Which register-access commands this camera has received, and where.
    ///
    /// For tests that must assert *how* a value reached the device: READREG
    /// and READMEM return identical bytes, so a test that checks only the
    /// value cannot tell them apart.
    pub fn commands(&self) -> &CommandCounters {
        &self.counters
    }

    /// GVSP blocks this camera has produced, including any its packet script
    /// then dropped whole.
    ///
    /// The left-hand side of a receiver's accounting identity: every block is
    /// complete, incomplete, missed or dropped to backpressure on the host.
    pub fn blocks_sent(&self) -> u64 {
        self.blocks_sent.load(Ordering::Relaxed)
    }

    /// What this camera did with `PACKETRESEND_CMD`s: requests received,
    /// packets sent again, packets it no longer held.
    pub fn resends(&self) -> ResendStats {
        self.resends.snapshot()
    }

    /// Datagrams sent from the [`spoof_source`](FakeCameraBuilder::spoof_source)
    /// address.
    pub fn spoofed_packets(&self) -> u64 {
        self.spoofed.load(Ordering::Relaxed)
    }

    /// Read `len` bytes of device state directly, bypassing GVCP.
    ///
    /// Not subject to access modes and not counted, so a test can check what a
    /// write actually stored without trusting the client's own read-back.
    pub async fn peek(&self, addr: u64, len: usize) -> Vec<u8> {
        self.regs.lock().await.read(addr, len)
    }

    /// The exposure saved in user set `set` by `UserSetSave`, or `None` if
    /// nothing has been saved there. Read past the protocol, like
    /// [`FakeCamera::peek`].
    pub async fn saved_exposure(&self, set: u32) -> Option<f64> {
        self.regs.lock().await.saved_exposure(set)
    }

    /// Stop the fake camera and wait for its background tasks to exit.
    ///
    /// Awaiting the `JoinHandle`s after `abort()` ensures the tokio tasks have
    /// dropped their `Arc<UdpSocket>` clones and the GVCP port is actually
    /// released before the call returns — otherwise a subsequent rebind on
    /// the same port can hit `EADDRINUSE`.
    pub async fn stop(mut self) {
        if let Some(h) = self.gvcp_handle.take() {
            h.abort();
            let _ = h.await;
        }
        if let Some(h) = self.gvsp_handle.take() {
            h.abort();
            let _ = h.await;
        }
    }
}

impl Drop for FakeCamera {
    /// Best-effort cleanup when the handle is dropped without calling `stop`.
    /// Does not wait for tasks to exit — use `stop().await` for that.
    fn drop(&mut self) {
        if let Some(h) = self.gvcp_handle.take() {
            h.abort();
        }
        if let Some(h) = self.gvsp_handle.take() {
            h.abort();
        }
    }
}

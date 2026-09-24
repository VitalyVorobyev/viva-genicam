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
mod gvsp_sender;
pub mod registers;

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

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
}

/// PFNC pixel format codes.
pub const MONO8: u32 = 0x0108_0001;
pub const RGB8: u32 = 0x0218_0014;

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

    /// Report a different `GevHeartbeatTimeout` than the 3 000 ms default.
    ///
    /// A shorter window keeps a test that has to wait one out from dominating
    /// the suite's runtime.
    pub fn heartbeat_timeout_ms(mut self, timeout_ms: u32) -> Self {
        self.heartbeat_timeout_ms = Some(timeout_ms);
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
        let regs = Arc::new(Mutex::new(register_map));

        let acq_start = Arc::new(Notify::new());
        let acq_stop_flag = Arc::new(AtomicBool::new(false));
        let counters = Arc::new(CommandCounters::default());

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
            };
            tokio::spawn(async move {
                gvcp_server::run(socket, regs, acq_start, acq_stop, bind_ip, options).await;
            })
        };

        // Spawn GVSP streaming task.
        let gvsp_handle = {
            let regs = regs.clone();
            let acq_start = acq_start.clone();
            let acq_stop = acq_stop_flag.clone();
            let fps = self.fps;
            tokio::spawn(async move {
                gvsp_sender::run(regs, acq_start, acq_stop, fps).await;
            })
        };

        Ok(FakeCamera {
            gvcp_handle: Some(gvcp_handle),
            gvsp_handle: Some(gvsp_handle),
            regs,
            counters,
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

    /// Read `len` bytes of device state directly, bypassing GVCP.
    ///
    /// Not subject to access modes and not counted, so a test can check what a
    /// write actually stored without trusting the client's own read-back.
    pub async fn peek(&self, addr: u64, len: usize) -> Vec<u8> {
        self.regs.lock().await.read(addr, len)
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

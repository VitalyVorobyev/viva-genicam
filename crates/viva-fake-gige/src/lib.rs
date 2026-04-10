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
//! camera.stop();
//! # }
//! ```

mod gvcp_server;
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

    /// Start the fake camera and return a handle.
    pub async fn build(self) -> Result<FakeCamera, std::io::Error> {
        let regs = Arc::new(Mutex::new(registers::RegisterMap::new(
            self.width,
            self.height,
            self.pixel_format,
        )));

        let acq_start = Arc::new(Notify::new());
        let acq_stop_flag = Arc::new(AtomicBool::new(false));

        // Bind GVCP control socket with SO_REUSEADDR to avoid TIME_WAIT issues.
        let bind_addr: std::net::SocketAddr =
            format!("{}:{}", self.bind_ip, self.port).parse().unwrap();
        let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        sock.set_reuse_address(true)?;
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
            tokio::spawn(async move {
                gvcp_server::run(socket, regs, acq_start, acq_stop, bind_ip).await;
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
            gvcp_handle,
            gvsp_handle,
            _regs: regs,
            local_addr,
        })
    }
}

/// Handle to a running fake GigE Vision camera.
///
/// The camera runs as background tokio tasks. Call [`stop`](FakeCamera::stop) or
/// drop the handle to shut down the camera.
pub struct FakeCamera {
    gvcp_handle: JoinHandle<()>,
    gvsp_handle: JoinHandle<()>,
    _regs: Arc<Mutex<registers::RegisterMap>>,
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

    /// Stop the fake camera by aborting its background tasks.
    pub fn stop(self) {
        self.gvcp_handle.abort();
        self.gvsp_handle.abort();
    }
}

impl Drop for FakeCamera {
    fn drop(&mut self) {
        self.gvcp_handle.abort();
        self.gvsp_handle.abort();
    }
}

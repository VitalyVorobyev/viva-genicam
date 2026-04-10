//! Per-device state wrapping `Camera<GigeRegisterIo>`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tracing::{debug, info};
use viva_genicam::gige::gvcp::consts as gvcp_consts;
use viva_genicam::gige::nic::Iface;
use viva_genicam::{
    Camera, FrameStream, GenicamError, GigeRegisterIo, StreamBuilder, connect_gige_with_xml, gige,
};

/// Transport-agnostic device operations used by shared Zenoh queryable handlers.
///
/// Implemented by [`DeviceHandle`] (GigE) and `U3vDeviceHandle` (USB3 Vision).
/// The `nodes` module and initial value publishing use only this trait.
#[async_trait::async_trait]
pub trait DeviceOps: Send + Sync + 'static {
    /// Unique device identifier (e.g. "cam-aabbccddeeff" for GigE).
    fn device_id(&self) -> &str;
    /// Raw GenICam XML fetched from the device.
    fn raw_xml(&self) -> &str;
    /// Read a feature value by name.
    async fn get_feature(&self, name: &str) -> Result<String, GenicamError>;
    /// Write a feature value by name.
    async fn set_feature(&self, name: &str, value: &str) -> Result<(), GenicamError>;
    /// Execute a command node.
    async fn exec_command(&self, name: &str) -> Result<(), GenicamError>;
}

/// GigE Vision device handle wrapping `Camera<GigeRegisterIo>`.
pub struct DeviceHandle {
    camera: Arc<Mutex<Camera<GigeRegisterIo>>>,
    raw_xml: String,
    device_id: String,
    info: gige::DeviceInfo,
    /// Network interface name for stream setup (e.g. "en0").
    iface_name: Option<String>,
    /// When true the heartbeat loop should skip pinging to avoid mutex
    /// contention during connection refresh (which replaces the camera).
    heartbeat_paused: AtomicBool,
}

impl DeviceHandle {
    /// Connect to a discovered device and return a handle.
    pub async fn connect(
        info: &gige::DeviceInfo,
        iface_name: Option<String>,
    ) -> Result<Self, GenicamError> {
        let (camera, xml) = connect_gige_with_xml(info).await?;
        let device_id = Self::derive_device_id(info);
        Ok(Self {
            camera: Arc::new(Mutex::new(camera)),
            raw_xml: xml,
            device_id,
            info: info.clone(),
            iface_name,
            heartbeat_paused: AtomicBool::new(false),
        })
    }

    fn derive_device_id(info: &gige::DeviceInfo) -> String {
        let mac = info
            .mac
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join("");
        format!("cam-{mac}")
    }

    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    pub fn raw_xml(&self) -> &str {
        &self.raw_xml
    }

    pub fn info(&self) -> &gige::DeviceInfo {
        &self.info
    }

    pub fn iface_name(&self) -> Option<&str> {
        self.iface_name.as_deref()
    }

    /// Pause the heartbeat loop so it skips pinging.
    pub fn pause_heartbeat(&self) {
        self.heartbeat_paused.store(true, Ordering::Release);
    }

    /// Resume the heartbeat loop after a pause.
    pub fn resume_heartbeat(&self) {
        self.heartbeat_paused.store(false, Ordering::Release);
    }

    /// Returns `true` while heartbeat pings should be skipped.
    pub fn is_heartbeat_paused(&self) -> bool {
        self.heartbeat_paused.load(Ordering::Acquire)
    }

    /// Build a GVSP stream using the CCP-holding device.
    ///
    /// This configures the stream channel registers (SCDA, SCPH, SCPS) on the
    /// device that owns Control Channel Privilege and binds the receiving UDP
    /// socket. The returned [`FrameStream`] is ready for frame reception.
    pub async fn build_stream(&self, iface: Iface) -> Result<FrameStream, GenicamError> {
        let cam = self.camera.clone();
        let handle = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            let cam = cam
                .lock()
                .map_err(|_| GenicamError::Transport("camera mutex poisoned".into()))?;
            let mut device = cam.transport().lock_device()?;
            handle.block_on(async {
                let stream = StreamBuilder::new(&mut device)
                    .iface(iface)
                    .auto_packet_size(true)
                    .build()
                    .await?;
                Ok(FrameStream::new(stream, None))
            })
        })
        .await
        .map_err(|e| GenicamError::Transport(e.to_string()))?
    }

    /// Refresh the control connection and replace the cached camera handle.
    ///
    /// The Aravis fake camera on macOS loopback can stop producing frames after
    /// a longer idle period even though register reads still succeed. Reopening
    /// the control connection immediately before stream setup restores the
    /// working state without changing the higher-level device identity.
    ///
    /// The heartbeat loop is paused while the swap happens to avoid the old
    /// socket holding the camera mutex (the old CCP is revoked once the new
    /// connection claims it, so the old heartbeat would retry for up to 2 s
    /// and starve the new connection's CCP timer).
    pub async fn refresh_connection(&self) -> Result<(), GenicamError> {
        // 1. Pause heartbeat so it does not contend for the mutex on the
        //    old (now CCP-revoked) socket while we create the new connection.
        self.pause_heartbeat();
        info!(
            device_id = self.device_id,
            "heartbeat paused for connection refresh"
        );

        let result = connect_gige_with_xml(&self.info).await;

        match result {
            Ok((camera, _xml)) => {
                // 2. Swap the camera handle.
                {
                    let mut slot = self
                        .camera
                        .lock()
                        .map_err(|_| GenicamError::Transport("camera mutex poisoned".into()))?;
                    *slot = camera;
                }

                // 3. Send an immediate heartbeat on the new socket to reset
                //    the camera's CCP timer before any other operations.
                if let Err(e) = self.heartbeat_ping().await {
                    info!(
                        device_id = self.device_id,
                        error = %e,
                        "immediate heartbeat after refresh failed (non-fatal)"
                    );
                }

                // 4. Resume heartbeat loop.
                self.resume_heartbeat();
                info!(
                    device_id = self.device_id,
                    "heartbeat resumed after connection refresh"
                );
                Ok(())
            }
            Err(e) => {
                // On failure, resume heartbeat with the old connection intact.
                self.resume_heartbeat();
                Err(e)
            }
        }
    }

    /// Send a heartbeat read to keep the control channel alive.
    ///
    /// GigE Vision cameras drop CCP after a timeout (~3 s on aravis fake camera)
    /// if no GVCP traffic is received. This reads the CCP register via GVCP
    /// READREG so Aravis updates its controller heartbeat timer.
    pub async fn heartbeat_ping(&self) -> Result<(), GenicamError> {
        let cam = self.camera.clone();
        let handle = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            let cam = cam
                .lock()
                .map_err(|_| GenicamError::Transport("mutex poisoned".into()))?;
            let mut device = cam.transport().lock_device()?;
            let privilege = handle
                .block_on(device.read_register(gvcp_consts::CONTROL_CHANNEL_PRIVILEGE as u32))
                .map_err(|e| GenicamError::Transport(e.to_string()))?;
            let controller_bits = gvcp_consts::CCP_CONTROL | gvcp_consts::CCP_EXCLUSIVE;
            if privilege & controller_bits == 0 {
                return Err(GenicamError::Transport(format!(
                    "control channel privilege lost (ccp=0x{privilege:08x})"
                )));
            }
            Ok(())
        })
        .await
        .map_err(|e| GenicamError::Transport(e.to_string()))?
    }

    /// Read a feature value via spawn_blocking.
    pub async fn get_feature(&self, name: &str) -> Result<String, GenicamError> {
        let cam = self.camera.clone();
        let name = name.to_string();
        tokio::task::spawn_blocking(move || {
            let cam = cam
                .lock()
                .map_err(|_| GenicamError::Transport("camera mutex poisoned".to_string()))?;
            cam.get(&name)
        })
        .await
        .map_err(|e| GenicamError::Transport(e.to_string()))?
    }

    /// Write a feature value via spawn_blocking.
    pub async fn set_feature(&self, name: &str, value: &str) -> Result<(), GenicamError> {
        let cam = self.camera.clone();
        let name = name.to_string();
        let value = value.to_string();
        tokio::task::spawn_blocking(move || {
            let mut cam = cam
                .lock()
                .map_err(|_| GenicamError::Transport("camera mutex poisoned".to_string()))?;
            cam.set(&name, &value)
        })
        .await
        .map_err(|e| GenicamError::Transport(e.to_string()))?
    }

    /// Execute a command node via spawn_blocking (commands are invoked via `set`).
    pub async fn exec_command(&self, name: &str) -> Result<(), GenicamError> {
        // Camera::set() dispatches Command nodes to exec_command internally.
        self.set_feature(name, "1").await
    }

    /// Read the model name from the camera (best-effort).
    #[allow(dead_code)]
    pub async fn model_name(&self) -> String {
        self.get_feature("DeviceModelName")
            .await
            .unwrap_or_else(|_| {
                self.info
                    .model
                    .clone()
                    .unwrap_or_else(|| "Unknown".to_string())
            })
    }

    /// Read the serial number from the camera (best-effort).
    #[allow(dead_code)]
    pub async fn serial_number(&self) -> String {
        match self.get_feature("DeviceSerialNumber").await {
            Ok(sn) if !sn.is_empty() => sn,
            _ => {
                debug!("DeviceSerialNumber not available, using device ID");
                self.device_id.clone()
            }
        }
    }
}

#[async_trait::async_trait]
impl DeviceOps for DeviceHandle {
    fn device_id(&self) -> &str {
        &self.device_id
    }

    fn raw_xml(&self) -> &str {
        &self.raw_xml
    }

    async fn get_feature(&self, name: &str) -> Result<String, GenicamError> {
        DeviceHandle::get_feature(self, name).await
    }

    async fn set_feature(&self, name: &str, value: &str) -> Result<(), GenicamError> {
        DeviceHandle::set_feature(self, name, value).await
    }

    async fn exec_command(&self, name: &str) -> Result<(), GenicamError> {
        DeviceHandle::exec_command(self, name).await
    }
}

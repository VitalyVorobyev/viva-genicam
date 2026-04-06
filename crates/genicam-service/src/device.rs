//! Per-device state wrapping `Camera<GigeRegisterIo>`.

use std::sync::{Arc, Mutex};

use genicam::gige::nic::Iface;
use genicam::{
    Camera, FrameStream, GenicamError, GigeRegisterIo, StreamBuilder, connect_gige_with_xml, gige,
};
use tracing::debug;

/// Wraps a connected camera with its raw XML and device identity.
pub struct DeviceHandle {
    camera: Arc<Mutex<Camera<GigeRegisterIo>>>,
    raw_xml: String,
    device_id: String,
    info: gige::DeviceInfo,
    /// Network interface name for stream setup (e.g. "en0").
    iface_name: Option<String>,
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

    /// Send a heartbeat read to keep the control channel alive.
    ///
    /// GigE Vision cameras drop CCP after a timeout (~3 s on aravis fake camera)
    /// if no GVCP traffic is received. This reads the CCP register as a keep-alive.
    pub async fn heartbeat_ping(&self) -> Result<(), GenicamError> {
        let cam = self.camera.clone();
        let handle = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            let cam = cam
                .lock()
                .map_err(|_| GenicamError::Transport("mutex poisoned".into()))?;
            let mut device = cam.transport().lock_device()?;
            handle
                .block_on(device.read_mem(0x0A00, 4))
                .map_err(|e| GenicamError::Transport(e.to_string()))?;
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

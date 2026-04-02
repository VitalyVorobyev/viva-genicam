//! Per-device state wrapping Camera<GigeRegisterIo>.

use std::sync::{Arc, Mutex};

use genicam::{connect_gige_with_xml, gige, Camera, GenicamError, GigeRegisterIo};
use tracing::debug;

/// Wraps a connected camera with its raw XML and device identity.
pub struct DeviceHandle {
    camera: Arc<Mutex<Camera<GigeRegisterIo>>>,
    raw_xml: String,
    device_id: String,
    info: gige::DeviceInfo,
}

impl DeviceHandle {
    /// Connect to a discovered device and return a handle.
    pub async fn connect(info: &gige::DeviceInfo) -> Result<Self, GenicamError> {
        let (camera, xml) = connect_gige_with_xml(info).await?;
        let device_id = Self::derive_device_id(info);
        Ok(Self {
            camera: Arc::new(Mutex::new(camera)),
            raw_xml: xml,
            device_id,
            info: info.clone(),
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

    /// Read a feature value via spawn_blocking.
    pub async fn get_feature(&self, name: &str) -> Result<String, GenicamError> {
        let cam = self.camera.clone();
        let name = name.to_string();
        tokio::task::spawn_blocking(move || {
            let cam = cam.lock().map_err(|_| {
                GenicamError::Transport("camera mutex poisoned".to_string())
            })?;
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
            let mut cam = cam.lock().map_err(|_| {
                GenicamError::Transport("camera mutex poisoned".to_string())
            })?;
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

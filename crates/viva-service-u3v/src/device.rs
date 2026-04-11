//! USB3 Vision device handle for the Zenoh service.

use std::sync::{Arc, Mutex};

use viva_fake_u3v::FakeU3vTransport;
use viva_genicam::{Camera, GenicamError, U3vRegisterIo};
use viva_service::device::DeviceOps;
use viva_u3v::stream::U3vStream;

/// Device handle for a USB3 Vision camera.
#[allow(dead_code)]
pub struct U3vDeviceHandle {
    camera: Arc<Mutex<Camera<U3vRegisterIo<FakeU3vTransport>>>>,
    raw_xml: String,
    device_id: String,
    /// Shared transport for streaming (same Arc used by the control channel).
    transport: Arc<FakeU3vTransport>,
    stream_ep: Option<u8>,
}

impl U3vDeviceHandle {
    /// Create a handle from a pre-built Camera + XML.
    pub fn new(
        camera: Camera<U3vRegisterIo<FakeU3vTransport>>,
        xml: String,
        device_id: String,
        transport: Arc<FakeU3vTransport>,
        stream_ep: Option<u8>,
    ) -> Self {
        Self {
            camera: Arc::new(Mutex::new(camera)),
            raw_xml: xml,
            device_id,
            transport,
            stream_ep,
        }
    }

    /// Open a U3V stream for frame reception.
    #[allow(dead_code)]
    pub fn open_stream(&self, payload_size: usize) -> Option<U3vStream<FakeU3vTransport>> {
        let ep = self.stream_ep?;
        Some(U3vStream::new(
            self.transport.clone(),
            ep,
            256, // max_leader_size
            256, // max_trailer_size
            payload_size,
        ))
    }
}

#[async_trait::async_trait]
impl DeviceOps for U3vDeviceHandle {
    fn device_id(&self) -> &str {
        &self.device_id
    }

    fn raw_xml(&self) -> &str {
        &self.raw_xml
    }

    async fn get_feature(&self, name: &str) -> Result<String, GenicamError> {
        let cam = self.camera.clone();
        let name = name.to_string();
        tokio::task::spawn_blocking(move || {
            let cam = cam
                .lock()
                .map_err(|_| GenicamError::Transport("camera mutex poisoned".into()))?;
            cam.get(&name)
        })
        .await
        .map_err(|e| GenicamError::Transport(e.to_string()))?
    }

    async fn set_feature(&self, name: &str, value: &str) -> Result<(), GenicamError> {
        let cam = self.camera.clone();
        let name = name.to_string();
        let value = value.to_string();
        tokio::task::spawn_blocking(move || {
            let mut cam = cam
                .lock()
                .map_err(|_| GenicamError::Transport("camera mutex poisoned".into()))?;
            cam.set(&name, &value)
        })
        .await
        .map_err(|e| GenicamError::Transport(e.to_string()))?
    }

    async fn exec_command(&self, name: &str) -> Result<(), GenicamError> {
        self.set_feature(name, "1").await
    }
}

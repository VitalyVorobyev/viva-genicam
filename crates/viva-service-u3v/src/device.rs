//! USB3 Vision device handle for the Zenoh service.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use viva_genicam::{Camera, GenicamError, U3vRegisterIo};
use viva_service::device::{DeviceOps, camera_feature_state};
use viva_u3v::stream::U3vStream;
use viva_u3v::usb::UsbTransfer;
use viva_zenoh_api::FeatureState;

/// Device handle for a USB3 Vision camera, generic over the USB transport.
pub struct U3vDeviceHandle<T: UsbTransfer + 'static> {
    camera: Arc<Mutex<Camera<U3vRegisterIo<T>>>>,
    raw_xml: String,
    device_id: String,
    /// Shared transport for streaming (same Arc used by the control channel).
    transport: Arc<T>,
    stream_ep: Option<u8>,
    /// Nodes a command executes through, from `NodeMap::command_targets`.
    /// A feature-state snapshot reports them without reading them.
    command_targets: Arc<HashSet<String>>,
}

impl<T: UsbTransfer + 'static> U3vDeviceHandle<T> {
    /// Create a handle from a pre-built Camera + XML.
    pub fn new(
        camera: Camera<U3vRegisterIo<T>>,
        xml: String,
        device_id: String,
        transport: Arc<T>,
        stream_ep: Option<u8>,
    ) -> Self {
        let command_targets = Arc::new(camera.nodemap().command_targets());
        Self {
            camera: Arc::new(Mutex::new(camera)),
            raw_xml: xml,
            device_id,
            transport,
            stream_ep,
            command_targets,
        }
    }

    /// Open a U3V stream for frame reception.
    pub fn open_stream(&self, payload_size: usize) -> Option<U3vStream<T>> {
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
impl<T: UsbTransfer + 'static> DeviceOps for U3vDeviceHandle<T> {
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

    /// The same typed NodeMap snapshot GigE serves: real kind, access mode,
    /// ranges and enum entries, and no read of a write-only node or of a
    /// command's register (backlog `SVC-02`).
    async fn get_feature_state(&self, name: &str) -> Result<FeatureState, String> {
        camera_feature_state(&self.camera, &self.command_targets, name).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use viva_fake_u3v::{FakeU3vTransport, REG_TRIGGER_SOFTWARE, WRITE_ONLY_REGISTERS};
    use viva_genicam::open_u3v_device;
    use viva_service::device::DeviceOps;
    use viva_u3v::device::U3vDevice;

    use super::U3vDeviceHandle;

    fn open_fake() -> (U3vDeviceHandle<FakeU3vTransport>, Arc<FakeU3vTransport>) {
        let transport = Arc::new(FakeU3vTransport::new(640, 480, 0x0108_0001));
        let device =
            U3vDevice::open(transport.clone(), 0x81, 0x01, Some(0x82), None).expect("open");
        let (camera, xml) = open_u3v_device(device).expect("camera");
        let handle = U3vDeviceHandle::new(
            camera,
            xml,
            "cam-fake-u3v".to_string(),
            transport.clone(),
            Some(0x82),
        );
        (handle, transport)
    }

    /// SVC-02: a U3V feature state is the typed one, not the default's
    /// `"RW"` / `"Unknown"` with no range.
    #[tokio::test]
    async fn a_feature_state_reports_kind_access_mode_and_range() {
        let (handle, _) = open_fake();

        let width = handle.get_feature_state("Width").await.expect("Width");
        assert_eq!(width.kind, "Integer");
        assert_eq!(width.access_mode, "RW");
        assert_eq!(width.value, serde_json::json!(640));
        let range = width.numeric.expect("an Integer reports its range");
        assert_eq!((range.min, range.max, range.inc), (1.0, 4096.0, Some(1.0)));

        let model = handle
            .get_feature_state("DeviceModelName")
            .await
            .expect("DeviceModelName");
        assert_eq!(model.access_mode, "RO");

        let format = handle
            .get_feature_state("PixelFormat")
            .await
            .expect("PixelFormat");
        assert_eq!(format.kind, "Enumeration");
        let entries = format.enum_available.expect("an Enumeration lists entries");
        assert!(entries.iter().any(|e| e == "Mono8"), "{entries:?}");
    }

    /// SVC-02: a snapshot of every node succeeds without reading a write-only
    /// register or a command's register. Asserted on the fake's own read log:
    /// the `RW` command register would serve a read without complaint, and a
    /// refused WO read could be swallowed on the way up.
    #[tokio::test]
    async fn a_snapshot_reads_neither_write_only_nodes_nor_command_registers() {
        let (handle, transport) = open_fake();
        let names: Vec<String> = {
            let camera = handle.camera.lock().unwrap();
            camera.nodemap().node_names().map(str::to_string).collect()
        };
        transport.clear_reads();

        for name in &names {
            handle
                .get_feature_state(name)
                .await
                .unwrap_or_else(|e| panic!("{name}: {e}"));
        }

        let key = handle
            .get_feature_state("ActionDeviceKey")
            .await
            .expect("ActionDeviceKey");
        assert_eq!(key.access_mode, "WO");
        assert!(key.value.is_null());
        assert!(key.numeric.is_some(), "a WO integer keeps its range");

        let trigger = handle
            .get_feature_state("TriggerSoftware")
            .await
            .expect("TriggerSoftware");
        assert_eq!(trigger.kind, "Command");
        assert!(trigger.value.is_null());

        assert!(
            !transport.reads().is_empty(),
            "the snapshot read nothing, so the log proves nothing"
        );
        for &reg in WRITE_ONLY_REGISTERS {
            assert!(
                !transport.was_read(reg),
                "read WO register {reg:#x}: {:x?}",
                transport.reads()
            );
        }
        assert!(
            !transport.was_read(REG_TRIGGER_SOFTWARE),
            "read the command register: {:x?}",
            transport.reads()
        );
    }
}

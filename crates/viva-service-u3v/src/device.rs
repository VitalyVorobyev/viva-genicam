//! A USB3 Vision camera the service serves.

use viva_genicam::report::{OpenCamera, ReportTarget};
use viva_genicam::{
    Camera, Event, EventConfiguration, EventSetupError, GenicamError, StreamBlock, StreamStats,
    U3vConnection, U3vEventStream, U3vFrameStream, U3vRegisterIo, U3vStreamOptions,
};
use viva_service::{BlockStream, DeviceOps, EventReceiver, SharedCamera, StreamSource};
use viva_u3v::UsbTransfer;
use viva_zenoh_api::DeviceAnnounce;

/// A USB3 Vision camera, generic over the USB transport so the same service
/// code runs against `FakeU3vTransport` and `NusbTransfer`.
pub(crate) struct U3vDeviceHandle<T: UsbTransfer + 'static> {
    camera: SharedCamera<U3vRegisterIo<T>>,
    /// Whether the camera is gone, readable without its lock.
    connection: U3vConnection,
    raw_xml: String,
    announce: DeviceAnnounce,
}

impl<T: UsbTransfer + Send + 'static> U3vDeviceHandle<T> {
    /// A handle on `camera`, published as `announce` says.
    pub(crate) fn new(
        camera: Camera<U3vRegisterIo<T>>,
        xml: String,
        announce: DeviceAnnounce,
    ) -> Self {
        Self {
            connection: camera.transport().connection(),
            camera: SharedCamera::new(camera),
            raw_xml: xml,
            announce,
        }
    }

    /// The camera's bootstrap registers, decoded and raw, for the report:
    /// USB3 Vision has met one camera so far (backlog TC-27).
    pub(crate) async fn bootstrap(&self) -> Vec<(String, String)> {
        self.camera
            .with(|camera| {
                let mut device = camera.transport().lock_device()?;
                Ok(device.describe())
            })
            .await
            .unwrap_or_default()
    }
}

#[async_trait::async_trait]
impl<T: UsbTransfer + Send + 'static> DeviceOps for U3vDeviceHandle<T> {
    type Transport = U3vRegisterIo<T>;

    fn device_id(&self) -> &str {
        &self.announce.id
    }

    fn raw_xml(&self) -> &str {
        &self.raw_xml
    }

    fn camera(&self) -> &SharedCamera<U3vRegisterIo<T>> {
        &self.camera
    }

    /// A register access or a stream on the camera found it disconnected:
    /// it is let go of even if a rescan finds a camera with its id — the
    /// same camera plugged in again, which must be opened afresh (UV-03).
    fn is_lost(&self) -> bool {
        self.connection.is_lost()
    }
}

#[async_trait::async_trait]
impl<T: UsbTransfer + Send + 'static> StreamSource for U3vDeviceHandle<T> {
    /// The stream as the library opens it: sized by the device's SIRM, with
    /// the transfer layout configured and streaming enabled. The service
    /// built a `U3vStream` itself once, which skipped the SIRM and sized a
    /// packed format's frames at one byte a pixel (backlog SVC-03).
    async fn open_stream(
        &self,
        _packet_size: Option<u32>,
    ) -> Result<Box<dyn BlockStream>, GenicamError> {
        let frames = self
            .camera
            .with(|camera| camera.open_stream(U3vStreamOptions::default()))
            .await?;
        Ok(Box::new(U3vBlocks(frames)))
    }

    /// The report cannot open the camera this service holds — a USB device
    /// admits one owner — so it describes it from what the service read
    /// (backlog DX-06).
    async fn report_target(&self) -> ReportTarget {
        let announce = &self.announce;
        let identity = [
            ("name", announce.name.clone()),
            ("model", announce.model.clone()),
            ("serial", announce.serial.clone()),
            (
                "manufacturer",
                announce
                    .manufacturer
                    .clone()
                    .unwrap_or_else(|| "-".to_string()),
            ),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .chain(self.bootstrap().await)
        .collect();
        ReportTarget::Open(OpenCamera {
            transport: "USB3 Vision".to_string(),
            identity,
            xml: self.raw_xml.clone(),
        })
    }

    fn service_name(&self) -> &'static str {
        "viva-service-u3v"
    }

    /// Notification on through `EventSelector`, then the event interface
    /// opened; an interface that will not open rolls the notifications back.
    async fn open_events(
        &self,
        events: Vec<String>,
    ) -> Result<(Box<dyn EventReceiver>, EventConfiguration), String> {
        let opened = self
            .camera
            .with(move |camera| {
                let names: Vec<&str> = events.iter().map(String::as_str).collect();
                let saved = match camera.configure_events_restorable(&names) {
                    Ok(saved) => saved,
                    Err(error) => return Ok(Err(viva_zenoh_api::setup_error_message(&error))),
                };
                Ok(match camera.open_event_stream() {
                    Ok(stream) => Ok((stream, saved)),
                    Err(source) => {
                        let restoration = camera.restore_events(saved);
                        Err(viva_zenoh_api::setup_error_message(&EventSetupError {
                            source,
                            restoration,
                        }))
                    }
                })
            })
            .await
            .map_err(|error| format!("Starting events failed: {error}"))??;
        let (stream, saved) = opened;
        Ok((Box::new(U3vEvents(stream)), saved))
    }

    fn events_carry_blocks(&self) -> bool {
        false
    }
}

/// A USB3 Vision event interface as the event loop reads it. Closing joins
/// its reader and restores the EventEnable word it found.
struct U3vEvents(U3vEventStream);

#[async_trait::async_trait]
impl EventReceiver for U3vEvents {
    async fn next_event(&self) -> Result<Event, GenicamError> {
        self.0.next().await
    }

    async fn close(self: Box<Self>) -> Result<(), GenicamError> {
        let U3vEvents(events) = *self;
        events.shutdown().await
    }
}

/// A USB3 Vision frame stream as the frame loop reads it. Reading is the
/// library's: a reader thread that waits out idle periods (trigger mode) and
/// is joined on [`BlockStream::close`].
struct U3vBlocks(U3vFrameStream);

#[async_trait::async_trait]
impl BlockStream for U3vBlocks {
    async fn next_block(&mut self) -> Result<Option<StreamBlock>, GenicamError> {
        Ok(self.0.next_frame().await?.map(StreamBlock::Image))
    }

    fn stats(&self) -> Option<StreamStats> {
        Some(self.0.stats())
    }

    async fn close(self: Box<Self>) {
        // The reader thread lets go of the endpoint before this returns.
        let U3vBlocks(frames) = *self;
        frames.shutdown().await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use viva_fake_u3v::{FakeU3vTransport, REG_TRIGGER_SOFTWARE, WRITE_ONLY_REGISTERS};
    use viva_genicam::open_u3v_device;
    use viva_service::DeviceOps;
    use viva_zenoh_api::DeviceAnnounce;

    use super::U3vDeviceHandle;

    fn open_fake() -> (U3vDeviceHandle<FakeU3vTransport>, Arc<FakeU3vTransport>) {
        let transport = Arc::new(FakeU3vTransport::new(640, 480, 0x0108_0001));
        let device = viva_fake_u3v::open(transport.clone()).expect("open");
        let (camera, xml) = open_u3v_device(device).expect("camera");
        let announce = DeviceAnnounce::new("cam-fake-u3v", "FakeU3V", "FakeU3V", "FAKE-001");
        let handle = U3vDeviceHandle::new(camera, xml, announce);
        (handle, transport)
    }

    /// The report describes the camera's bootstrap registers, decoded and as
    /// the raw bytes a USB3 Vision reporter's report would settle a
    /// disagreement with (TC-27).
    #[tokio::test(flavor = "multi_thread")]
    async fn the_report_carries_the_bootstrap_registers() {
        let (handle, _transport) = open_fake();
        let lines = handle.bootstrap().await;
        let value = |key: &str| {
            lines
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
                .unwrap_or_else(|| panic!("no {key} in {lines:?}"))
        };
        assert_eq!(value("GenCP version"), "1.2");
        assert_eq!(value("U3V version"), "1.0");
        assert_eq!(value("max command / ack transfer"), "4096 / 4096 bytes");
        // The SBRM's first bytes: U3V 1.0, little-endian.
        assert!(value("SBRM bytes").starts_with("00000100"), "{lines:?}");
        assert_eq!(value("SIRM bytes").len(), 2 * 0x30);
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
        let names: Vec<String> = handle
            .camera()
            .with(|camera| Ok(camera.nodemap().node_names().map(str::to_string).collect()))
            .await
            .expect("node names");
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

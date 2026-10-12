//! How the USB3 Vision service finds cameras: by USB enumeration, or, with
//! `--fake`, cameras in process.

use std::sync::{Arc, Mutex};

use viva_fake_u3v::FakeU3vTransport;
use viva_genicam::{GenicamError, open_u3v_device};
use viva_service::Discovery;
use viva_u3v::{RusbTransfer, U3vDeviceInfo};
use viva_zenoh_api::DeviceAnnounce;

use crate::device::U3vDeviceHandle;

/// USB enumeration of USB3 Vision devices, each pass.
pub(crate) struct UsbDiscovery;

#[async_trait::async_trait]
impl Discovery for UsbDiscovery {
    type Found = U3vDeviceInfo;
    type Device = U3vDeviceHandle<RusbTransfer>;

    async fn discover(&self) -> Result<Vec<U3vDeviceInfo>, GenicamError> {
        tokio::task::spawn_blocking(viva_u3v::discover)
            .await
            .map_err(|source| GenicamError::Task {
                during: "USB enumeration",
                source,
            })?
            .map_err(GenicamError::from)
    }

    fn announce(&self, found: &U3vDeviceInfo) -> DeviceAnnounce {
        let id = viva_zenoh_api::u3v_device_id(
            found.vendor_id,
            found.product_id,
            found.bus,
            found.address,
        );
        let model = found.model.clone().unwrap_or_else(|| "Unknown".to_string());
        let name = found
            .model
            .clone()
            .unwrap_or_else(|| "U3V Camera".to_string());
        // USB enumeration has no IP, MAC or user-defined name to offer, so the
        // serial is what tells two cameras of one model apart on screen.
        // Empty when the string descriptor could not be read, as
        // `DeviceAnnounce::serial` specifies.
        let serial = found.serial.clone().unwrap_or_default();
        let mut announce = DeviceAnnounce::new(id, name, model, serial);
        announce.manufacturer = found.manufacturer.clone();
        announce
    }

    async fn connect(
        &self,
        found: &U3vDeviceInfo,
    ) -> Result<U3vDeviceHandle<RusbTransfer>, GenicamError> {
        let announce = self.announce(found);
        let found = found.clone();
        let (camera, xml) =
            tokio::task::spawn_blocking(move || viva_genicam::connect_u3v_with_xml(&found))
                .await
                .map_err(|source| GenicamError::Task {
                    during: "open the USB3 Vision camera",
                    source,
                })??;
        Ok(U3vDeviceHandle::new(camera, xml, announce))
    }
}

/// One in-process fake camera.
#[derive(Debug, Clone)]
pub(crate) struct FakeSpec {
    /// The device id it is published under.
    pub(crate) id: String,
    /// Image width.
    pub(crate) width: u32,
    /// Image height.
    pub(crate) height: u32,
    /// PFNC pixel format code.
    pub(crate) pixel_format: u32,
}

/// `--fake`: cameras in process, the set a test may change between passes,
/// as cameras come and go on a USB bus.
#[derive(Clone, Default)]
pub(crate) struct FakeDiscovery {
    cameras: Arc<Mutex<Vec<FakeSpec>>>,
}

impl FakeDiscovery {
    /// Plug `camera` in: the next pass finds it.
    pub(crate) fn plug(&self, camera: FakeSpec) {
        self.cameras.lock().expect("fake list").push(camera);
    }

    /// Unplug the camera published as `id`: the next pass misses it.
    #[cfg(test)]
    pub(crate) fn unplug(&self, id: &str) {
        self.cameras
            .lock()
            .expect("fake list")
            .retain(|camera| camera.id != id);
    }
}

#[async_trait::async_trait]
impl Discovery for FakeDiscovery {
    type Found = FakeSpec;
    type Device = U3vDeviceHandle<FakeU3vTransport>;

    async fn discover(&self) -> Result<Vec<FakeSpec>, GenicamError> {
        Ok(self.cameras.lock().expect("fake list").clone())
    }

    fn announce(&self, found: &FakeSpec) -> DeviceAnnounce {
        DeviceAnnounce::new(found.id.clone(), "FakeU3V", "FakeU3V", found.id.clone())
    }

    async fn connect(
        &self,
        found: &FakeSpec,
    ) -> Result<U3vDeviceHandle<FakeU3vTransport>, GenicamError> {
        let transport = Arc::new(FakeU3vTransport::new(
            found.width,
            found.height,
            found.pixel_format,
        ));
        let device = viva_fake_u3v::open(transport)?;
        let (camera, xml) = open_u3v_device(device)?;
        Ok(U3vDeviceHandle::new(camera, xml, self.announce(found)))
    }
}

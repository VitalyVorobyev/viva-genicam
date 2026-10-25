//! A served camera: the [`DeviceOps`] the queryables use, over a camera
//! shared behind a mutex and driven from the blocking pool.

use std::sync::{Arc, Mutex};

use tracing::warn;
use viva_genicam::genapi::RegisterIo;
use viva_genicam::{COMMAND_TIMEOUT, Camera, GenicamError};
use viva_zenoh_api::FeatureState;

/// A camera shared between a service's tasks: every call takes the mutex on
/// the blocking pool, since register I/O blocks.
#[derive(Debug)]
pub struct SharedCamera<T: RegisterIo> {
    camera: Arc<Mutex<Camera<T>>>,
}

impl<T: RegisterIo> Clone for SharedCamera<T> {
    fn clone(&self) -> Self {
        Self {
            camera: Arc::clone(&self.camera),
        }
    }
}

impl<T: RegisterIo + Send + 'static> SharedCamera<T> {
    /// Share `camera`.
    pub fn new(camera: Camera<T>) -> Self {
        Self {
            camera: Arc::new(Mutex::new(camera)),
        }
    }

    /// Run `f` on the camera, on the blocking pool.
    pub async fn with<R, F>(&self, f: F) -> Result<R, GenicamError>
    where
        R: Send + 'static,
        F: FnOnce(&mut Camera<T>) -> Result<R, GenicamError> + Send + 'static,
    {
        let camera = Arc::clone(&self.camera);
        tokio::task::spawn_blocking(move || {
            let mut camera = camera
                .lock()
                .map_err(|_| GenicamError::Panicked("a thread holding the camera"))?;
            f(&mut camera)
        })
        .await
        .map_err(|source| GenicamError::Task {
            during: "a camera call",
            source,
        })?
    }
}

/// What the Zenoh queryables need of a served camera, GigE Vision and USB3
/// Vision alike: its identity, its XML, and its features, read and written
/// through [`Camera`].
///
/// Everything but the first three methods is provided, so the two services
/// answer every query the same way (backlog API-04).
#[async_trait::async_trait]
pub trait DeviceOps: Send + Sync + 'static {
    /// The camera's transport.
    type Transport: RegisterIo + Send + 'static;

    /// The id the camera is published under, such as `cam-deadbeefcafe`.
    fn device_id(&self) -> &str;

    /// The GenICam XML the camera serves.
    fn raw_xml(&self) -> &str;

    /// The camera.
    fn camera(&self) -> &SharedCamera<Self::Transport>;

    /// Whether the transport has reported the camera gone, such as a USB3
    /// Vision camera unplugged: [`run_discovery`](crate::run_discovery) then
    /// lets go of it even if the next pass finds a camera with its id. A
    /// transport that cannot tell says `false`, and the camera is lost when a
    /// discovery pass no longer finds it.
    fn is_lost(&self) -> bool {
        false
    }

    /// Read a feature as text.
    async fn get_feature(&self, name: &str) -> Result<String, GenicamError> {
        let name = name.to_string();
        self.camera().with(move |camera| camera.get(&name)).await
    }

    /// Write a feature from text.
    async fn set_feature(&self, name: &str, value: &str) -> Result<(), GenicamError> {
        let (name, value) = (name.to_string(), value.to_string());
        self.camera()
            .with(move |camera| camera.set(&name, &value))
            .await
    }

    /// Execute a command and wait for the camera to report it finished
    /// (GenICam `IsDone`, up to [`COMMAND_TIMEOUT`]): a `UserSetSave` is
    /// done when the caller hears back (backlog ST-48). One still busy at
    /// the timeout was executed all the same, so that is logged, not
    /// returned.
    async fn exec_command(&self, name: &str) -> Result<(), GenicamError> {
        let name = name.to_string();
        self.camera()
            .with(
                move |camera| match camera.execute_and_wait_blocking(&name, COMMAND_TIMEOUT) {
                    Err(GenicamError::CommandNotDone { name, waited }) => {
                        warn!(%name, ?waited, "command executed but not reported done");
                        Ok(())
                    }
                    other => other,
                },
            )
            .await
    }

    /// Start acquisition the facade's way, `TLParamsLocked` with it
    /// ([`Camera::acquisition_start`]).
    async fn acquisition_start(&self) -> Result<(), GenicamError> {
        self.camera()
            .with(|camera| camera.acquisition_start())
            .await
    }

    /// Stop acquisition, clearing `TLParamsLocked`
    /// ([`Camera::acquisition_stop`]).
    async fn acquisition_stop(&self) -> Result<(), GenicamError> {
        self.camera().with(|camera| camera.acquisition_stop()).await
    }

    /// The full live state of a feature in its wire form: value, access
    /// mode, kind, range, available entries, unit
    /// ([`Camera::feature_state`]). The state is computed by `viva-genapi`,
    /// which Viva Studio calls too, so the write-only and command-register
    /// rules cannot drift between them (backlog `SVC-02`, `ST-26`).
    async fn get_feature_state(&self, name: &str) -> Result<FeatureState, String> {
        let owned = name.to_string();
        self.camera()
            .with(move |camera| camera.feature_state(&owned).map(FeatureState::from))
            .await
            .map_err(|e| format!("Failed to read '{name}': {e}"))
    }

    /// The features a write to `name` may change, which a client should
    /// re-read ([`viva_genicam::genapi::NodeMap::invalidated_by`]). Empty if
    /// the camera cannot be reached.
    async fn invalidated_by(&self, name: &str) -> Vec<String> {
        let name = name.to_string();
        self.camera()
            .with(move |camera| Ok(camera.nodemap().invalidated_by(&name)))
            .await
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use viva_genicam::genapi::{GenApiError, NodeMap, RegisterIo};
    use viva_zenoh_api::FeatureState;

    /// A write-only integer, an integer delegating to a write-only register
    /// (the `ActionDeviceKey` shape from #112), a command reaching an `RW`
    /// register through `<pValue>` -- the shape 213 of the 490 command
    /// targets in the vendor corpus take -- and one ordinary feature.
    const XML: &str = r#"
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
            <Integer Name="Width">
                <Address>0x100</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Min>0</Min>
                <Max>4096</Max>
            </Integer>
            <Integer Name="ActionDeviceKey">
                <pValue>ActionDeviceKeyReg</pValue>
            </Integer>
            <IntReg Name="ActionDeviceKeyReg">
                <Address>0x400</Address>
                <Length>4</Length>
                <AccessMode>WO</AccessMode>
                <Sign>Unsigned</Sign>
                <Endianess>BigEndian</Endianess>
            </IntReg>
            <Integer Name="SoftwarePulse">
                <Address>0x200</Address>
                <Length>4</Length>
                <AccessMode>WO</AccessMode>
                <Min>0</Min>
                <Max>1</Max>
            </Integer>
            <Command Name="AcquisitionStart">
                <pValue>AcquisitionStartReg</pValue>
                <CommandValue>1</CommandValue>
            </Command>
            <IntReg Name="AcquisitionStartReg">
                <Address>0x300</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Sign>Unsigned</Sign>
                <Endianess>BigEndian</Endianess>
            </IntReg>
        </RegisterDescription>
    "#;

    /// Serves `Width` and records every read address, so a test can tell a
    /// read that never happened from one whose failure was swallowed.
    #[derive(Default)]
    struct RecordingIo {
        reads: RefCell<Vec<u64>>,
    }

    impl RegisterIo for RecordingIo {
        fn read(&self, addr: u64, len: usize) -> Result<Vec<u8>, GenApiError> {
            self.reads.borrow_mut().push(addr);
            let mut bytes = vec![0; len];
            if addr == 0x100 && len == 4 {
                bytes.copy_from_slice(&640u32.to_be_bytes());
            }
            Ok(bytes)
        }

        fn write(&self, _addr: u64, _data: &[u8]) -> Result<(), GenApiError> {
            Ok(())
        }
    }

    /// SVC-08 and ST-21: one snapshot over every node succeeds, the write-only
    /// node and the command register are reported without a value, and
    /// neither address is read. Checks the wire form the service publishes:
    /// `viva-genapi`'s state through `viva-zenoh-api`'s conversion.
    #[test]
    fn a_snapshot_reports_unreadable_nodes_without_reading_them() {
        let model = viva_genapi_xml::parse(XML).expect("parse");
        let nodemap = NodeMap::try_from_xml(model).expect("build nodemap");
        let io = RecordingIo::default();

        let names = [
            "Width",
            "ActionDeviceKey",
            "ActionDeviceKeyReg",
            "SoftwarePulse",
            "AcquisitionStart",
            "AcquisitionStartReg",
        ];
        let states: HashMap<&str, _> = names
            .into_iter()
            .map(|name| {
                let state = nodemap
                    .feature_state(name, &io)
                    .map(FeatureState::from)
                    .unwrap_or_else(|e| panic!("{name}: {e}"));
                (name, state)
            })
            .collect();

        assert_eq!(states["Width"].value, serde_json::json!(640));

        let pulse = &states["SoftwarePulse"];
        assert_eq!(pulse.access_mode, "WO");
        assert_eq!(pulse.kind, "Integer");
        assert!(pulse.value.is_null());
        assert!(pulse.numeric.is_some(), "a WO integer keeps its range");

        for name in ["ActionDeviceKey", "ActionDeviceKeyReg"] {
            let state = &states[name];
            assert_eq!(state.access_mode, "WO", "{name}");
            assert!(state.value.is_null(), "{name}");
        }

        let command = &states["AcquisitionStart"];
        assert_eq!(command.kind, "Command");
        assert_eq!(command.access_mode, "WO");
        assert!(command.value.is_null());

        let register = &states["AcquisitionStartReg"];
        assert_eq!(register.access_mode, "RW", "reported as declared");
        assert!(register.value.is_null());

        let reads = io.reads.borrow();
        assert!(!reads.contains(&0x200), "read the WO register: {reads:x?}");
        assert!(!reads.contains(&0x400), "read the WO register: {reads:x?}");
        assert!(
            !reads.contains(&0x300),
            "read the command register: {reads:x?}"
        );
    }
}

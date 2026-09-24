//! Per-device state wrapping `Camera<GigeRegisterIo>`.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tracing::{debug, warn};
use viva_genicam::genapi::{AccessMode, Node, NodeMap, RegisterIo, SkOutput};
use viva_genicam::gige::nic::Iface;
use viva_genicam::{
    Camera, FrameStream, GenicamError, GigeRegisterIo, StreamBuilder, connect_gige_with_xml, gige,
};
use viva_zenoh_api::{FeatureState, NumericRange};

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

    /// Read the full live state of a feature: value, access mode, kind, range,
    /// available enum entries, unit. Default implementation projects from
    /// [`DeviceOps::get_feature`] with `kind: "Unknown"` and no range/enum
    /// data; every handle that holds a [`Camera`] — GigE's [`DeviceHandle`]
    /// and `U3vDeviceHandle` — overrides it with [`camera_feature_state`].
    ///
    /// Transports that cannot introspect (e.g. remote Zenoh relays) keep the
    /// default implementation — the UI renders "range unknown" / falls back to
    /// static XML in that case rather than showing invented defaults.
    async fn get_feature_state(&self, name: &str) -> Result<FeatureState, String> {
        let value = self.get_feature(name).await.map_err(|e| format!("{e}"))?;
        Ok(FeatureState {
            value: serde_json::Value::String(value),
            access_mode: "RW".to_string(),
            kind: "Unknown".to_string(),
            is_implemented: true,
            is_available: true,
            numeric: None,
            enum_available: None,
            unit: None,
        })
    }
}

/// GigE Vision device handle wrapping `Camera<GigeRegisterIo>`.
pub struct DeviceHandle {
    camera: Arc<Mutex<Camera<GigeRegisterIo>>>,
    raw_xml: String,
    device_id: String,
    info: gige::DeviceInfo,
    /// Host interface for stream setup, already resolved.
    ///
    /// Held resolved rather than as a name: the round trip through a string
    /// is what let the receive path drift into resolving it a second way, and
    /// wrongly (backlog `SVC-06`).
    iface: Option<Iface>,
    /// Nodes a command executes through, from [`NodeMap::command_targets`].
    ///
    /// Computed once at connect: a snapshot consults it for every feature, and
    /// a reconnect in [`DeviceHandle::refresh_connection`] reopens the same
    /// device and therefore the same XML.
    command_targets: Arc<HashSet<String>>,
}

impl DeviceHandle {
    /// Connect to a discovered device and return a handle.
    pub async fn connect(
        info: &gige::DeviceInfo,
        iface: Option<Iface>,
    ) -> Result<Self, GenicamError> {
        let (camera, xml) = connect_gige_with_xml(info).await?;
        let device_id = Self::derive_device_id(info);
        let command_targets = Arc::new(camera.nodemap().command_targets());
        Ok(Self {
            camera: Arc::new(Mutex::new(camera)),
            raw_xml: xml,
            device_id,
            info: info.clone(),
            iface,
            command_targets,
        })
    }

    fn derive_device_id(info: &gige::DeviceInfo) -> String {
        viva_zenoh_api::gige_device_id(&info.mac)
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

    /// The host interface `--iface` selected, if the operator named one.
    pub fn iface(&self) -> Option<&Iface> {
        self.iface.as_ref()
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
                    .auto_packet_size()
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
    /// The keepalive needs no coordination here. It belongs to the transport, so
    /// the new connection brings its own and the old one retires when the swap
    /// below drops the camera that owned it — nothing has to be paused, and
    /// nothing can outlive the privilege it was protecting.
    pub async fn refresh_connection(&self) -> Result<(), GenicamError> {
        const MAX_RETRIES: u32 = 5;
        const BASE_DELAY: Duration = Duration::from_millis(500);
        const MAX_DELAY: Duration = Duration::from_secs(16);

        // 1. Retry connection with exponential backoff.
        let mut attempt = 0u32;
        let result = loop {
            attempt += 1;
            match connect_gige_with_xml(&self.info).await {
                Ok(pair) => break Ok(pair),
                Err(e) if attempt >= MAX_RETRIES => {
                    warn!(
                        device_id = self.device_id,
                        error = %e,
                        attempt,
                        "reconnect failed, giving up"
                    );
                    break Err(e);
                }
                Err(e) => {
                    let delay = BASE_DELAY
                        .saturating_mul(1 << (attempt - 1).min(5))
                        .min(MAX_DELAY);
                    warn!(
                        device_id = self.device_id,
                        error = %e,
                        attempt,
                        ?delay,
                        "reconnect failed, retrying"
                    );
                    tokio::time::sleep(delay).await;
                }
            }
        };

        // 2. Swap the camera handle. Dropping the old one retires its keepalive.
        let (camera, _xml) = result?;
        let mut slot = self
            .camera
            .lock()
            .map_err(|_| GenicamError::Transport("camera mutex poisoned".into()))?;
        *slot = camera;
        Ok(())
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

    /// Rich introspection for GigE devices: typed reads against the NodeMap
    /// populate the full [`FeatureState`] tuple. This is what lets remote-mode
    /// UIs show live ranges, filter enum dropdowns, and gate Apply/Execute on
    /// the actual device access mode rather than the hardcoded `"RW"` the
    /// default implementation returned.
    async fn get_feature_state(&self, name: &str) -> Result<FeatureState, String> {
        camera_feature_state(&self.camera, &self.command_targets, name).await
    }
}

/// [`build_feature_state`] for a camera shared behind a mutex, on the blocking
/// pool.
///
/// Generic over the transport so every [`DeviceOps`] implementation that holds
/// a [`Camera`] answers [`DeviceOps::get_feature_state`] the same way: GigE's
/// [`DeviceHandle`] and the USB3 Vision service's handle both call this, which
/// is what keeps the write-only and command-register rules from drifting apart
/// between transports (backlog `SVC-02`).
///
/// `command_targets` is [`NodeMap::command_targets`], computed once when the
/// camera is opened.
pub async fn camera_feature_state<T>(
    camera: &Arc<Mutex<Camera<T>>>,
    command_targets: &Arc<HashSet<String>>,
    name: &str,
) -> Result<FeatureState, String>
where
    T: RegisterIo + Send + 'static,
{
    let cam = camera.clone();
    let command_targets = command_targets.clone();
    let name = name.to_string();
    tokio::task::spawn_blocking(move || {
        let cam = cam
            .lock()
            .map_err(|_| "camera mutex poisoned".to_string())?;
        build_feature_state(cam.nodemap(), cam.transport(), &command_targets, &name)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Build a [`FeatureState`] snapshot using typed NodeMap reads.
///
/// Takes the nodemap and transport rather than a [`Camera`], so any transport
/// can use it. The service's Zenoh queryables call this to produce the
/// authoritative snapshot the UI consumes.
///
/// `command_targets` is [`NodeMap::command_targets`]: the nodes a command
/// writes through, which are reported but never read.
pub fn build_feature_state(
    nodemap: &NodeMap,
    transport: &dyn RegisterIo,
    command_targets: &HashSet<String>,
    name: &str,
) -> Result<FeatureState, String> {
    let node = nodemap
        .node(name)
        .ok_or_else(|| format!("Node '{name}' not found"))?;

    let kind = node.kind_name().to_string();

    // Resolve the live implementation/availability/access state. Each call
    // degrades to a permissive default on evaluation error so a single bad
    // predicate doesn't break the whole feature snapshot for the UI.
    let is_implemented = nodemap.is_implemented(name, transport).unwrap_or_else(|e| {
        tracing::warn!(%name, error = %e, "is_implemented eval failed");
        true
    });
    let is_available = nodemap.is_available(name, transport).unwrap_or_else(|e| {
        tracing::warn!(%name, error = %e, "is_available eval failed");
        true
    });
    // Read a value only from a node that has one to give. A write-only node
    // refuses the read -- `viva-genapi` refuses it locally, a device answers
    // ACCESS_DENIED -- and a command's backing register holds a trigger, not a
    // value; some devices even treat reading one as significant. Both used to
    // be read: the `?` below turned one `WO` node into a failed snapshot
    // (SVC-08, #135), and a command register was read for a value it does not
    // hold (ST-21, #112).
    // They are reported with their access mode and a null value instead, so
    // the UI still offers the write or the execute. Declarations decide, not
    // the live mode: an unavailable node reports `RO` but stays unreadable.
    let write_only = nodemap.is_write_only(name);
    let has_value = !write_only && !command_targets.contains(name);

    let effective = nodemap.effective_access_mode(name, transport).ok();
    let access_mode = match effective.or_else(|| node.access_mode()) {
        Some(AccessMode::RO) => "RO".to_string(),
        // Declared `RW` but delegating to a `WO` register: say what it is.
        Some(AccessMode::RW) if write_only => "WO".to_string(),
        Some(AccessMode::RW) => "RW".to_string(),
        Some(AccessMode::WO) => "WO".to_string(),
        None => "NA".to_string(),
    };

    let value = match node {
        _ if !has_value => serde_json::Value::Null,
        Node::Integer(_) => nodemap
            .get_integer(name, transport)
            .map(|v| serde_json::Value::Number(v.into()))
            .map_err(|e| format!("Failed to read integer '{name}': {e}"))?,
        Node::Float(_) => nodemap
            .get_float(name, transport)
            .map(f64_to_json)
            .map_err(|e| format!("Failed to read float '{name}': {e}"))?,
        Node::Enum(_) => nodemap
            .get_enum(name, transport)
            .map(serde_json::Value::String)
            .map_err(|e| format!("Failed to read enum '{name}': {e}"))?,
        Node::Boolean(_) => nodemap
            .get_bool(name, transport)
            .map(serde_json::Value::Bool)
            .map_err(|e| format!("Failed to read bool '{name}': {e}"))?,
        Node::String(_) => nodemap
            .get_string(name, transport)
            .map(serde_json::Value::String)
            .map_err(|e| format!("Failed to read string '{name}': {e}"))?,
        // Report the size, not the bytes. A `<Register>` can be very large --
        // the Micro-Epsilon scanCONTROL declares a 100 000-byte
        // `FileAccessBuffer` -- and this state is polled by the UI, so
        // base64-ing the payload into every refresh would be a poor trade for
        // a value nothing renders.
        Node::Register(reg) => serde_json::json!({
            "kind": "register",
            "length": reg.declared_len(),
        }),
        Node::SwissKnife(sk) => match sk.output {
            SkOutput::Float => nodemap
                .get_float(name, transport)
                .map(f64_to_json)
                .map_err(|e| format!("Failed to eval SwissKnife '{name}': {e}"))?,
            SkOutput::Integer => nodemap
                .get_integer(name, transport)
                .map(|v| serde_json::Value::Number(v.into()))
                .map_err(|e| format!("Failed to eval SwissKnife '{name}': {e}"))?,
        },
        Node::Converter(_) => nodemap
            .get_converter(name, transport)
            .map(f64_to_json)
            .map_err(|e| format!("Failed to eval Converter '{name}': {e}"))?,
        Node::IntConverter(_) => nodemap
            .get_int_converter(name, transport)
            .map(|v| serde_json::Value::Number(v.into()))
            .map_err(|e| format!("Failed to eval IntConverter '{name}': {e}"))?,
        Node::Command(_) | Node::Category(_) => serde_json::Value::Null,
        // `Node` is `#[non_exhaustive]`, so a node type added in `viva-genapi`
        // no longer breaks this build. Name the kind rather than emitting a
        // bare null, so an unhandled type stays legible in the UI and the log.
        other => serde_json::json!({ "kind": other.kind_name(), "unsupported": true }),
    };

    let (numeric, unit) = match node {
        Node::Integer(n) => {
            // When the XML defers bounds to runtime registers (`<pMin>` /
            // `<pMax>`), `n.min` / `n.max` are `i64::MIN` / `i64::MAX`
            // sentinels. Resolve the referenced nodes' current values so the
            // UI can render a real range. A failed pMin/pMax read falls back
            // to the static bound — the UI suppresses sentinel bleed-through.
            let resolved_min = n
                .p_min
                .as_deref()
                .and_then(|pm| nodemap.get_integer(pm, transport).ok())
                .unwrap_or(n.min);
            let resolved_max = n
                .p_max
                .as_deref()
                .and_then(|pm| nodemap.get_integer(pm, transport).ok())
                .unwrap_or(n.max);
            (
                Some(NumericRange {
                    min: resolved_min as f64,
                    max: resolved_max as f64,
                    inc: n.inc.map(|i| i as f64),
                }),
                n.unit.clone(),
            )
        }
        Node::Float(n) => (
            Some(NumericRange {
                min: n.min,
                max: n.max,
                inc: None,
            }),
            n.unit.clone(),
        ),
        _ => (None, None),
    };

    let enum_available = if matches!(node, Node::Enum(_)) {
        // Prefer the live predicate-filtered list; fall back to the static
        // entries if the predicates error out.
        nodemap
            .available_enum_entries(name, transport)
            .or_else(|e| {
                tracing::warn!(%name, error = %e, "available_enum_entries eval failed");
                nodemap.enum_entries(name).map_err(|e| format!("{e}"))
            })
            .ok()
    } else {
        None
    };

    Ok(FeatureState {
        value,
        access_mode,
        kind,
        is_implemented,
        is_available,
        numeric,
        enum_available,
        unit,
    })
}

fn f64_to_json(v: f64) -> serde_json::Value {
    serde_json::Number::from_f64(v)
        .map(serde_json::Value::Number)
        .unwrap_or_else(|| serde_json::Value::String(v.to_string()))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use viva_genicam::genapi::{GenApiError, NodeMap, RegisterIo};

    use super::build_feature_state;

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
    /// neither address is read.
    #[test]
    fn a_snapshot_reports_unreadable_nodes_without_reading_them() {
        let model = viva_genapi_xml::parse(XML).expect("parse");
        let nodemap = NodeMap::try_from_xml(model).expect("build nodemap");
        let targets = nodemap.command_targets();
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
                let state = build_feature_state(&nodemap, &io, &targets, name)
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

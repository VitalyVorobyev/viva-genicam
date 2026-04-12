//! Per-device state wrapping `Camera<GigeRegisterIo>`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tracing::{debug, info, warn};
use viva_genicam::genapi::{AccessMode, Node, SkOutput};
use viva_genicam::gige::gvcp::consts as gvcp_consts;
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
    /// [`get_feature`] with `kind: "Unknown"` and no range/enum data; GigE's
    /// [`DeviceHandle`] overrides with typed reads against the NodeMap.
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
        const MAX_RETRIES: u32 = 5;
        const BASE_DELAY: Duration = Duration::from_millis(500);
        const MAX_DELAY: Duration = Duration::from_secs(16);

        // 1. Pause heartbeat so it does not contend for the mutex on the
        //    old (now CCP-revoked) socket while we create the new connection.
        self.pause_heartbeat();
        info!(
            device_id = self.device_id,
            "heartbeat paused for connection refresh"
        );

        // 2. Retry connection with exponential backoff.
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

        match result {
            Ok((camera, _xml)) => {
                // 3. Swap the camera handle.
                {
                    let mut slot = self
                        .camera
                        .lock()
                        .map_err(|_| GenicamError::Transport("camera mutex poisoned".into()))?;
                    *slot = camera;
                }

                // 4. Send an immediate heartbeat on the new socket to reset
                //    the camera's CCP timer before any other operations.
                if let Err(e) = self.heartbeat_ping().await {
                    info!(
                        device_id = self.device_id,
                        error = %e,
                        "immediate heartbeat after refresh failed (non-fatal)"
                    );
                }

                // 5. Resume heartbeat loop.
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

    /// Rich introspection for GigE devices: typed reads against the NodeMap
    /// populate the full [`FeatureState`] tuple. This is what lets remote-mode
    /// UIs show live ranges, filter enum dropdowns, and gate Apply/Execute on
    /// the actual device access mode rather than the hardcoded `"RW"` the
    /// default implementation returned.
    async fn get_feature_state(&self, name: &str) -> Result<FeatureState, String> {
        let cam = self.camera.clone();
        let name = name.to_string();
        tokio::task::spawn_blocking(move || {
            let cam = cam
                .lock()
                .map_err(|_| "camera mutex poisoned".to_string())?;
            build_feature_state(&cam, &name)
        })
        .await
        .map_err(|e| e.to_string())?
    }
}

/// Build a [`FeatureState`] snapshot using typed NodeMap reads.
///
/// Shared between GigE (`DeviceHandle`) and any other transport that wraps
/// [`Camera<GigeRegisterIo>`]. The service's Zenoh queryables call this to
/// produce the authoritative snapshot the UI consumes.
fn build_feature_state(
    camera: &Camera<GigeRegisterIo>,
    name: &str,
) -> Result<FeatureState, String> {
    let nodemap = camera.nodemap();
    let node = nodemap
        .node(name)
        .ok_or_else(|| format!("Node '{name}' not found"))?;

    let kind = node.kind_name().to_string();
    let transport = camera.transport();

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
    let effective = nodemap.effective_access_mode(name, transport).ok();
    let access_mode = match effective.or_else(|| node.access_mode()) {
        Some(AccessMode::RO) => "RO".to_string(),
        Some(AccessMode::RW) => "RW".to_string(),
        Some(AccessMode::WO) => "WO".to_string(),
        None => "NA".to_string(),
    };

    let value = match node {
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
                camera.enum_entries(name).map_err(|e| format!("{e}"))
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

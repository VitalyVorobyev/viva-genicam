//! In-process fake cameras — Python bindings over `viva-fake-gige` and
//! `viva-fake-u3v`.
//!
//! Lets users exercise the full bindings (discovery, control, streaming)
//! without any hardware and without shelling out to a binary, so
//! `pip install viva-genicam` is self-sufficient.
//!
//! The fake also reports its own state — who holds control, what a register
//! holds, how many software triggers and action commands it acted on — so a
//! test can check what reached the device rather than what the library says
//! it did.

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::time::Duration;

use pyo3::prelude::*;
use pyo3::types::PyBytes;
use viva_fake_gige as regs;
use viva_fake_gige::{FakeCamera, GvcpCommand, PacketScript, ScriptStep, MONO8, RGB8};
use viva_genicam::gige;
use viva_genicam::gige::consts as bootstrap;
use viva_pfnc::PixelFormat;

use std::sync::Arc;

use viva_fake_u3v::FakeU3vTransport;

use crate::camera::{open_u3v, PyCamera, UsbLink};
use crate::discovery::PyGigeDeviceInfo;
use crate::errors::{parse_error, transport_error, IntoPyErr};
use crate::runtime::runtime;

/// The PFNC code of `name`: any format `viva_pfnc` names, plus the lower-case
/// spellings this binding has always accepted.
fn pixel_format_code(name: &str) -> PyResult<u32> {
    match name {
        "mono8" => return Ok(MONO8),
        "rgb8" => return Ok(RGB8),
        _ => {}
    }
    match PixelFormat::from_name(name) {
        PixelFormat::Unknown(_) => Err(parse_error(format!(
            "unknown pixel_format '{name}'; expected a PFNC name such as 'Mono8', \
             'Mono12p' or 'RGB8'"
        ))),
        format => Ok(format.code()),
    }
}

/// How to build the fake beyond its geometry.
#[derive(Clone, Default)]
struct Behaviour {
    packet_resend: bool,
    drop_packets: Vec<(u64, u32)>,
    delay_ack: Vec<(u64, Duration)>,
}

#[pyclass(name = "FakeGigeCamera", module = "viva_genicam._native")]
pub(crate) struct PyFakeGigeCamera {
    width: u32,
    height: u32,
    fps: u32,
    bind_ip: Ipv4Addr,
    port: u16,
    pixel_format: u32,
    behaviour: Behaviour,
    handle: Option<FakeCamera>,
}

impl PyFakeGigeCamera {
    fn running(&self) -> PyResult<&FakeCamera> {
        self.handle.as_ref().ok_or_else(|| {
            transport_error("fake camera is not running; call start() or use as a context manager")
        })
    }
}

#[pymethods]
impl PyFakeGigeCamera {
    /// The device key the fake answers action commands for.
    #[classattr]
    const DEVICE_KEY: u32 = viva_fake_gige::FAKE_DEVICE_KEY;

    /// The group key the fake answers action commands for.
    #[classattr]
    const GROUP_KEY: u32 = viva_fake_gige::FAKE_GROUP_KEY;

    /// The fake's group mask: an action command's mask must share a bit.
    #[classattr]
    const GROUP_MASK: u32 = viva_fake_gige::FAKE_GROUP_MASK;

    /// The fake's MAC address, as discovery reports it.
    #[classattr]
    #[allow(non_snake_case)]
    fn MAC() -> String {
        viva_fake_gige::FAKE_MAC
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(":")
    }

    /// Register addresses of the fake's register map, by the feature or
    /// bootstrap register they hold.
    #[classattr]
    #[allow(non_snake_case)]
    fn REGISTERS() -> HashMap<&'static str, u64> {
        HashMap::from([
            ("Width", regs::REG_WIDTH),
            ("Height", regs::REG_HEIGHT),
            ("OffsetX", regs::REG_OFFSET_X),
            ("OffsetY", regs::REG_OFFSET_Y),
            ("PixelFormat", regs::REG_PIXEL_FORMAT),
            ("AcquisitionMode", regs::REG_ACQ_MODE),
            ("AcquisitionFrameRate", regs::REG_ACQ_FRAME_RATE),
            (
                "AcquisitionFrameRateEnable",
                regs::REG_ACQ_FRAME_RATE_ENABLE,
            ),
            ("ExposureTime", regs::REG_EXPOSURE_TIME),
            ("ExposureAuto", regs::REG_EXPOSURE_AUTO),
            ("Gain", regs::REG_GAIN),
            ("SensorType", regs::REG_SENSOR_TYPE),
            ("BinningHorizontal", regs::REG_BINNING_HORIZONTAL),
            ("BinningVertical", regs::REG_BINNING_VERTICAL),
            ("TriggerSelector", regs::REG_TRIGGER_SELECTOR),
            ("TriggerMode", regs::REG_TRIGGER_MODE),
            ("TriggerSource", regs::REG_TRIGGER_SOURCE),
            ("UserSetSelector", regs::REG_USER_SET_SELECTOR),
            ("UserSetDefault", regs::REG_USER_SET_DEFAULT),
            ("ChunkModeActive", regs::REG_CHUNK_MODE_ACTIVE),
            ("DeviceTemperature", regs::REG_DEVICE_TEMPERATURE),
            ("DeviceModelName", regs::REG_DEVICE_MODEL_NAME),
            ("DeviceUserID", regs::REG_DEVICE_USER_ID),
            ("DeviceUserBuffer", regs::REG_DEVICE_USER_BUFFER),
            ("CCP", bootstrap::CONTROL_CHANNEL_PRIVILEGE),
            ("CurrentIPConfiguration", bootstrap::CURRENT_IP_CONFIG),
            ("PersistentIPAddress", bootstrap::PERSISTENT_IP_ADDRESS),
            ("PersistentSubnetMask", bootstrap::PERSISTENT_SUBNET_MASK),
            (
                "PersistentDefaultGateway",
                bootstrap::PERSISTENT_DEFAULT_GATEWAY,
            ),
        ])
    }

    #[new]
    #[pyo3(signature = (
        width = 640,
        height = 480,
        fps = 30,
        bind_ip = "127.0.0.1",
        port = 3956,
        pixel_format = "Mono8",
        packet_resend = true,
        drop_packets = None,
        delay_ack = None,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        width: u32,
        height: u32,
        fps: u32,
        bind_ip: &str,
        port: u16,
        pixel_format: &str,
        packet_resend: bool,
        drop_packets: Option<Vec<(u64, u32)>>,
        delay_ack: Option<HashMap<u64, f64>>,
    ) -> PyResult<Self> {
        let ip: Ipv4Addr = bind_ip
            .parse()
            .map_err(|e| parse_error(format!("invalid bind_ip '{bind_ip}': {e}")))?;
        let code = pixel_format_code(pixel_format)?;
        let delay_ack = delay_ack
            .unwrap_or_default()
            .into_iter()
            .map(|(addr, secs)| {
                Duration::try_from_secs_f64(secs)
                    .map(|delay| (addr, delay))
                    .map_err(|_| {
                        parse_error("delay_ack: a delay is a non-negative number of seconds")
                    })
            })
            .collect::<PyResult<Vec<_>>>()?;
        Ok(Self {
            width,
            height,
            fps,
            bind_ip: ip,
            port,
            pixel_format: code,
            behaviour: Behaviour {
                packet_resend,
                drop_packets: drop_packets.unwrap_or_default(),
                delay_ack,
            },
            handle: None,
        })
    }

    /// Bind the UDP sockets and spawn the GVCP/GVSP tasks.
    ///
    /// Idempotent: calling `start` on an already-started camera is a no-op.
    fn start(&mut self, py: Python<'_>) -> PyResult<()> {
        if self.handle.is_some() {
            return Ok(());
        }
        let width = self.width;
        let height = self.height;
        let fps = self.fps;
        let bind_ip = self.bind_ip;
        let port = self.port;
        let pixel_format = self.pixel_format;
        let behaviour = self.behaviour.clone();
        let built = py.detach(|| {
            runtime().block_on(async move {
                let script = behaviour
                    .drop_packets
                    .iter()
                    .fold(PacketScript::new(), |script, &(block, packet)| {
                        script.step(ScriptStep::Drop { block, packet })
                    });
                let mut builder = FakeCamera::builder()
                    .width(width)
                    .height(height)
                    .fps(fps)
                    .bind_ip(bind_ip)
                    .port(port)
                    .pixel_format(pixel_format)
                    .packet_resend(behaviour.packet_resend)
                    .packet_script(script);
                for &(addr, delay) in &behaviour.delay_ack {
                    builder = builder.delay_ack(addr, delay);
                }
                builder.build().await
            })
        });
        let cam = built.map_err(|e| transport_error(format!("fake camera: {e}")))?;
        self.port = cam.local_addr().port();
        self.handle = Some(cam);
        Ok(())
    }

    /// Abort the background tasks and release the sockets.
    ///
    /// Blocks until the tokio tasks have actually exited so the GVCP port
    /// is released by the time this returns; otherwise a quick back-to-back
    /// `start()` on the same port can hit `EADDRINUSE`.
    fn stop(&mut self, py: Python<'_>) {
        if let Some(cam) = self.handle.take() {
            py.detach(|| runtime().block_on(cam.stop()));
        }
    }

    /// IP the GVCP socket is bound to.
    #[getter]
    fn ip(&self) -> String {
        self.bind_ip.to_string()
    }

    /// Port the GVCP socket is bound to. Reflects the actual port
    /// assigned by the OS if `port=0` was requested.
    #[getter]
    fn port(&self) -> u16 {
        self.port
    }

    /// Run discovery against the loopback address and return the
    /// matching `GigeDeviceInfo`.
    ///
    /// Raises `TransportError` if the camera isn't running or doesn't
    /// respond within `timeout_ms`.
    #[pyo3(signature = (timeout_ms = 1500))]
    fn device_info(&self, py: Python<'_>, timeout_ms: u64) -> PyResult<PyGigeDeviceInfo> {
        self.running()?;
        let target_ip = self.bind_ip;
        let devices = py.detach(|| {
            runtime().block_on(async move {
                gige::discover_all(Duration::from_millis(timeout_ms)).await
            })
        });
        let devices = devices.map_err(|e| transport_error(e.to_string()))?;
        devices
            .into_iter()
            .find(|d| d.ip == target_ip)
            .map(PyGigeDeviceInfo::from)
            .ok_or_else(|| {
                transport_error(format!(
                    "fake camera at {target_ip} did not reply to discovery within {timeout_ms} ms"
                ))
            })
    }

    /// The application holding control privilege, as `"ip:port"`, or `None`.
    fn controller(&self, py: Python<'_>) -> PyResult<Option<String>> {
        let cam = self.running()?;
        Ok(py
            .detach(|| runtime().block_on(cam.controller()))
            .map(|addr| addr.to_string()))
    }

    /// Give control privilege up on the device side, as a camera does when
    /// its controller's heartbeat expires.
    fn revoke_control(&self, py: Python<'_>) -> PyResult<()> {
        let cam = self.running()?;
        py.detach(|| runtime().block_on(cam.revoke_control()));
        Ok(())
    }

    /// `length` bytes of device memory at `address`, read past the protocol:
    /// what a write actually stored.
    fn peek<'py>(
        &self,
        py: Python<'py>,
        address: u64,
        length: usize,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let cam = self.running()?;
        let bytes = py.detach(|| runtime().block_on(cam.peek(address, length)));
        Ok(PyBytes::new(py, &bytes))
    }

    /// How many times `TriggerSoftware` was executed, counted by the device.
    fn software_triggers(&self, py: Python<'_>) -> PyResult<u64> {
        let cam = self.running()?;
        Ok(py.detach(|| runtime().block_on(cam.software_triggers())))
    }

    /// The exposure `UserSetSave` stored in user set `user_set` (0 is
    /// `Default`), or `None` if nothing was saved there.
    fn saved_exposure(&self, py: Python<'_>, user_set: u32) -> PyResult<Option<f64>> {
        let cam = self.running()?;
        Ok(py.detach(|| runtime().block_on(cam.saved_exposure(user_set))))
    }

    /// Action commands addressed to this camera that it acted on.
    fn actions(&self) -> PyResult<u64> {
        Ok(self.running()?.actions())
    }

    /// GVSP blocks the camera has produced, any it then dropped included.
    fn blocks_sent(&self) -> PyResult<u64> {
        Ok(self.running()?.blocks_sent())
    }

    /// Register writes received (WRITEREG and WRITEMEM), from anyone.
    fn writes(&self) -> PyResult<u64> {
        let commands = self.running()?.commands();
        Ok(commands.total(GvcpCommand::WriteReg) + commands.total(GvcpCommand::WriteMem))
    }

    fn __enter__(mut slf: PyRefMut<'_, Self>, py: Python<'_>) -> PyResult<Py<Self>> {
        slf.start(py)?;
        Ok(slf.into())
    }

    fn __exit__(
        &mut self,
        py: Python<'_>,
        _exc_type: Py<PyAny>,
        _exc: Py<PyAny>,
        _tb: Py<PyAny>,
    ) -> PyResult<bool> {
        self.stop(py);
        Ok(false)
    }

    fn __repr__(&self) -> String {
        let state = if self.handle.is_some() {
            "running"
        } else {
            "stopped"
        };
        format!(
            "FakeGigeCamera({state}, {}x{} @ {} fps on {}:{}, pixel_format=0x{:08x})",
            self.width, self.height, self.fps, self.bind_ip, self.port, self.pixel_format,
        )
    }
}

/// An in-process USB3 Vision camera: the `viva-fake-u3v` device, whose
/// registers and GenCP and stream protocol are laid out from the standard.
/// There is no USB enumeration to find it on; `connect` opens it.
#[pyclass(name = "FakeU3vCamera", module = "viva_genicam._native")]
pub(crate) struct PyFakeU3vCamera {
    transport: Arc<FakeU3vTransport>,
}

#[pymethods]
impl PyFakeU3vCamera {
    #[new]
    #[pyo3(signature = (width = 640, height = 480, pixel_format = "Mono8"))]
    fn new(width: u32, height: u32, pixel_format: &str) -> PyResult<Self> {
        let code = pixel_format_code(pixel_format)?;
        Ok(Self {
            transport: Arc::new(FakeU3vTransport::new(width, height, code)),
        })
    }

    /// Open the camera, as `connect_u3v` opens one on USB.
    fn connect(&self, py: Python<'_>) -> PyResult<PyCamera> {
        let link = Arc::new(UsbLink::Fake(Arc::clone(&self.transport)));
        py.detach(|| {
            viva_fake_u3v::open(link)
                .map_err(|source| viva_genicam::GenicamError::U3v {
                    during: "open the device",
                    source,
                })
                .and_then(open_u3v)
        })
        .into_py_err()
    }

    /// Whether the camera is acquiring: `AcquisitionStart` reached it and
    /// `AcquisitionStop` has not.
    fn acquiring(&self) -> bool {
        self.transport.acquiring()
    }

    /// `length` bytes of the camera's registers at `address`, as it holds
    /// them.
    fn peek<'py>(&self, py: Python<'py>, address: u64, length: usize) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.transport.peek(address, length))
    }

    fn __repr__(&self) -> String {
        "FakeU3vCamera()".to_owned()
    }
}

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyFakeGigeCamera>()?;
    m.add_class::<PyFakeU3vCamera>()?;
    Ok(())
}

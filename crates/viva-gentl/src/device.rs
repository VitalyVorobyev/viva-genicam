//! The Device module (GenTL 1.6 §2.2.3, §3.4) and the remote device's port
//! (§4.1.1.1).
//!
//! The Device module is the producer's proxy for one camera: its own port
//! serves the module's XML; the remote port, from `DevGetPort`, is the
//! camera's register map, reached through the facade's `GigeRegisterIo` —
//! which also keeps control privilege alive while the device is open and
//! gives it back when it closes.

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use viva_genicam::GigeRegisterIo;
use viva_genicam::genapi::RegisterIo;
use viva_genicam::gige::{self, DeviceInfo, GigeDevice, GigeError};

use crate::abi::*;
use crate::error::{
    Direction, Failure, Result, from_genapi, invalid_handle, not_available, not_implemented,
    poisoned,
};
use crate::interface::{device_id, device_info};
use crate::library::{Library, Object};
use crate::out::Info;
use crate::regmap::{
    ACCESS_STATUS_ENTRIES, Feature, Features, Kind, Layout, Register, Repr, TL_TYPE_ENTRIES, Value,
};
use crate::stream::Stream;

/// How the device was opened (`DEVICE_ACCESS_FLAGS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Access {
    /// `DEVICE_ACCESS_READONLY`: a monitor; nothing claimed, writes refused.
    ReadOnly,
    /// `DEVICE_ACCESS_CONTROL`: GigE Vision control access.
    Control,
    /// `DEVICE_ACCESS_EXCLUSIVE`: GigE Vision exclusive access.
    Exclusive,
}

/// The one data stream's ID.
pub(crate) const STREAM_ID: &str = "Stream0";

/// The Device module.
pub(crate) struct Device {
    pub(crate) key: usize,
    pub(crate) interface_key: usize,
    /// The remote port's key: a child of this device.
    pub(crate) remote_key: usize,
    pub(crate) info: DeviceInfo,
    pub(crate) access: Access,
    /// `None` once the device is closed. Calls in flight hold their own
    /// clone, so the camera is released when the last of them finishes.
    io: RwLock<Option<Arc<GigeRegisterIo>>>,
    /// Data streams the camera has: GigE Vision stream channels, of which
    /// the producer serves the first.
    streams: u32,
    tick_frequency: Option<u64>,
    open_stream: Mutex<Option<usize>>,
    stream_selector: Mutex<usize>,
}

impl Device {
    /// Open the camera `info` names with `access`: claim it (unless
    /// read-only) and start the keepalive that keeps it.
    pub(crate) fn open(
        lib: &Library,
        key: usize,
        interface_key: usize,
        info: DeviceInfo,
        access: Access,
    ) -> Result<Self> {
        let address = SocketAddr::new(IpAddr::V4(info.ip), gige::GVCP_PORT);
        let opened = lib.runtime.block_on(async {
            let mut device = GigeDevice::open(address).await?;
            if access != Access::ReadOnly {
                device.set_exclusive_access(access == Access::Exclusive);
                device.claim_control().await?;
                if let Err(err) = device.enable_pending_ack().await {
                    tracing::debug!(error = %err, "PENDING_ACK not enabled");
                }
            }
            // A camera that does not say how many stream channels it has is
            // given the benefit of the doubt: GigE Vision requires one.
            let streams = device.stream_channel_count().await.unwrap_or(1).min(1);
            let tick_frequency = device
                .timestamp_tick_frequency()
                .await
                .ok()
                .filter(|&f| f > 0);
            Ok::<_, GigeError>((device, streams, tick_frequency))
        });
        let (device, streams, tick_frequency) = opened.map_err(|err| open_failure(&err, &info))?;
        let io = match access {
            Access::ReadOnly => GigeRegisterIo::monitor(lib.runtime.clone(), device),
            Access::Control | Access::Exclusive => GigeRegisterIo::new(lib.runtime.clone(), device),
        };
        Ok(Self {
            key,
            interface_key,
            remote_key: lib.reserve(),
            info,
            access,
            io: RwLock::new(Some(Arc::new(io))),
            streams,
            tick_frequency,
            open_stream: Mutex::new(None),
            stream_selector: Mutex::new(0),
        })
    }

    /// The camera's transport, while the device is open.
    pub(crate) fn io(&self) -> Result<Arc<GigeRegisterIo>> {
        self.io
            .read()
            .map_err(poisoned)?
            .clone()
            .ok_or_else(|| invalid_handle("the device is closed"))
    }

    /// Close: release the camera (once no call holds it any more).
    pub(crate) fn shut_down(&self) {
        if let Ok(mut io) = self.io.write() {
            io.take();
        }
    }

    pub(crate) fn id(&self) -> String {
        device_id(&self.info)
    }

    pub(crate) fn access_status(&self) -> i32 {
        match self.access {
            Access::ReadOnly => DEVICE_ACCESS_STATUS_OPEN_READONLY,
            Access::Control | Access::Exclusive => DEVICE_ACCESS_STATUS_OPEN_READWRITE,
        }
    }

    /// `DevGetInfo`: in sync with `IFGetDeviceInfo` (§6.3.4.2).
    pub(crate) fn info(&self, cmd: DeviceInfoCmd) -> Result<Info> {
        match cmd {
            DEVICE_INFO_TIMESTAMP_FREQUENCY => self
                .tick_frequency
                .map(Info::U64)
                .ok_or_else(|| not_available("the camera reports no timestamp tick frequency")),
            _ => device_info(&self.info, cmd, self.access_status()),
        }
    }

    pub(crate) fn tick_frequency(&self) -> Option<u64> {
        self.tick_frequency
    }

    pub(crate) fn stream_count(&self) -> u32 {
        self.streams
    }

    /// `DevGetDataStreamID`.
    pub(crate) fn stream_id(&self, index: u32) -> Result<&'static str> {
        if self.streams == 0 {
            return Err(not_implemented("the camera has no stream channel"));
        }
        if index >= self.streams {
            return Err(Failure::new(
                GC_ERR_INVALID_INDEX,
                format!("stream {index} of {}", self.streams),
            ));
        }
        Ok(STREAM_ID)
    }

    /// `DevOpenDataStream`.
    pub(crate) fn open_stream(self: &Arc<Self>, lib: &Library, id: &str) -> Result<usize> {
        if self.streams == 0 {
            return Err(not_implemented("the camera has no stream channel"));
        }
        if id != STREAM_ID {
            return Err(Failure::new(
                GC_ERR_INVALID_ID,
                format!("no data stream '{id}'"),
            ));
        }
        let mut open = self.open_stream.lock().map_err(poisoned)?;
        if open.is_some() {
            return Err(Failure::new(
                GC_ERR_RESOURCE_IN_USE,
                "the data stream is already open",
            ));
        }
        let key = lib.reserve();
        let stream = Arc::new(Stream::new(key, self.clone()));
        lib.insert_at(key, Some(self.key), Object::Stream(stream))?;
        *open = Some(key);
        Ok(key)
    }

    /// A child stream closed: it opens again.
    pub(crate) fn child_closed(&self, key: usize) {
        if let Ok(mut open) = self.open_stream.lock()
            && *open == Some(key)
        {
            *open = None;
        }
    }

    /// `GCReadPort` on the remote port. GVCP reads whole 32-bit words at
    /// aligned addresses; §4.1 has the producer widen any other read and
    /// return only the bytes asked for.
    pub(crate) fn read_remote(&self, address: u64, len: usize) -> Result<Vec<u8>> {
        if len == 0 {
            return Ok(Vec::new());
        }
        let end = address
            .checked_add(len as u64)
            .ok_or_else(|| Failure::new(GC_ERR_INVALID_ADDRESS, "the read wraps around"))?;
        let start = address & !3;
        let end = end.next_multiple_of(4);
        let words = self
            .io()?
            .read(start, (end - start) as usize)
            .map_err(|err| from_genapi(&err, Direction::Read))?;
        let from = (address - start) as usize;
        words
            .get(from..from + len)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| Failure::new(GC_ERR_IO, "the camera returned fewer bytes than asked"))
    }

    /// `GCWritePort` on the remote port: an unaligned write is completed to
    /// whole words with the bytes around it (`GigeDevice::write_mem`).
    pub(crate) fn write_remote(&self, address: u64, data: &[u8]) -> Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        self.io()?
            .write(address, data)
            .map_err(|err| from_genapi(&err, Direction::Write))
    }

    /// The camera's GenApi XML URLs: its First and Second URL registers,
    /// those that are not empty.
    pub(crate) fn remote_urls(&self) -> Result<Vec<String>> {
        let mut urls = Vec::new();
        for register in [gige::consts::FIRST_URL, gige::consts::SECOND_URL] {
            let bytes = self.read_remote(register, gige::consts::URL_LEN)?;
            let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
            let url = String::from_utf8_lossy(&bytes[..end]).trim().to_string();
            if !url.is_empty() {
                urls.push(url);
            }
        }
        Ok(urls)
    }
}

/// Why a camera did not open, in GenTL's terms.
fn open_failure(err: &GigeError, info: &DeviceInfo) -> Failure {
    use viva_genicam::gencp::StatusCode;
    use viva_genicam::gige::GigeStatus;
    let code = match err {
        GigeError::Status(GigeStatus::Common(StatusCode::AccessDenied)) => GC_ERR_ACCESS_DENIED,
        _ => GC_ERR_IO,
    };
    Failure::new(code, format!("open the camera at {}: {err}", info.ip))
}

static LAYOUT: Layout = Layout {
    file: "viva_gentl_device",
    port: "DevicePort",
    guid: "76697661-6765-6e74-6c00-000000000003",
    categories: &[
        (
            "Root",
            &["DeviceInformation", "DeviceControl", "StreamEnumeration"],
        ),
        (
            "DeviceInformation",
            &[
                "DeviceID",
                "DeviceSerialNumber",
                "DeviceUserID",
                "DeviceVendorName",
                "DeviceModelName",
                "DeviceVersion",
                "DeviceType",
                "DeviceDisplayName",
                "DeviceAccessStatus",
                "DeviceTimestampFrequency",
                "GevDeviceMACAddress",
                "GevDeviceIPAddress",
                "GevDeviceSubnetMask",
                "GevDeviceGateway",
            ],
        ),
        ("DeviceControl", &["DeviceEndianessMechanism"]),
        ("StreamEnumeration", &["StreamSelector", "StreamID"]),
    ],
    features: &[
        Feature {
            name: "DeviceID",
            kind: Kind::Str,
            tip: "Interface-wide unique identifier of this device.",
        },
        Feature {
            name: "DeviceSerialNumber",
            kind: Kind::Str,
            tip: "Serial number of the remote device.",
        },
        Feature {
            name: "DeviceUserID",
            kind: Kind::Str,
            tip: "User-defined name of the remote device.",
        },
        Feature {
            name: "DeviceVendorName",
            kind: Kind::Str,
            tip: "Vendor of the remote device.",
        },
        Feature {
            name: "DeviceModelName",
            kind: Kind::Str,
            tip: "Model of the remote device.",
        },
        Feature {
            name: "DeviceVersion",
            kind: Kind::Str,
            tip: "Version of the remote device.",
        },
        Feature {
            name: "DeviceType",
            kind: Kind::Enum(TL_TYPE_ENTRIES),
            tip: "Transport layer type of the device.",
        },
        Feature {
            name: "DeviceDisplayName",
            kind: Kind::Str,
            tip: "User readable name of the device.",
        },
        Feature {
            name: "DeviceAccessStatus",
            kind: Kind::Enum(ACCESS_STATUS_ENTRIES),
            tip: "How the producer has the device open.",
        },
        Feature {
            name: "DeviceTimestampFrequency",
            kind: Kind::Int(Repr::Number),
            tip: "Tick frequency of the device's timestamp counter (0: not known).",
        },
        Feature {
            name: "GevDeviceMACAddress",
            kind: Kind::Int(Repr::Mac),
            tip: "MAC address of the remote device.",
        },
        Feature {
            name: "GevDeviceIPAddress",
            kind: Kind::Int(Repr::Ipv4),
            tip: "Current IP address of the remote device.",
        },
        Feature {
            name: "GevDeviceSubnetMask",
            kind: Kind::Int(Repr::Ipv4),
            tip: "Current subnet mask of the remote device.",
        },
        Feature {
            name: "GevDeviceGateway",
            kind: Kind::Int(Repr::Ipv4),
            tip: "Current gateway of the remote device.",
        },
        Feature {
            name: "DeviceEndianessMechanism",
            kind: Kind::Enum(&[("Standard", 1)]),
            tip: "Register data travels as the camera's XML declares it; nothing is swapped.",
        },
        Feature {
            name: "StreamSelector",
            kind: Kind::Selector(&["StreamID"]),
            tip: "Selects a stream channel; 0-based, as DevGetDataStreamID.",
        },
        Feature {
            name: "StreamID",
            kind: Kind::Str,
            tip: "Unique identifier of the selected data stream.",
        },
    ],
    xml: OnceLock::new(),
};

impl Features for Device {
    fn layout(&self) -> &'static Layout {
        &LAYOUT
    }

    fn read_feature(&self, feature: &Feature, register: Register) -> Result<Value> {
        let ip = |ip: Option<std::net::Ipv4Addr>| {
            Value::Int(i64::from(u32::from(
                ip.unwrap_or(std::net::Ipv4Addr::UNSPECIFIED),
            )))
        };
        Ok(match (feature.name, register) {
            ("DeviceID", _) => Value::Str(self.id()),
            ("DeviceSerialNumber", _) => Value::Str(self.info.serial.clone().unwrap_or_default()),
            ("DeviceUserID", _) => Value::Str(self.info.user_name.clone().unwrap_or_default()),
            ("DeviceVendorName", _) => {
                Value::Str(self.info.manufacturer.clone().unwrap_or_default())
            }
            ("DeviceModelName", _) => Value::Str(self.info.model.clone().unwrap_or_default()),
            ("DeviceVersion", _) => Value::Str(self.info.version.clone().unwrap_or_default()),
            ("DeviceType", _) => Value::Int(crate::regmap::GIGE_VISION),
            ("DeviceDisplayName", _) => Value::Str(crate::interface::display_name(&self.info)),
            ("DeviceAccessStatus", _) => Value::Int(i64::from(self.access_status())),
            ("DeviceTimestampFrequency", _) => Value::Int(
                self.tick_frequency
                    .and_then(|f| i64::try_from(f).ok())
                    .unwrap_or(0),
            ),
            ("GevDeviceMACAddress", _) => Value::Int(
                self.info
                    .mac
                    .iter()
                    .fold(0i64, |acc, &b| (acc << 8) | i64::from(b)),
            ),
            ("GevDeviceIPAddress", _) => ip(Some(self.info.ip)),
            ("GevDeviceSubnetMask", _) => ip(self.info.subnet),
            ("GevDeviceGateway", _) => ip(self.info.gateway),
            ("DeviceEndianessMechanism", _) => Value::Int(1),
            ("StreamSelector", Register::Value) => {
                Value::Int(*self.stream_selector.lock().map_err(poisoned)? as i64)
            }
            ("StreamSelector", Register::Max) => {
                Value::Int(i64::from(self.streams.saturating_sub(1)))
            }
            ("StreamID", _) => Value::Str(self.stream_id(0)?.to_string()),
            (name, _) => return Err(not_implemented(format!("feature {name}"))),
        })
    }

    fn write_feature(&self, feature: &Feature, value: i64) -> Result<()> {
        match feature.name {
            "StreamSelector" => {
                if value != 0 {
                    return Err(Failure::new(GC_ERR_INVALID_VALUE, "one stream channel"));
                }
                *self.stream_selector.lock().map_err(poisoned)? = 0;
                Ok(())
            }
            name => Err(Failure::new(
                GC_ERR_ACCESS_DENIED,
                format!("{name} is not writable"),
            )),
        }
    }
}

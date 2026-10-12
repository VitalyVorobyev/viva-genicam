//! The Interface module (GenTL 1.6 §2.2.2, §3.3): one host network
//! interface and the GigE Vision cameras discovered on it.

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use viva_genicam::gige::{self, DeviceInfo};

use crate::abi::*;
use crate::device::{Access, Device};
use crate::error::{Failure, Result, invalid_parameter, not_available, not_implemented, poisoned};
use crate::library::{Library, Object};
use crate::nic::Nic;
use crate::out::Info;
use crate::regmap::{
    ACCESS_STATUS_ENTRIES, Feature, Features, Kind, Layout, MAX_TIMEOUT_MS, Register, Repr,
    TL_TYPE_ENTRIES, Value,
};

/// How long a device list update listens when the consumer says
/// `GENTL_INFINITE`: GigE Vision cameras answer discovery well within it.
const DEFAULT_DISCOVERY: Duration = Duration::from_millis(1000);
/// The longest a device list update listens, whatever the consumer asks:
/// §6.3.3.7 wants it "completed in a reasonable amount of time".
const MAX_DISCOVERY: Duration = Duration::from_millis(MAX_TIMEOUT_MS as u64);
/// `DeviceUpdateTimeout`'s default.
const DEFAULT_UPDATE_TIMEOUT_MS: i64 = 1000;

/// The Interface module.
pub(crate) struct Interface {
    pub(crate) key: usize,
    pub(crate) nic: Nic,
    state: Mutex<State>,
}

struct State {
    /// The device list as of the last `IFUpdateDeviceList`.
    devices: Vec<DeviceInfo>,
    /// Devices open now: their ID, key and access.
    open: HashMap<String, (usize, Access)>,
    device_selector: usize,
    subnet_selector: usize,
    gateway_selector: usize,
    update_timeout_ms: i64,
}

/// A device's ID: `GEV:` and its MAC address. Different from anything the
/// remote device reports about itself, as §3.4 recommends, and the same in
/// every session.
pub(crate) fn device_id(info: &DeviceInfo) -> String {
    format!("GEV:{}", info.mac_string())
}

/// `VENDOR MODEL (ID)`, as `DEVICE_INFO_DISPLAYNAME` falls back to
/// (§6.4.3.3).
pub(crate) fn display_name(info: &DeviceInfo) -> String {
    format!(
        "{} {} ({})",
        info.manufacturer.as_deref().unwrap_or_default(),
        info.model.as_deref().unwrap_or_default(),
        device_id(info)
    )
}

/// The `DEVICE_INFO_CMD`s a discovery acknowledgement answers (§6.4.3.3).
pub(crate) fn device_info(info: &DeviceInfo, cmd: DeviceInfoCmd, status: i32) -> Result<Info> {
    let optional = |value: &Option<String>, what: &'static str| {
        value
            .clone()
            .filter(|s| !s.is_empty())
            .map(Info::Str)
            .ok_or_else(|| not_available(format!("the camera reports no {what}")))
    };
    Ok(match cmd {
        DEVICE_INFO_ID => Info::Str(device_id(info)),
        DEVICE_INFO_VENDOR => Info::Str(info.manufacturer.clone().unwrap_or_default()),
        DEVICE_INFO_MODEL => Info::Str(info.model.clone().unwrap_or_default()),
        DEVICE_INFO_TLTYPE => Info::Str(TL_TYPE_GEV.into()),
        DEVICE_INFO_DISPLAYNAME => Info::Str(display_name(info)),
        DEVICE_INFO_ACCESS_STATUS => Info::I32(status),
        DEVICE_INFO_USER_DEFINED_NAME => optional(&info.user_name, "user-defined name")?,
        DEVICE_INFO_SERIAL_NUMBER => optional(&info.serial, "serial number")?,
        DEVICE_INFO_VERSION => optional(&info.version, "version")?,
        DEVICE_INFO_TIMESTAMP_FREQUENCY => {
            return Err(not_available(
                "the tick frequency is read once the device is open",
            ));
        }
        other => return Err(not_implemented(format!("DEVICE_INFO_CMD {other}"))),
    })
}

fn ip_value(ip: Option<Ipv4Addr>) -> Value {
    Value::Int(i64::from(u32::from(ip.unwrap_or(Ipv4Addr::UNSPECIFIED))))
}

fn mac_value(mac: [u8; 6]) -> Value {
    Value::Int(mac.iter().fold(0i64, |acc, &b| (acc << 8) | i64::from(b)))
}

impl Interface {
    pub(crate) fn new(key: usize, nic: Nic) -> Self {
        Self {
            key,
            nic,
            state: Mutex::new(State {
                devices: Vec::new(),
                open: HashMap::new(),
                device_selector: 0,
                subnet_selector: 0,
                gateway_selector: 0,
                update_timeout_ms: DEFAULT_UPDATE_TIMEOUT_MS,
            }),
        }
    }

    /// `IFUpdateDeviceList`: discover on this interface for `timeout_ms`
    /// (capped), and report whether the list changed.
    pub(crate) fn update_devices(&self, lib: &Library, timeout_ms: u64) -> Result<bool> {
        let wait = if timeout_ms == GENTL_INFINITE {
            DEFAULT_DISCOVERY
        } else {
            Duration::from_millis(timeout_ms).min(MAX_DISCOVERY)
        };
        let found = lib
            .runtime
            .block_on(gige::discover_on_interface(wait, &self.nic.name))
            .map_err(|err| {
                Failure::new(GC_ERR_IO, format!("discovery on {}: {err}", self.nic.name))
            })?;
        let mut state = self.state.lock().map_err(poisoned)?;
        let ids = |devices: &[DeviceInfo]| -> Vec<(String, Ipv4Addr)> {
            devices.iter().map(|d| (device_id(d), d.ip)).collect()
        };
        let changed = ids(&state.devices) != ids(&found);
        state.devices = found;
        if state.device_selector >= state.devices.len() {
            state.device_selector = 0;
        }
        Ok(changed)
    }

    pub(crate) fn device_count(&self) -> Result<u32> {
        let state = self.state.lock().map_err(poisoned)?;
        Ok(u32::try_from(state.devices.len()).unwrap_or(u32::MAX))
    }

    /// `IFGetDeviceID`.
    pub(crate) fn device_id_at(&self, index: u32) -> Result<String> {
        let state = self.state.lock().map_err(poisoned)?;
        state
            .devices
            .get(index as usize)
            .map(device_id)
            .ok_or_else(|| {
                Failure::new(
                    GC_ERR_INVALID_INDEX,
                    format!("device {index} of {}", state.devices.len()),
                )
            })
    }

    /// The listed device `id` names: its ID, or its IPv4 address (§3.3's
    /// convenience names).
    fn listed(&self, id: &str) -> Result<Option<DeviceInfo>> {
        let state = self.state.lock().map_err(poisoned)?;
        let ip = id.parse::<Ipv4Addr>().ok();
        Ok(state
            .devices
            .iter()
            .find(|d| device_id(d) == id || Some(d.ip) == ip)
            .cloned())
    }

    /// The access status `IFGetDeviceInfo` reports for `id`.
    fn status_of(&self, id: &str) -> Result<i32> {
        let state = self.state.lock().map_err(poisoned)?;
        Ok(match state.open.get(id) {
            Some((_, Access::ReadOnly)) => DEVICE_ACCESS_STATUS_OPEN_READONLY,
            Some(_) => DEVICE_ACCESS_STATUS_OPEN_READWRITE,
            // Discovery does not say whether another host controls it.
            None => DEVICE_ACCESS_STATUS_UNKNOWN,
        })
    }

    /// `IFGetDeviceInfo`.
    pub(crate) fn device_info(&self, id: &str, cmd: DeviceInfoCmd) -> Result<Info> {
        let info = self.listed(id)?.ok_or_else(|| {
            Failure::new(GC_ERR_INVALID_ID, format!("no device '{id}' in the list"))
        })?;
        let status = self.status_of(&device_id(&info))?;
        device_info(&info, cmd, status)
    }

    /// `IFOpenDevice`.
    pub(crate) fn open_device(
        &self,
        lib: &Library,
        id: &str,
        flags: DeviceAccessFlags,
    ) -> Result<usize> {
        let access = match flags {
            DEVICE_ACCESS_READONLY => Access::ReadOnly,
            DEVICE_ACCESS_CONTROL => Access::Control,
            DEVICE_ACCESS_EXCLUSIVE => Access::Exclusive,
            f if f >= 1000 => {
                return Err(not_implemented(format!("DEVICE_ACCESS_FLAGS {f}")));
            }
            _ => {
                return Err(invalid_parameter(
                    "iOpenFlag is not a valid DEVICE_ACCESS_FLAGS",
                ));
            }
        };
        let info = match self.listed(id)? {
            Some(info) => info,
            None => self.discover(lib, id)?.ok_or_else(|| {
                Failure::new(
                    GC_ERR_INVALID_ID,
                    format!("no device '{id}' on {}", self.nic.name),
                )
            })?,
        };
        let id = device_id(&info);
        if self.state.lock().map_err(poisoned)?.open.contains_key(&id) {
            return Err(Failure::new(
                GC_ERR_RESOURCE_IN_USE,
                format!("device {id} is already open"),
            ));
        }
        let key = lib.reserve();
        let device = Arc::new(Device::open(lib, key, self.key, info, access)?);
        let mut state = self.state.lock().map_err(poisoned)?;
        if state.open.contains_key(&id) {
            // Another thread opened it meanwhile; this one gives control back.
            drop(state);
            device.shut_down();
            return Err(Failure::new(
                GC_ERR_RESOURCE_IN_USE,
                format!("device {id} is already open"),
            ));
        }
        let remote = device.remote_key;
        lib.insert_at(key, Some(self.key), Object::Device(device.clone()))?;
        lib.insert_at(remote, Some(key), Object::RemotePort(device))?;
        state.open.insert(id, (key, access));
        Ok(key)
    }

    /// Find a device that is not in the list (§3.7: "the GenTL Producer
    /// might need to (re)execute the device discovery process on its own"),
    /// without changing the list.
    fn discover(&self, lib: &Library, id: &str) -> Result<Option<DeviceInfo>> {
        if let Ok(ip) = id.parse::<Ipv4Addr>() {
            return lib
                .runtime
                .block_on(gige::discover_ip(ip, DEFAULT_DISCOVERY))
                .map_err(|err| Failure::new(GC_ERR_IO, format!("discovery of {ip}: {err}")));
        }
        if !id.starts_with("GEV:") {
            return Ok(None);
        }
        let found = lib
            .runtime
            .block_on(gige::discover_on_interface(
                DEFAULT_DISCOVERY,
                &self.nic.name,
            ))
            .map_err(|err| {
                Failure::new(GC_ERR_IO, format!("discovery on {}: {err}", self.nic.name))
            })?;
        Ok(found.into_iter().find(|d| device_id(d) == id))
    }

    /// A child device closed: its ID opens again.
    pub(crate) fn child_closed(&self, key: usize) {
        if let Ok(mut state) = self.state.lock() {
            state.open.retain(|_, &mut (open, _)| open != key);
        }
    }

    fn selected_device(&self) -> Result<(DeviceInfo, i32)> {
        let info = {
            let state = self.state.lock().map_err(poisoned)?;
            state
                .devices
                .get(state.device_selector)
                .cloned()
                .ok_or_else(|| not_available("no device is selected: the list is empty"))?
        };
        let status = self.status_of(&device_id(&info))?;
        Ok((info, status))
    }
}

static LAYOUT: Layout = Layout {
    file: "viva_gentl_interface",
    port: "InterfacePort",
    guid: "76697661-6765-6e74-6c00-000000000002",
    categories: &[
        ("Root", &["InterfaceInformation", "DeviceEnumeration"]),
        (
            "InterfaceInformation",
            &[
                "InterfaceID",
                "InterfaceDisplayName",
                "InterfaceType",
                "InterfaceTLVersionMajor",
                "InterfaceTLVersionMinor",
                "GevInterfaceGatewaySelector",
                "GevInterfaceGateway",
                "GevInterfaceMACAddress",
                "GevInterfaceSubnetSelector",
                "GevInterfaceSubnetIPAddress",
                "GevInterfaceSubnetMask",
            ],
        ),
        (
            "DeviceEnumeration",
            &[
                "DeviceUpdateList",
                "DeviceUpdateTimeout",
                "DeviceSelector",
                "DeviceID",
                "DeviceVendorName",
                "DeviceModelName",
                "DeviceAccessStatus",
                "DeviceSerialNumber",
                "DeviceUserID",
                "DeviceTLVersionMajor",
                "DeviceTLVersionMinor",
                "GevDeviceIPAddress",
                "GevDeviceSubnetMask",
                "GevDeviceGateway",
                "GevDeviceMACAddress",
            ],
        ),
    ],
    features: &[
        Feature {
            name: "InterfaceID",
            kind: Kind::Str,
            tip: "Unique identifier of this interface.",
        },
        Feature {
            name: "InterfaceDisplayName",
            kind: Kind::Str,
            tip: "User readable name of this interface.",
        },
        Feature {
            name: "InterfaceType",
            kind: Kind::Enum(TL_TYPE_ENTRIES),
            tip: "Transport layer type of this interface.",
        },
        Feature {
            name: "InterfaceTLVersionMajor",
            kind: Kind::Int(Repr::Number),
            tip: "Major version of the GigE Vision specification the producer implements.",
        },
        Feature {
            name: "InterfaceTLVersionMinor",
            kind: Kind::Int(Repr::Number),
            tip: "Minor version of the GigE Vision specification the producer implements.",
        },
        Feature {
            name: "GevInterfaceGatewaySelector",
            kind: Kind::Selector(&["GevInterfaceGateway"]),
            tip: "Selects a gateway entry of this interface.",
        },
        Feature {
            name: "GevInterfaceGateway",
            kind: Kind::Int(Repr::Ipv4),
            tip: "IP address of the selected gateway (0: not known).",
        },
        Feature {
            name: "GevInterfaceMACAddress",
            kind: Kind::Int(Repr::Mac),
            tip: "MAC address of this interface (0: not known).",
        },
        Feature {
            name: "GevInterfaceSubnetSelector",
            kind: Kind::Selector(&["GevInterfaceSubnetIPAddress", "GevInterfaceSubnetMask"]),
            tip: "Selects a subnet of this interface.",
        },
        Feature {
            name: "GevInterfaceSubnetIPAddress",
            kind: Kind::Int(Repr::Ipv4),
            tip: "IP address of the selected subnet.",
        },
        Feature {
            name: "GevInterfaceSubnetMask",
            kind: Kind::Int(Repr::Ipv4),
            tip: "Subnet mask of the selected subnet.",
        },
        Feature {
            name: "DeviceUpdateList",
            kind: Kind::Command,
            tip: "Updates the list of devices: GigE Vision discovery on this interface.",
        },
        Feature {
            name: "DeviceUpdateTimeout",
            kind: Kind::Timeout,
            tip: "How long DeviceUpdateList listens for cameras.",
        },
        Feature {
            name: "DeviceSelector",
            kind: Kind::Selector(&[
                "DeviceID",
                "DeviceVendorName",
                "DeviceModelName",
                "DeviceAccessStatus",
                "DeviceSerialNumber",
                "DeviceUserID",
                "DeviceTLVersionMajor",
                "DeviceTLVersionMinor",
                "GevDeviceIPAddress",
                "GevDeviceSubnetMask",
                "GevDeviceGateway",
                "GevDeviceMACAddress",
            ]),
            tip: "Selects a device; 0-based, as IFGetDeviceID.",
        },
        Feature {
            name: "DeviceID",
            kind: Kind::Str,
            tip: "Unique identifier of the selected device.",
        },
        Feature {
            name: "DeviceVendorName",
            kind: Kind::Str,
            tip: "Vendor of the selected device.",
        },
        Feature {
            name: "DeviceModelName",
            kind: Kind::Str,
            tip: "Model of the selected device.",
        },
        Feature {
            name: "DeviceAccessStatus",
            kind: Kind::Enum(ACCESS_STATUS_ENTRIES),
            tip: "Access status of the selected device as of the last update.",
        },
        Feature {
            name: "DeviceSerialNumber",
            kind: Kind::Str,
            tip: "Serial number of the selected device.",
        },
        Feature {
            name: "DeviceUserID",
            kind: Kind::Str,
            tip: "User-defined name of the selected device.",
        },
        Feature {
            name: "DeviceTLVersionMajor",
            kind: Kind::Int(Repr::Number),
            tip: "Major GigE Vision version of the selected device (0: not known).",
        },
        Feature {
            name: "DeviceTLVersionMinor",
            kind: Kind::Int(Repr::Number),
            tip: "Minor GigE Vision version of the selected device (0: not known).",
        },
        Feature {
            name: "GevDeviceIPAddress",
            kind: Kind::Int(Repr::Ipv4),
            tip: "Current IP address of the selected device.",
        },
        Feature {
            name: "GevDeviceSubnetMask",
            kind: Kind::Int(Repr::Ipv4),
            tip: "Current subnet mask of the selected device.",
        },
        Feature {
            name: "GevDeviceGateway",
            kind: Kind::Int(Repr::Ipv4),
            tip: "Current gateway of the selected device.",
        },
        Feature {
            name: "GevDeviceMACAddress",
            kind: Kind::Int(Repr::Mac),
            tip: "MAC address of the selected device.",
        },
    ],
    xml: OnceLock::new(),
};

/// The GigE Vision version the transport implements, which
/// `InterfaceTLVersionMajor`/`Minor` report: 2.0's extended IDs and
/// `PENDING_ACK` are in `viva-gige`; nothing has checked compliance against
/// the specification, which is not freely available.
const GEV_VERSION: (i64, i64) = (2, 0);

impl Features for Interface {
    fn layout(&self) -> &'static Layout {
        &LAYOUT
    }

    fn read_feature(&self, feature: &Feature, register: Register) -> Result<Value> {
        let lock = || self.state.lock().map_err(poisoned);
        Ok(match (feature.name, register) {
            ("InterfaceID", _) => Value::Str(self.nic.name.clone()),
            ("InterfaceDisplayName", _) => Value::Str(self.nic.display_name()),
            ("InterfaceType", _) => Value::Int(crate::regmap::GIGE_VISION),
            ("InterfaceTLVersionMajor", _) => Value::Int(GEV_VERSION.0),
            ("InterfaceTLVersionMinor", _) => Value::Int(GEV_VERSION.1),
            ("GevInterfaceGatewaySelector", Register::Value) => {
                Value::Int(lock()?.gateway_selector as i64)
            }
            ("GevInterfaceSubnetSelector", Register::Value) => {
                Value::Int(lock()?.subnet_selector as i64)
            }
            ("GevInterfaceSubnetSelector", Register::Max) => {
                Value::Int(self.nic.subnets.len().saturating_sub(1) as i64)
            }
            ("GevInterfaceGatewaySelector", Register::Max) => Value::Int(0),
            ("GevInterfaceGateway", _) | ("GevInterfaceMACAddress", _) => Value::Int(0),
            ("GevInterfaceSubnetIPAddress", _) | ("GevInterfaceSubnetMask", _) => {
                let selected = lock()?.subnet_selector;
                let (ip, mask) = self
                    .nic
                    .subnets
                    .get(selected)
                    .copied()
                    .ok_or_else(|| not_available("no subnet is selected"))?;
                if feature.name == "GevInterfaceSubnetMask" {
                    ip_value(Some(mask))
                } else {
                    ip_value(Some(ip))
                }
            }
            ("DeviceUpdateList", _) => Value::Int(0),
            ("DeviceUpdateTimeout", _) => Value::Int(lock()?.update_timeout_ms),
            ("DeviceSelector", Register::Value) => Value::Int(lock()?.device_selector as i64),
            ("DeviceSelector", Register::Max) => {
                Value::Int(lock()?.devices.len().saturating_sub(1) as i64)
            }
            (name, _) => {
                let (info, status) = self.selected_device()?;
                match name {
                    "DeviceID" => Value::Str(device_id(&info)),
                    "DeviceVendorName" => Value::Str(info.manufacturer.unwrap_or_default()),
                    "DeviceModelName" => Value::Str(info.model.unwrap_or_default()),
                    "DeviceAccessStatus" => Value::Int(i64::from(status)),
                    "DeviceSerialNumber" => Value::Str(info.serial.unwrap_or_default()),
                    "DeviceUserID" => Value::Str(info.user_name.unwrap_or_default()),
                    // Discovery does not hand the device's version on.
                    "DeviceTLVersionMajor" | "DeviceTLVersionMinor" => Value::Int(0),
                    "GevDeviceIPAddress" => ip_value(Some(info.ip)),
                    "GevDeviceSubnetMask" => ip_value(info.subnet),
                    "GevDeviceGateway" => ip_value(info.gateway),
                    "GevDeviceMACAddress" => mac_value(info.mac),
                    other => return Err(not_implemented(format!("feature {other}"))),
                }
            }
        })
    }

    fn write_feature(&self, feature: &Feature, value: i64) -> Result<()> {
        let invalid = || {
            Failure::new(
                GC_ERR_INVALID_VALUE,
                format!("{} out of range", feature.name),
            )
        };
        match feature.name {
            "DeviceUpdateList" => {
                if value == 1 {
                    let timeout = self.state.lock().map_err(poisoned)?.update_timeout_ms;
                    let lib = crate::library::get()?;
                    self.update_devices(&lib, timeout as u64)?;
                }
                Ok(())
            }
            "DeviceUpdateTimeout" => {
                if !(0..=MAX_TIMEOUT_MS).contains(&value) {
                    return Err(invalid());
                }
                self.state.lock().map_err(poisoned)?.update_timeout_ms = value;
                Ok(())
            }
            "DeviceSelector" => {
                let mut state = self.state.lock().map_err(poisoned)?;
                let index = usize::try_from(value)
                    .ok()
                    .filter(|&i| i < state.devices.len().max(1))
                    .ok_or_else(invalid)?;
                state.device_selector = index;
                Ok(())
            }
            "GevInterfaceSubnetSelector" => {
                let index = usize::try_from(value)
                    .ok()
                    .filter(|&i| i < self.nic.subnets.len().max(1))
                    .ok_or_else(invalid)?;
                self.state.lock().map_err(poisoned)?.subnet_selector = index;
                Ok(())
            }
            "GevInterfaceGatewaySelector" => {
                if value != 0 {
                    return Err(invalid());
                }
                Ok(())
            }
            name => Err(Failure::new(
                GC_ERR_ACCESS_DENIED,
                format!("{name} is not writable"),
            )),
        }
    }
}

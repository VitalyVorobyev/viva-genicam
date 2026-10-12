//! The System module (GenTL 1.6 §2.2.1, §3.2): the producer itself and the
//! list of host interfaces.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::abi::*;
use crate::error::{Failure, Result, not_available, not_implemented, poisoned};
use crate::interface::Interface;
use crate::library::{Library, Object};
use crate::nic::{self, Nic};
use crate::out::Info;
use crate::regmap::{
    self, Feature, Features, Kind, Layout, MAX_TIMEOUT_MS, Register, Repr, TL_TYPE_ENTRIES, Value,
};

/// `TL_INFO_ID`: "Unique ID identifying a GenTL Producer".
pub(crate) const PRODUCER_ID: &str = "viva_gentl";
/// `TL_INFO_VENDOR`.
pub(crate) const VENDOR: &str = "viva-genicam";
/// `TL_INFO_MODEL`.
pub(crate) const MODEL: &str = "viva-gentl";
/// `TL_INFO_DISPLAYNAME`.
const DISPLAY_NAME: &str = "viva-genicam GenTL producer (GigE Vision)";
/// GenTL SFNC version the module XMLs follow.
const SFNC_VERSION: (i64, i64) = (1, 2);
/// `InterfaceUpdateTimeout`'s default.
const DEFAULT_UPDATE_TIMEOUT_MS: i64 = 1000;

/// The System module.
pub(crate) struct System {
    pub(crate) key: usize,
    state: Mutex<State>,
}

struct State {
    /// The interface list as of the last `TLUpdateInterfaceList`.
    interfaces: Vec<Nic>,
    /// Interfaces open now, by ID.
    open: HashMap<String, usize>,
    selector: usize,
    update_timeout_ms: i64,
}

/// `GCGetInfo` and `TLGetInfo` (§6.4.1.3; "must be in sync").
pub(crate) fn info(cmd: TlInfoCmd) -> Result<Info> {
    let path = crate::libpath::library_path();
    Ok(match cmd {
        TL_INFO_ID => Info::Str(PRODUCER_ID.into()),
        TL_INFO_VENDOR => Info::Str(VENDOR.into()),
        TL_INFO_MODEL => Info::Str(MODEL.into()),
        TL_INFO_VERSION => Info::Str(env!("CARGO_PKG_VERSION").into()),
        TL_INFO_TLTYPE => Info::Str(TL_TYPE_GEV.into()),
        TL_INFO_NAME => Info::Str(
            path.as_deref()
                .and_then(|p| p.file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .ok_or_else(|| not_available("the producer's file name is not known"))?,
        ),
        TL_INFO_PATHNAME => Info::Str(
            path.map(|p| p.to_string_lossy().into_owned())
                .ok_or_else(|| not_available("the producer's path is not known"))?,
        ),
        TL_INFO_DISPLAYNAME => Info::Str(DISPLAY_NAME.into()),
        TL_INFO_CHAR_ENCODING => Info::I32(TL_CHAR_ENCODING_UTF8),
        TL_INFO_GENTL_VER_MAJOR => Info::U32(GENTL_VERSION.0),
        TL_INFO_GENTL_VER_MINOR => Info::U32(GENTL_VERSION.1),
        #[cfg(debug_assertions)]
        crate::PANIC_PROBE => panic!("the panic probe (a debug-build test hook)"),
        other => return Err(not_implemented(format!("TL_INFO_CMD {other}"))),
    })
}

/// `INTERFACE_INFO_CMD` for `nic` (§6.4.2.1).
pub(crate) fn interface_info(nic: &Nic, cmd: InterfaceInfoCmd) -> Result<Info> {
    Ok(match cmd {
        INTERFACE_INFO_ID => Info::Str(nic.name.clone()),
        INTERFACE_INFO_DISPLAYNAME => Info::Str(nic.display_name()),
        INTERFACE_INFO_TLTYPE => Info::Str(TL_TYPE_GEV.into()),
        other => return Err(not_implemented(format!("INTERFACE_INFO_CMD {other}"))),
    })
}

impl System {
    /// `TLOpen`: one System module per process (§3.2).
    pub(crate) fn open(lib: &Library) -> Result<usize> {
        if lib.any(|object| matches!(object, Object::System(_)))? {
            return Err(Failure::new(
                GC_ERR_RESOURCE_IN_USE,
                "the System module is already open",
            ));
        }
        let key = lib.reserve();
        // §3.2: "A GenTL Producer may call TLUpdateInterfaceList at module
        // instantiation if needed."
        let system = Arc::new(System {
            key,
            state: Mutex::new(State {
                interfaces: nic::list()?,
                open: HashMap::new(),
                selector: 0,
                update_timeout_ms: DEFAULT_UPDATE_TIMEOUT_MS,
            }),
        });
        lib.insert_at(key, None, Object::System(system))?;
        Ok(key)
    }

    /// `TLUpdateInterfaceList`: whether the list changed.
    pub(crate) fn update_interfaces(&self) -> Result<bool> {
        let fresh = nic::list()?;
        let mut state = self.state.lock().map_err(poisoned)?;
        let changed = state.interfaces != fresh;
        state.interfaces = fresh;
        if state.selector >= state.interfaces.len() {
            state.selector = 0;
        }
        Ok(changed)
    }

    pub(crate) fn interface_count(&self) -> Result<u32> {
        let state = self.state.lock().map_err(poisoned)?;
        Ok(u32::try_from(state.interfaces.len()).unwrap_or(u32::MAX))
    }

    /// `TLGetInterfaceID`.
    pub(crate) fn interface_id(&self, index: u32) -> Result<String> {
        let state = self.state.lock().map_err(poisoned)?;
        state
            .interfaces
            .get(index as usize)
            .map(|nic| nic.name.clone())
            .ok_or_else(|| {
                Failure::new(
                    GC_ERR_INVALID_INDEX,
                    format!("interface {index} of {}", state.interfaces.len()),
                )
            })
    }

    /// The interface `id` names: from the list, or the host as it is now.
    pub(crate) fn find_interface(&self, id: &str) -> Result<Nic> {
        let listed = self.state.lock().map_err(poisoned)?.interfaces.clone();
        nic::find(&listed, id)?
            .ok_or_else(|| Failure::new(GC_ERR_INVALID_ID, format!("no network interface '{id}'")))
    }

    /// `TLOpenInterface`.
    pub(crate) fn open_interface(&self, lib: &Library, id: &str) -> Result<usize> {
        let nic = self.find_interface(id)?;
        let mut state = self.state.lock().map_err(poisoned)?;
        if state.open.contains_key(&nic.name) {
            return Err(Failure::new(
                GC_ERR_RESOURCE_IN_USE,
                format!("interface '{}' is already open", nic.name),
            ));
        }
        let key = lib.reserve();
        let name = nic.name.clone();
        let interface = Arc::new(Interface::new(key, nic));
        lib.insert_at(key, Some(self.key), Object::Interface(interface))?;
        state.open.insert(name, key);
        Ok(key)
    }

    /// A child interface closed: its ID opens again.
    pub(crate) fn child_closed(&self, key: usize) {
        if let Ok(mut state) = self.state.lock() {
            state.open.retain(|_, &mut open| open != key);
        }
    }

    fn selected(&self) -> Result<Nic> {
        let state = self.state.lock().map_err(poisoned)?;
        state
            .interfaces
            .get(state.selector)
            .cloned()
            .ok_or_else(|| not_available("no interface is selected: the list is empty"))
    }
}

static LAYOUT: Layout = Layout {
    file: "viva_gentl_system",
    port: "TLPort",
    guid: "76697661-6765-6e74-6c00-000000000001",
    categories: &[
        ("Root", &["SystemInformation", "InterfaceEnumeration"]),
        (
            "SystemInformation",
            &[
                "TLID",
                "TLVendorName",
                "TLModelName",
                "TLVersion",
                "TLFileName",
                "TLDisplayName",
                "TLPath",
                "TLType",
                "GenTLVersionMajor",
                "GenTLVersionMinor",
                "GenTLSFNCVersionMajor",
                "GenTLSFNCVersionMinor",
            ],
        ),
        (
            "InterfaceEnumeration",
            &[
                "InterfaceUpdateList",
                "InterfaceUpdateTimeout",
                "InterfaceSelector",
                "InterfaceID",
                "InterfaceDisplayName",
                "GevInterfaceMACAddress",
                "GevInterfaceDefaultIPAddress",
                "GevInterfaceDefaultSubnetMask",
                "GevInterfaceDefaultGateway",
            ],
        ),
    ],
    features: &[
        Feature {
            name: "TLID",
            kind: Kind::Str,
            tip: "Unique identifier of the GenTL Producer.",
        },
        Feature {
            name: "TLVendorName",
            kind: Kind::Str,
            tip: "Name of the GenTL Producer vendor.",
        },
        Feature {
            name: "TLModelName",
            kind: Kind::Str,
            tip: "Name of the GenTL Producer.",
        },
        Feature {
            name: "TLVersion",
            kind: Kind::Str,
            tip: "Version of the GenTL Producer.",
        },
        Feature {
            name: "TLFileName",
            kind: Kind::Str,
            tip: "File name of the GenTL Producer, with its extension.",
        },
        Feature {
            name: "TLDisplayName",
            kind: Kind::Str,
            tip: "User readable name of the GenTL Producer.",
        },
        Feature {
            name: "TLPath",
            kind: Kind::Str,
            tip: "Full path to the GenTL Producer.",
        },
        Feature {
            name: "TLType",
            kind: Kind::Enum(TL_TYPE_ENTRIES),
            tip: "Transport layer type of the GenTL Producer.",
        },
        Feature {
            name: "GenTLVersionMajor",
            kind: Kind::Int(Repr::Number),
            tip: "Major version of the GenTL standard the producer complies with.",
        },
        Feature {
            name: "GenTLVersionMinor",
            kind: Kind::Int(Repr::Number),
            tip: "Minor version of the GenTL standard the producer complies with.",
        },
        Feature {
            name: "GenTLSFNCVersionMajor",
            kind: Kind::Int(Repr::Number),
            tip: "Major version of the GenTL SFNC these features follow.",
        },
        Feature {
            name: "GenTLSFNCVersionMinor",
            kind: Kind::Int(Repr::Number),
            tip: "Minor version of the GenTL SFNC these features follow.",
        },
        Feature {
            name: "InterfaceUpdateList",
            kind: Kind::Command,
            tip: "Updates the list of interfaces.",
        },
        Feature {
            name: "InterfaceUpdateTimeout",
            kind: Kind::Timeout,
            tip: "Timeout for InterfaceUpdateList.",
        },
        Feature {
            name: "InterfaceSelector",
            kind: Kind::Selector(&[
                "InterfaceID",
                "InterfaceDisplayName",
                "GevInterfaceMACAddress",
                "GevInterfaceDefaultIPAddress",
                "GevInterfaceDefaultSubnetMask",
                "GevInterfaceDefaultGateway",
            ]),
            tip: "Selects an interface; 0-based, as TLGetInterfaceID.",
        },
        Feature {
            name: "InterfaceID",
            kind: Kind::Str,
            tip: "Unique identifier of the selected interface.",
        },
        Feature {
            name: "InterfaceDisplayName",
            kind: Kind::Str,
            tip: "User readable name of the selected interface.",
        },
        Feature {
            name: "GevInterfaceMACAddress",
            kind: Kind::Int(Repr::Mac),
            tip: "MAC address of the selected interface (0: not known).",
        },
        Feature {
            name: "GevInterfaceDefaultIPAddress",
            kind: Kind::Int(Repr::Ipv4),
            tip: "IP address of the first subnet of the selected interface.",
        },
        Feature {
            name: "GevInterfaceDefaultSubnetMask",
            kind: Kind::Int(Repr::Ipv4),
            tip: "Subnet mask of the first subnet of the selected interface.",
        },
        Feature {
            name: "GevInterfaceDefaultGateway",
            kind: Kind::Int(Repr::Ipv4),
            tip: "Gateway of the selected interface (0: not known).",
        },
    ],
    xml: OnceLock::new(),
};

impl Features for System {
    fn layout(&self) -> &'static Layout {
        &LAYOUT
    }

    fn read_feature(&self, feature: &Feature, register: Register) -> Result<Value> {
        let string = |info: Result<Info>| match info? {
            Info::Str(s) => Ok(Value::Str(s)),
            _ => Ok(Value::Str(String::new())),
        };
        match (feature.name, register) {
            ("TLID", _) => string(info(TL_INFO_ID)),
            ("TLVendorName", _) => string(info(TL_INFO_VENDOR)),
            ("TLModelName", _) => string(info(TL_INFO_MODEL)),
            ("TLVersion", _) => string(info(TL_INFO_VERSION)),
            ("TLFileName", _) => string(info(TL_INFO_NAME)),
            ("TLDisplayName", _) => string(info(TL_INFO_DISPLAYNAME)),
            ("TLPath", _) => string(info(TL_INFO_PATHNAME)),
            ("TLType", _) => Ok(Value::Int(regmap::GIGE_VISION)),
            ("GenTLVersionMajor", _) => Ok(Value::Int(i64::from(GENTL_VERSION.0))),
            ("GenTLVersionMinor", _) => Ok(Value::Int(i64::from(GENTL_VERSION.1))),
            ("GenTLSFNCVersionMajor", _) => Ok(Value::Int(SFNC_VERSION.0)),
            ("GenTLSFNCVersionMinor", _) => Ok(Value::Int(SFNC_VERSION.1)),
            ("InterfaceUpdateList", _) => Ok(Value::Int(0)),
            ("InterfaceUpdateTimeout", _) => Ok(Value::Int(
                self.state.lock().map_err(poisoned)?.update_timeout_ms,
            )),
            ("InterfaceSelector", Register::Value) => Ok(Value::Int(
                self.state.lock().map_err(poisoned)?.selector as i64,
            )),
            ("InterfaceSelector", Register::Max) => {
                let count = self.state.lock().map_err(poisoned)?.interfaces.len();
                Ok(Value::Int(count.saturating_sub(1) as i64))
            }
            ("InterfaceID", _) => Ok(Value::Str(self.selected()?.name)),
            ("InterfaceDisplayName", _) => Ok(Value::Str(self.selected()?.display_name())),
            ("GevInterfaceMACAddress", _) | ("GevInterfaceDefaultGateway", _) => {
                self.selected()?;
                Ok(Value::Int(0))
            }
            ("GevInterfaceDefaultIPAddress", _) => {
                Ok(Value::Int(i64::from(u32::from(self.selected()?.address()))))
            }
            ("GevInterfaceDefaultSubnetMask", _) => {
                Ok(Value::Int(i64::from(u32::from(self.selected()?.netmask()))))
            }
            (name, _) => Err(not_implemented(format!("feature {name}"))),
        }
    }

    fn write_feature(&self, feature: &Feature, value: i64) -> Result<()> {
        match feature.name {
            "InterfaceUpdateList" => {
                if value == 1 {
                    self.update_interfaces()?;
                }
                Ok(())
            }
            "InterfaceUpdateTimeout" => {
                if !(0..=MAX_TIMEOUT_MS).contains(&value) {
                    return Err(Failure::new(GC_ERR_INVALID_VALUE, "timeout out of range"));
                }
                self.state.lock().map_err(poisoned)?.update_timeout_ms = value;
                Ok(())
            }
            "InterfaceSelector" => {
                let mut state = self.state.lock().map_err(poisoned)?;
                let index = usize::try_from(value)
                    .ok()
                    .filter(|&i| i < state.interfaces.len().max(1))
                    .ok_or_else(|| {
                        Failure::new(GC_ERR_INVALID_VALUE, "no interface with that index")
                    })?;
                state.selector = index;
                Ok(())
            }
            name => Err(Failure::new(
                GC_ERR_ACCESS_DENIED,
                format!("{name} is not writable"),
            )),
        }
    }
}

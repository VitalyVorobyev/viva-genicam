//! The port functions (GenTL 1.6 §4.1, §6.3.6): a module's own port, or the
//! remote device's.

use std::sync::Arc;

use crate::abi::*;
use crate::device::Device;
use crate::error::{Failure, Result, invalid_handle, not_available, not_implemented};
use crate::library::{Library, Object};
use crate::out::Info;
use crate::regmap::{self, Features, XML_ADDRESS};
use crate::system::{MODEL, VENDOR};

/// The port a handle names.
pub(crate) enum Port {
    /// A module's own port: its virtual register map and XML.
    Module {
        features: Arc<dyn Features + Send + Sync>,
        /// `PORT_INFO_MODULE`.
        module: &'static str,
        /// `PORT_INFO_ID`: the module's ID.
        id: String,
    },
    /// The remote device's port (`DevGetPort`).
    Remote(Arc<Device>),
}

/// The port `handle` names. A Buffer module has none (§4.1.1.2); an event is
/// no module at all.
pub(crate) fn port(lib: &Library, handle: Handle) -> Result<Port> {
    Ok(match lib.lookup(handle)?.0 {
        Object::System(system) => Port::Module {
            features: system,
            module: MODULE_SYSTEM,
            id: crate::system::PRODUCER_ID.into(),
        },
        Object::Interface(interface) => Port::Module {
            id: interface.nic.name.clone(),
            features: interface,
            module: MODULE_INTERFACE,
        },
        Object::Device(device) => Port::Module {
            id: device.id(),
            features: device,
            module: MODULE_DEVICE,
        },
        Object::Stream(stream) => Port::Module {
            features: stream,
            module: MODULE_DATA_STREAM,
            id: crate::device::STREAM_ID.into(),
        },
        Object::RemotePort(device) => Port::Remote(device),
        Object::Buffer(_) => return Err(not_implemented("a Buffer module has no port")),
        Object::NewBufferEvent(_) => return Err(invalid_handle("an event handle is not a port")),
    })
}

impl Port {
    /// `GCReadPort`.
    pub(crate) fn read(&self, address: u64, len: usize) -> Result<Vec<u8>> {
        match self {
            Port::Module { features, .. } => regmap::read(features.as_ref(), address, len),
            Port::Remote(device) => device.read_remote(address, len),
        }
    }

    /// `GCWritePort`.
    pub(crate) fn write(&self, address: u64, data: &[u8]) -> Result<()> {
        match self {
            Port::Module { features, .. } => regmap::write(features.as_ref(), address, data),
            Port::Remote(device) => device.write_remote(address, data),
        }
    }

    /// The port's XML URLs: a module's one, or the camera's.
    pub(crate) fn urls(&self) -> Result<Vec<Url>> {
        match self {
            Port::Module { features, .. } => {
                let layout = features.layout();
                Ok(vec![Url {
                    text: layout.url(),
                    scheme: URL_SCHEME_LOCAL,
                    file: Some(layout.file_name()),
                    address: Some(XML_ADDRESS),
                    size: Some(layout.xml().len() as u64),
                    schema: Some((1, 1, 0)),
                    file_version: Some(crate_version()),
                }])
            }
            Port::Remote(device) => Ok(device
                .remote_urls()?
                .iter()
                .map(|u| Url::parse(u))
                .collect()),
        }
    }

    /// `GCGetPortURL`'s list (deprecated): each URL with its schema version,
    /// which "is only to be used with the legacy function" (§4.1.2.1).
    pub(crate) fn legacy_urls(&self) -> Result<Vec<String>> {
        Ok(self
            .urls()?
            .into_iter()
            .map(|url| match (&self, url.schema) {
                (Port::Module { .. }, Some((a, b, c))) => {
                    format!("{}?SchemaVersion={a}.{b}.{c}", url.text)
                }
                _ => url.text,
            })
            .collect())
    }

    /// `GCGetPortInfo` (§6.4.5.1).
    pub(crate) fn info(&self, cmd: PortInfoCmd) -> Result<Info> {
        let remote = match self {
            Port::Module { .. } => None,
            Port::Remote(device) => Some(device),
        };
        Ok(match cmd {
            PORT_INFO_ID => Info::Str(match self {
                Port::Module { id, .. } => id.clone(),
                // "In case of the remote device module PORT_INFO_ID returns
                // the same ID as for the local device module."
                Port::Remote(device) => device.id(),
            }),
            PORT_INFO_VENDOR => Info::Str(match remote {
                Some(device) => device.info.manufacturer.clone().unwrap_or_default(),
                None => VENDOR.into(),
            }),
            PORT_INFO_MODEL => Info::Str(match remote {
                Some(device) => device.info.model.clone().unwrap_or_default(),
                None => MODEL.into(),
            }),
            PORT_INFO_TLTYPE => Info::Str(TL_TYPE_GEV.into()),
            PORT_INFO_MODULE => Info::Str(match self {
                Port::Module { module, .. } => (*module).into(),
                Port::Remote(_) => MODULE_REMOTE_DEVICE.into(),
            }),
            // Module registers are little-endian; GigE Vision's big-endian.
            PORT_INFO_LITTLE_ENDIAN => Info::Bool(remote.is_none()),
            PORT_INFO_BIG_ENDIAN => Info::Bool(remote.is_some()),
            PORT_INFO_ACCESS_READ => Info::Bool(true),
            PORT_INFO_ACCESS_WRITE => Info::Bool(match remote {
                Some(device) => device.access != crate::device::Access::ReadOnly,
                None => true,
            }),
            PORT_INFO_ACCESS_NA | PORT_INFO_ACCESS_NI => Info::Bool(false),
            PORT_INFO_VERSION => Info::Str(match remote {
                Some(device) => device.info.version.clone().unwrap_or_default(),
                None => env!("CARGO_PKG_VERSION").into(),
            }),
            PORT_INFO_PORTNAME => Info::Str(match self {
                Port::Module { features, .. } => features.layout().port.into(),
                // The camera's XML names its port; by convention, "Device".
                Port::Remote(_) => "Device".into(),
            }),
            other => return Err(not_implemented(format!("PORT_INFO_CMD {other}"))),
        })
    }
}

fn crate_version() -> (i32, i32, i32) {
    let mut parts = env!("CARGO_PKG_VERSION")
        .split('.')
        .map(|p| p.parse::<i32>().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

/// One XML URL and what `GCGetPortURLInfo` reports about it (§4.1.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Url {
    pub(crate) text: String,
    pub(crate) scheme: i32,
    pub(crate) file: Option<String>,
    pub(crate) address: Option<u64>,
    pub(crate) size: Option<u64>,
    pub(crate) schema: Option<(i32, i32, i32)>,
    pub(crate) file_version: Option<(i32, i32, i32)>,
}

impl Url {
    /// Parse a camera's URL: `local:[///]file.ext;address;length`, `file:` or
    /// `http:`, each with an optional `?SchemaVersion=x.y.z` (Table 4-1).
    pub(crate) fn parse(text: &str) -> Url {
        let (body, query) = text.split_once('?').unwrap_or((text, ""));
        let schema = query
            .split('&')
            .find_map(|pair| pair.strip_prefix("SchemaVersion="))
            .and_then(|version| {
                let mut parts = version.split('.').map(|p| p.parse::<i32>().ok());
                Some((
                    parts.next()??,
                    parts.next()??,
                    parts.next().flatten().unwrap_or(0),
                ))
            });
        let (scheme_name, rest) = body.split_once(':').unwrap_or(("", body));
        let mut url = Url {
            text: text.to_string(),
            scheme: URL_SCHEME_CUSTOM_ID,
            file: None,
            address: None,
            size: None,
            schema,
            file_version: None,
        };
        match scheme_name.to_ascii_lowercase().as_str() {
            "local" => {
                url.scheme = URL_SCHEME_LOCAL;
                let parts: Vec<&str> = rest.trim_start_matches('/').split(';').collect();
                if let [file, address, size] = parts.as_slice() {
                    url.file = Some((*file).to_string());
                    url.address = u64::from_str_radix(address.trim(), 16).ok();
                    url.size = u64::from_str_radix(size.trim(), 16).ok();
                }
            }
            "file" => {
                url.scheme = URL_SCHEME_FILE;
                url.file = Some(rest.trim_start_matches("///").to_string());
            }
            "http" | "https" => url.scheme = URL_SCHEME_HTTP,
            _ => {}
        }
        url
    }

    /// `GCGetPortURLInfo` (§6.4.5.2).
    pub(crate) fn info(&self, cmd: UrlInfoCmd) -> Result<Info> {
        let missing = |what: &'static str| not_available(format!("the URL gives no {what}"));
        Ok(match cmd {
            URL_INFO_URL => Info::Str(self.text.clone()),
            URL_INFO_SCHEMA_VER_MAJOR => Info::I32(self.schema.ok_or_else(|| missing("schema"))?.0),
            URL_INFO_SCHEMA_VER_MINOR => Info::I32(self.schema.ok_or_else(|| missing("schema"))?.1),
            URL_INFO_FILE_VER_MAJOR => {
                Info::I32(self.file_version.ok_or_else(|| missing("file version"))?.0)
            }
            URL_INFO_FILE_VER_MINOR => {
                Info::I32(self.file_version.ok_or_else(|| missing("file version"))?.1)
            }
            URL_INFO_FILE_VER_SUBMINOR => {
                Info::I32(self.file_version.ok_or_else(|| missing("file version"))?.2)
            }
            URL_INFO_FILE_SHA1_HASH => return Err(missing("SHA-1 hash")),
            URL_INFO_FILE_REGISTER_ADDRESS => {
                Info::U64(self.address.ok_or_else(|| missing("register address"))?)
            }
            URL_INFO_FILE_SIZE => Info::U64(self.size.ok_or_else(|| missing("file size"))?),
            URL_INFO_SCHEME => Info::I32(self.scheme),
            URL_INFO_FILENAME => Info::Str(self.file.clone().ok_or_else(|| missing("file name"))?),
            other => return Err(not_implemented(format!("URL_INFO_CMD {other}"))),
        })
    }
}

/// `URL_SCHEME_CUSTOM_ID` (§6.4.5.3): a scheme the standard does not name.
const URL_SCHEME_CUSTOM_ID: i32 = 1000;

/// `iURLIndex` past the list.
pub(crate) fn url_index(urls: &[Url], index: u32) -> Result<&Url> {
    urls.get(index as usize).ok_or_else(|| {
        Failure::new(
            GC_ERR_INVALID_INDEX,
            format!("URL {index} of {}", urls.len()),
        )
    })
}

//! Each module's virtual register map and the GenApi XML that describes it
//! (GenTL 1.6 §2.3.2, §4.1).
//!
//! One table per module — a [`Layout`] — generates both, so the XML and the
//! registers cannot disagree. Feature `i` lives at
//! `FEATURE_BASE + i × SLOT`; the XML document at [`XML_ADDRESS`]. Every
//! register is little-endian and `NoCache` (§7: these values "should not be
//! cached in the nodemap but read every time from the module"). The names
//! are GenTL SFNC 1.2's, and so are the enumeration values, taken from its
//! reference XML (`TLType` `GigEVision` = 0, …).

use std::borrow::Cow;
use std::fmt::Write as _;
use std::sync::OnceLock;

use crate::abi::*;
use crate::error::{Failure, Result};

/// Where the first feature register is.
const FEATURE_BASE: u64 = 0x0001_0000;
/// Address space per feature: a string register, or an integer and, for a
/// selector, its maximum at [`MAX_OFFSET`].
const SLOT: u64 = 0x1000;
/// Bytes of a string register, its terminating 0 included.
pub(crate) const STR_LEN: usize = 1024;
/// Bytes of an integer register.
const INT_LEN: usize = 8;
/// Where a selector's maximum sits in its slot.
const MAX_OFFSET: u64 = 0x10;
/// Where the module's XML document is.
pub(crate) const XML_ADDRESS: u64 = 0x1000_0000;

/// How a feature's value is shown and reached.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Kind {
    /// `IString`, read-only.
    Str,
    /// `IInteger`, read-only, with a GenApi `<Representation>`.
    Int(Repr),
    /// `IBoolean`, read-only.
    Bool,
    /// `IEnumeration`, read-only: its entries and their values.
    Enum(&'static [(&'static str, i64)]),
    /// `IInteger`, read/write, from 0 to a maximum the module reports,
    /// selecting the features named.
    Selector(&'static [&'static str]),
    /// `ICommand`: writing executes it, and it is done when it returns.
    Command,
    /// `IInteger`, read/write: a timeout in milliseconds.
    Timeout,
}

/// GenApi's `<Representation>` for an integer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Repr {
    Number,
    Ipv4,
    Mac,
}

/// One feature of a module.
#[derive(Debug)]
pub(crate) struct Feature {
    pub(crate) name: &'static str,
    pub(crate) kind: Kind,
    pub(crate) tip: &'static str,
}

/// A feature's current value, from the module.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Value {
    Str(String),
    Int(i64),
}

/// A module's features, the categories that list them, and its XML.
#[derive(Debug)]
pub(crate) struct Layout {
    /// The XML's file name without extension, and its `ModelName`.
    pub(crate) file: &'static str,
    /// The `<Port>` its registers name (`TLPort`, `InterfacePort`, ...).
    pub(crate) port: &'static str,
    /// `ProductGuid` of the document.
    pub(crate) guid: &'static str,
    /// `Root` first, then every other category.
    pub(crate) categories: &'static [(&'static str, &'static [&'static str])],
    pub(crate) features: &'static [Feature],
    pub(crate) xml: OnceLock<String>,
}

/// Which register of a feature an address names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Register {
    /// The feature's value.
    Value,
    /// A selector's maximum.
    Max,
}

/// What a module answers about its features.
pub(crate) trait Features {
    fn layout(&self) -> &'static Layout;
    /// The value of `feature` (its `Register::Value`), or a selector's
    /// maximum (`Register::Max`).
    fn read_feature(&self, feature: &Feature, register: Register) -> Result<Value>;
    /// Write `value` to a writable feature: a selector, a command, a timeout.
    fn write_feature(&self, feature: &Feature, value: i64) -> Result<()>;
}

impl Layout {
    /// The URL `GCGetPortURLInfo` reports (§4.1.2.1, Table 4-1). The schema
    /// version is reported by `URL_INFO_SCHEMA_VER_*`, not in the URL, which
    /// carries it only for the legacy `GCGetPortURL`.
    pub(crate) fn url(&self) -> String {
        format!(
            "local:{}.xml;{:X};{:X}",
            self.file,
            XML_ADDRESS,
            self.xml().len()
        )
    }

    pub(crate) fn file_name(&self) -> String {
        format!("{}.xml", self.file)
    }

    /// The module's GenApi XML (schema 1.1).
    pub(crate) fn xml(&self) -> &str {
        self.xml.get_or_init(|| generate(self))
    }

    fn feature_at(&self, address: u64) -> Option<(usize, u64)> {
        let offset = address.checked_sub(FEATURE_BASE)?;
        let index = usize::try_from(offset / SLOT).ok()?;
        (index < self.features.len()).then_some((index, offset % SLOT))
    }

    fn address(index: usize) -> u64 {
        FEATURE_BASE + index as u64 * SLOT
    }
}

/// `GCReadPort` on a module's port: byte-aligned, any span inside one
/// register or the XML.
pub(crate) fn read(module: &dyn Features, address: u64, len: usize) -> Result<Vec<u8>> {
    let layout = module.layout();
    let xml = layout.xml().as_bytes();
    if let Some(start) = address.checked_sub(XML_ADDRESS) {
        let start = usize::try_from(start).map_err(|_| out_of_range(address, len))?;
        let end = start
            .checked_add(len)
            .ok_or_else(|| out_of_range(address, len))?;
        return xml
            .get(start..end)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| out_of_range(address, len));
    }
    let (index, offset) = layout
        .feature_at(address)
        .ok_or_else(|| out_of_range(address, len))?;
    let feature = &layout.features[index];
    let (register, start) = match (feature.kind, offset) {
        (Kind::Selector(_), o) if o >= MAX_OFFSET => (Register::Max, o - MAX_OFFSET),
        (_, o) => (Register::Value, o),
    };
    let bytes = register_bytes(feature, module.read_feature(feature, register)?);
    let start = start as usize;
    start
        .checked_add(len)
        .and_then(|end| bytes.get(start..end))
        .map(<[u8]>::to_vec)
        .ok_or_else(|| out_of_range(address, len))
}

/// `GCWritePort` on a module's port: a writable integer register, whole or
/// in part (read-modify-write, §4.1).
pub(crate) fn write(module: &dyn Features, address: u64, data: &[u8]) -> Result<()> {
    let layout = module.layout();
    let (index, offset) = layout
        .feature_at(address)
        .ok_or_else(|| out_of_range(address, data.len()))?;
    let feature = &layout.features[index];
    match feature.kind {
        Kind::Selector(_) | Kind::Command | Kind::Timeout if offset < INT_LEN as u64 => {}
        _ => {
            return Err(Failure::new(
                GC_ERR_ACCESS_DENIED,
                format!("{} is not writable (address {address:#x})", feature.name),
            ));
        }
    }
    let offset = offset as usize;
    if offset + data.len() > INT_LEN {
        return Err(out_of_range(address, data.len()));
    }
    let mut word = register_bytes(feature, module.read_feature(feature, Register::Value)?);
    word[offset..offset + data.len()].copy_from_slice(data);
    let value = i64::from_le_bytes(word[..INT_LEN].try_into().expect("8 bytes"));
    module.write_feature(feature, value)
}

fn out_of_range(address: u64, len: usize) -> Failure {
    Failure::new(
        GC_ERR_INVALID_ADDRESS,
        format!("no register holds {len} bytes at {address:#x}"),
    )
}

/// A register's bytes: a string NUL-padded to [`STR_LEN`], an integer as 8
/// little-endian bytes.
fn register_bytes(feature: &Feature, value: Value) -> Vec<u8> {
    match (feature.kind, value) {
        (Kind::Str, Value::Str(s)) => {
            let mut bytes = vec![0u8; STR_LEN];
            let text = s.as_bytes();
            let n = text.len().min(STR_LEN - 1);
            bytes[..n].copy_from_slice(&text[..n]);
            bytes
        }
        (_, Value::Int(v)) => v.to_le_bytes().to_vec(),
        (_, Value::Str(_)) => vec![0; INT_LEN],
    }
}

/// Escape text for XML element content.
fn escape(text: &str) -> Cow<'_, str> {
    if !text.contains(['&', '<', '>']) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;"),
    )
}

/// The XML for `layout`, in the element order the GenApi 1.1 schema fixes.
fn generate(layout: &Layout) -> String {
    let version: Vec<u32> = env!("CARGO_PKG_VERSION")
        .split('.')
        .map(|part| part.parse().unwrap_or(0))
        .chain(std::iter::repeat(0))
        .take(3)
        .collect();
    let port = layout.port;
    let mut xml = String::with_capacity(32 * 1024);
    let _ = write!(
        xml,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <RegisterDescription xmlns=\"http://www.genicam.org/GenApi/Version_1_1\" \
         xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" \
         xsi:schemaLocation=\"http://www.genicam.org/GenApi/Version_1_1 \
         http://www.genicam.org/GenApi/GenApiSchema_Version_1_1.xsd\" \
         ModelName=\"{model}\" VendorName=\"viva_genicam\" \
         ToolTip=\"viva-gentl module, GenTL SFNC 1.2 names\" StandardNameSpace=\"None\" \
         SchemaMajorVersion=\"1\" SchemaMinorVersion=\"1\" SchemaSubMinorVersion=\"0\" \
         MajorVersion=\"{major}\" MinorVersion=\"{minor}\" SubMinorVersion=\"{patch}\" \
         ProductGuid=\"{guid}\" VersionGuid=\"{guid}\">\n",
        model = layout.file,
        major = version[0],
        minor = version[1],
        patch = version[2],
        guid = layout.guid,
    );
    for (name, features) in layout.categories {
        let _ = writeln!(xml, "  <Category Name=\"{name}\" NameSpace=\"Standard\">");
        for feature in *features {
            let _ = writeln!(xml, "    <pFeature>{feature}</pFeature>");
        }
        xml.push_str("  </Category>\n");
    }
    for (index, feature) in layout.features.iter().enumerate() {
        let address = Layout::address(index);
        let name = feature.name;
        let tip = escape(feature.tip);
        match feature.kind {
            Kind::Str => {
                let _ = write!(
                    xml,
                    "  <StringReg Name=\"{name}\" NameSpace=\"Standard\">\n\
                     \x20   <ToolTip>{tip}</ToolTip>\n\
                     \x20   <Address>0x{address:X}</Address>\n\
                     \x20   <Length>{STR_LEN}</Length>\n\
                     \x20   <AccessMode>RO</AccessMode>\n\
                     \x20   <pPort>{port}</pPort>\n\
                     \x20   <Cachable>NoCache</Cachable>\n\
                     \x20 </StringReg>\n"
                );
            }
            Kind::Int(repr) => {
                int_reg(
                    &mut xml,
                    name,
                    "Standard",
                    Some(&tip),
                    address,
                    "RO",
                    port,
                    repr,
                );
            }
            Kind::Bool => {
                let _ = write!(
                    xml,
                    "  <Boolean Name=\"{name}\" NameSpace=\"Standard\">\n\
                     \x20   <ToolTip>{tip}</ToolTip>\n\
                     \x20   <ImposedAccessMode>RO</ImposedAccessMode>\n\
                     \x20   <pValue>{name}Reg</pValue>\n\
                     \x20   <OnValue>1</OnValue>\n\
                     \x20   <OffValue>0</OffValue>\n\
                     \x20 </Boolean>\n"
                );
                hidden_reg(&mut xml, name, "Reg", address, "RO", port);
            }
            Kind::Enum(entries) => {
                let _ = write!(
                    xml,
                    "  <Enumeration Name=\"{name}\" NameSpace=\"Standard\">\n\
                     \x20   <ToolTip>{tip}</ToolTip>\n\
                     \x20   <ImposedAccessMode>RO</ImposedAccessMode>\n"
                );
                for (entry, value) in entries {
                    let _ = write!(
                        xml,
                        "    <EnumEntry Name=\"{entry}\" NameSpace=\"Standard\">\n\
                         \x20     <Value>{value}</Value>\n\
                         \x20   </EnumEntry>\n"
                    );
                }
                let _ = write!(
                    xml,
                    "    <pValue>{name}Reg</pValue>\n\
                     \x20 </Enumeration>\n"
                );
                hidden_reg(&mut xml, name, "Reg", address, "RO", port);
            }
            Kind::Selector(selected) => {
                let _ = write!(
                    xml,
                    "  <Integer Name=\"{name}\" NameSpace=\"Standard\">\n\
                     \x20   <ToolTip>{tip}</ToolTip>\n\
                     \x20   <pValue>{name}Reg</pValue>\n\
                     \x20   <Min>0</Min>\n\
                     \x20   <pMax>{name}MaxReg</pMax>\n"
                );
                for feature in selected {
                    let _ = writeln!(xml, "    <pSelected>{feature}</pSelected>");
                }
                xml.push_str("  </Integer>\n");
                hidden_reg(&mut xml, name, "Reg", address, "RW", port);
                hidden_reg(&mut xml, name, "MaxReg", address + MAX_OFFSET, "RO", port);
            }
            Kind::Command => {
                let _ = write!(
                    xml,
                    "  <Command Name=\"{name}\" NameSpace=\"Standard\">\n\
                     \x20   <ToolTip>{tip}</ToolTip>\n\
                     \x20   <pValue>{name}Reg</pValue>\n\
                     \x20   <CommandValue>1</CommandValue>\n\
                     \x20 </Command>\n"
                );
                hidden_reg(&mut xml, name, "Reg", address, "RW", port);
            }
            Kind::Timeout => {
                let _ = write!(
                    xml,
                    "  <Integer Name=\"{name}\" NameSpace=\"Standard\">\n\
                     \x20   <ToolTip>{tip}</ToolTip>\n\
                     \x20   <pValue>{name}Reg</pValue>\n\
                     \x20   <Min>0</Min>\n\
                     \x20   <Max>{MAX_TIMEOUT_MS}</Max>\n\
                     \x20   <Unit>ms</Unit>\n\
                     \x20 </Integer>\n"
                );
                hidden_reg(&mut xml, name, "Reg", address, "RW", port);
            }
        }
    }
    let _ = write!(
        xml,
        "  <Port Name=\"{port}\" NameSpace=\"Standard\">\n\
         \x20   <ToolTip>The port through which this module's features are reached.</ToolTip>\n\
         \x20   <Visibility>Invisible</Visibility>\n\
         \x20 </Port>\n\
         </RegisterDescription>\n"
    );
    xml
}

/// The longest timeout a module's `*UpdateTimeout` feature accepts.
pub(crate) const MAX_TIMEOUT_MS: i64 = 10_000;

#[allow(clippy::too_many_arguments)]
fn int_reg(
    xml: &mut String,
    name: &str,
    namespace: &str,
    tip: Option<&str>,
    address: u64,
    access: &str,
    port: &str,
    repr: Repr,
) {
    let _ = writeln!(xml, "  <IntReg Name=\"{name}\" NameSpace=\"{namespace}\">");
    match tip {
        Some(tip) => {
            let _ = writeln!(xml, "    <ToolTip>{tip}</ToolTip>");
        }
        None => xml.push_str("    <Visibility>Invisible</Visibility>\n"),
    }
    let _ = write!(
        xml,
        "    <Address>0x{address:X}</Address>\n\
         \x20   <Length>{INT_LEN}</Length>\n\
         \x20   <AccessMode>{access}</AccessMode>\n\
         \x20   <pPort>{port}</pPort>\n\
         \x20   <Cachable>NoCache</Cachable>\n\
         \x20   <Sign>Unsigned</Sign>\n\
         \x20   <Endianess>LittleEndian</Endianess>\n"
    );
    match repr {
        Repr::Number => {}
        Repr::Ipv4 => xml.push_str("    <Representation>IPV4Address</Representation>\n"),
        Repr::Mac => xml.push_str("    <Representation>MACAddress</Representation>\n"),
    }
    xml.push_str("  </IntReg>\n");
}

/// The register behind a feature that is not itself a register.
fn hidden_reg(xml: &mut String, name: &str, suffix: &str, address: u64, access: &str, port: &str) {
    int_reg(
        xml,
        &format!("{name}{suffix}"),
        "Custom",
        None,
        address,
        access,
        port,
        Repr::Number,
    );
}

/// `TLType`, `InterfaceType`, `DeviceType`, `StreamType`: GenTL SFNC 1.2's
/// values, of which this producer is `GigEVision`.
pub(crate) const TL_TYPE_ENTRIES: &[(&str, i64)] = &[("GigEVision", 0)];
/// `GigEVision`'s value in [`TL_TYPE_ENTRIES`].
pub(crate) const GIGE_VISION: i64 = 0;

/// `DeviceAccessStatus` (GenTL SFNC 1.2; the values are
/// `DEVICE_ACCESS_STATUS`'s).
pub(crate) const ACCESS_STATUS_ENTRIES: &[(&str, i64)] = &[
    ("Unknown", DEVICE_ACCESS_STATUS_UNKNOWN as i64),
    ("ReadWrite", DEVICE_ACCESS_STATUS_READWRITE as i64),
    ("ReadOnly", DEVICE_ACCESS_STATUS_READONLY as i64),
    ("NoAccess", DEVICE_ACCESS_STATUS_NOACCESS as i64),
    ("Busy", DEVICE_ACCESS_STATUS_BUSY as i64),
    ("OpenReadWrite", DEVICE_ACCESS_STATUS_OPEN_READWRITE as i64),
    ("OpenReadOnly", DEVICE_ACCESS_STATUS_OPEN_READONLY as i64),
];

#[cfg(test)]
mod tests {
    use super::*;

    static TABLE: Layout = Layout {
        file: "test_module",
        port: "TestPort",
        guid: "00000000-0000-0000-0000-000000000000",
        categories: &[("Root", &["Name", "Selector", "Count"])],
        features: &[
            Feature {
                name: "Name",
                kind: Kind::Str,
                tip: "A <name> & more",
            },
            Feature {
                name: "Selector",
                kind: Kind::Selector(&["Count"]),
                tip: "Selects",
            },
            Feature {
                name: "Count",
                kind: Kind::Int(Repr::Number),
                tip: "Counts",
            },
        ],
        xml: OnceLock::new(),
    };

    struct Module(std::sync::Mutex<i64>);

    impl Features for Module {
        fn layout(&self) -> &'static Layout {
            &TABLE
        }
        fn read_feature(&self, feature: &Feature, register: Register) -> Result<Value> {
            Ok(match (feature.name, register) {
                ("Name", _) => Value::Str("viva".into()),
                ("Selector", Register::Value) => Value::Int(*self.0.lock().unwrap()),
                ("Selector", Register::Max) => Value::Int(3),
                (_, _) => Value::Int(0x0102_0304),
            })
        }
        fn write_feature(&self, _: &Feature, value: i64) -> Result<()> {
            *self.0.lock().unwrap() = value;
            Ok(())
        }
    }

    #[test]
    fn registers_read_byte_by_byte_and_strings_are_padded() {
        let module = Module(std::sync::Mutex::new(0));
        let name = read(&module, FEATURE_BASE, STR_LEN).unwrap();
        assert_eq!(&name[..5], b"viva\0");
        assert_eq!(name.len(), STR_LEN);
        let count = FEATURE_BASE + 2 * SLOT;
        assert_eq!(
            read(&module, count, 8).unwrap(),
            0x0102_0304i64.to_le_bytes()
        );
        assert_eq!(read(&module, count + 1, 2).unwrap(), vec![0x03, 0x02]);
        assert_eq!(
            read(&module, count + 4, 8).unwrap_err().code,
            GC_ERR_INVALID_ADDRESS,
            "past the register"
        );
        let max = FEATURE_BASE + SLOT + MAX_OFFSET;
        assert_eq!(read(&module, max, 8).unwrap(), 3i64.to_le_bytes());
    }

    #[test]
    fn writes_reach_writable_registers_only() {
        let module = Module(std::sync::Mutex::new(0));
        write(&module, FEATURE_BASE + SLOT, &2i64.to_le_bytes()).unwrap();
        assert_eq!(*module.0.lock().unwrap(), 2);
        // One byte of the eight, read-modify-write.
        write(&module, FEATURE_BASE + SLOT, &[1]).unwrap();
        assert_eq!(*module.0.lock().unwrap(), 1);
        let refused = write(&module, FEATURE_BASE + 2 * SLOT, &[1]).unwrap_err();
        assert_eq!(refused.code, GC_ERR_ACCESS_DENIED);
    }

    #[test]
    fn the_xml_is_served_at_its_address_and_escaped() {
        let module = Module(std::sync::Mutex::new(0));
        let xml = TABLE.xml();
        assert!(xml.contains("A &lt;name&gt; &amp; more"));
        assert_eq!(
            read(&module, XML_ADDRESS, xml.len()).unwrap(),
            xml.as_bytes()
        );
        assert!(TABLE.url().starts_with("local:test_module.xml;10000000;"));
        let model = viva_genicam::genapi::NodeMap::try_from_xml(
            viva_genapi_xml::parse(xml).expect("parses"),
        );
        assert!(model.is_ok(), "{:?}", model.err());
    }
}

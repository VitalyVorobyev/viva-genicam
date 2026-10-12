//! A feature's live state: what it holds and what it permits, read at once.
//!
//! A viewer needs more than a value to draw a control. It needs to know
//! whether the feature exists on this device, whether it is available in the
//! current selector state, whether it can be written now, its current range
//! and the enumeration entries the device will accept. [`NodeMap::feature_state`]
//! gathers all of it in one call, so every consumer — the camera services and
//! Viva Studio among them — computes it the same way ([ADR-0010], backlog
//! ST-26).
//!
//! [ADR-0010]: https://github.com/VitalyVorobyev/viva-genicam/blob/main/docs/adrs/adr0010-feature-state-contract.md

use std::collections::HashSet;

use tracing::{debug, warn};
use viva_genapi_xml::{AccessMode, CachingMode};

use crate::nodemap::Limit;
use crate::nodes::Node;
use crate::{GenApiError, NodeMap, RegisterIo, SkOutput};

/// The live state of one feature, read from the device in a single call.
///
/// See [`NodeMap::feature_state`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct FeatureState {
    /// GenICam node type, as [`Node::kind_name`] spells it.
    pub kind: &'static str,
    /// The current value, or [`FeatureValue::None`] for a feature that has
    /// none to read.
    pub value: FeatureValue,
    /// What the feature permits now: the declared mode along its `<pValue>`
    /// chain, made `RO` by a lock or by being unavailable.
    ///
    /// `None` means neither reading nor writing is possible, or the node has
    /// no access mode at all, as a category does.
    pub access: Option<AccessMode>,
    /// `pIsImplemented`: whether the device has this feature at all.
    pub is_implemented: bool,
    /// `pIsAvailable` and selector gating: whether the feature can be used in
    /// the device's current state.
    pub is_available: bool,
    /// Current bounds of an Integer, Float or converter feature; `None` when
    /// either bound is unknown — not declared, or declared through a `<pMin>`
    /// or `<pMax>` that could not be read. A converter's are its `<pValue>`'s
    /// mapped through `<FormulaFrom>` and `<Slope>`.
    pub range: Option<ValueRange>,
    /// The entries an Enumeration accepts now, after their own
    /// `pIsImplemented`/`pIsAvailable`.
    pub enum_entries: Option<Vec<String>>,
    /// Engineering unit, such as `"us"`.
    pub unit: Option<String>,
    /// How long a viewer may show this value before reading it again: the
    /// `<PollingTime>` of the feature or of a register its value is read
    /// through ([`NodeMap::polling_time`]). `None` when nothing polls it.
    pub polling_time: Option<std::time::Duration>,
    /// Whether, and how, its value may be answered from a cache
    /// ([`NodeMap::caching_mode`]). A `NoCache` feature without a polling
    /// time changes on the device with nothing to say when.
    pub caching: CachingMode,
}

/// A feature's current value, typed by the node that holds it.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum FeatureValue {
    /// Nothing to read: a category, a command, a write-only feature, a node a
    /// command writes through, or a feature not implemented or not available
    /// now.
    None,
    /// An Integer, IntConverter or integer SwissKnife.
    Integer(i64),
    /// A Float, Converter or float SwissKnife.
    Float(f64),
    /// A Boolean.
    Boolean(bool),
    /// The symbolic name of an Enumeration's current entry.
    Enum(String),
    /// A StringReg.
    String(String),
    /// A raw `<Register>`, reported by size rather than content.
    ///
    /// A register can be very large — the Micro-Epsilon scanCONTROL declares
    /// a 100 000-byte `FileAccessBuffer` — and viewers poll feature state, so
    /// copying the payload into every snapshot would be a poor trade for a
    /// value nothing renders. Read it with [`NodeMap::get_register`].
    Register {
        /// Declared length in bytes.
        len: u32,
    },
}

/// The current bounds of a numeric feature.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ValueRange {
    /// Smallest accepted value.
    pub min: f64,
    /// Largest accepted value.
    pub max: f64,
    /// Step between accepted values, when the feature declares one.
    pub inc: Option<f64>,
}

impl NodeMap {
    /// Read everything a viewer needs about `name` from the device: its value,
    /// what it permits now, whether it is implemented and available, its
    /// range, the enumeration entries it accepts, and its unit.
    ///
    /// Only a node that has a value is read. A write-only node refuses the
    /// read — locally, or with the device's `ACCESS_DENIED` — and a node a
    /// command writes through holds a trigger, not a value; some devices even
    /// treat reading one as significant (issues #112, #135). Both are reported
    /// with [`FeatureValue::None`] and their access mode, so a viewer still
    /// offers the write or the execute.
    ///
    /// A feature that is not implemented, or not available now, is not read
    /// either — not its value, its `<pMin>`/`<pMax>` or its entries'
    /// predicates. GenApi forbids value access on such a node, and cameras
    /// commonly refuse to read the register behind it. It is reported with
    /// [`FeatureValue::None`], its static range and its declared entries.
    ///
    /// A predicate that fails to evaluate is logged and treated as satisfied,
    /// so one bad `pIsAvailable` does not hide a feature; a failed value read
    /// is an error.
    pub fn feature_state(
        &self,
        name: &str,
        io: &dyn RegisterIo,
    ) -> Result<FeatureState, GenApiError> {
        let node = self
            .node(name)
            .ok_or_else(|| GenApiError::NodeNotFound(name.to_string()))?;

        let is_implemented = self.is_implemented(name, io).unwrap_or_else(|e| {
            warn!(%name, error = %e, "is_implemented eval failed");
            true
        });
        let is_available = self.is_available(name, io).unwrap_or_else(|e| {
            warn!(%name, error = %e, "is_available eval failed");
            true
        });
        let declared = self.declared_access_mode(name);
        let access = match self.effective_access_mode(name, io) {
            // An unavailable node with no access at all stays without one.
            Ok(_) if declared.is_none() => None,
            Ok(mode) => Some(mode),
            Err(e) => {
                warn!(%name, error = %e, "effective_access_mode eval failed");
                declared
            }
        };

        let usable = is_implemented && is_available;
        let has_value =
            usable && !self.is_write_only(name) && !self.command_targets().contains(name);
        // A chunk or event feature with nothing attached has no value now and
        // cannot be used: the port it reads is "not present" (GenICam
        // Standard v2.1.1 §2.8.16), as it is between frames.
        let mut detached = false;
        let value = if has_value {
            match self.read_value(node, io) {
                Err(GenApiError::NotAttached { .. }) => {
                    detached = true;
                    FeatureValue::None
                }
                other => other?,
            }
        } else {
            FeatureValue::None
        };
        let is_available = is_available && !detached;
        let usable = usable && !detached;
        let access = if detached { None } else { access };

        // A bound is reported only when something declares it: a `<Min>`,
        // a `<pMin>`, or one of those on the node `<pValue>` delegates to. A
        // feature that cannot be used now is not read, so only its own
        // constants are; a pointer that cannot be read leaves them too.
        let (range, unit) = match node {
            Node::Integer(n) => {
                let bound = |which: Limit, declared: Option<i64>| -> Option<i64> {
                    if !usable {
                        return declared;
                    }
                    match self.limit(name, which, io, &mut HashSet::new()) {
                        Ok(value) => value.map(|v| v.as_i64()),
                        Err(e) => {
                            warn!(%name, error = %e, ?which, "reading a bound failed");
                            declared
                        }
                    }
                };
                let (min, max, inc) = (
                    bound(Limit::Min, n.min),
                    bound(Limit::Max, n.max),
                    bound(Limit::Inc, n.inc),
                );
                let range = match (min, max) {
                    (Some(min), Some(max)) => Some(ValueRange {
                        min: min as f64,
                        max: max as f64,
                        inc: inc.map(|i| i as f64),
                    }),
                    _ => None,
                };
                (range, n.unit.clone())
            }
            Node::Float(_) | Node::Converter(_) | Node::IntConverter(_) => {
                let (declared_min, declared_max, unit) = match node {
                    Node::Float(n) => (n.min, n.max, n.unit.clone()),
                    _ => (None, None, None),
                };
                let is_converter = !matches!(node, Node::Float(_));
                let bound = |which: Limit, declared: Option<f64>| -> Option<f64> {
                    if !usable {
                        return declared;
                    }
                    match self.limit(name, which, io, &mut HashSet::new()) {
                        Ok(value) => value.map(|v| v.as_f64()),
                        // A converter's formula need not be defined at its
                        // raw bounds (`1 / TO` at 0); that is no fault.
                        Err(e) if is_converter => {
                            debug!(%name, error = %e, ?which, "converter bound undefined");
                            declared
                        }
                        Err(e) => {
                            warn!(%name, error = %e, ?which, "reading a bound failed");
                            declared
                        }
                    }
                };
                let range = match (
                    bound(Limit::Min, declared_min),
                    bound(Limit::Max, declared_max),
                ) {
                    (Some(min), Some(max)) => Some(ValueRange {
                        min,
                        max,
                        inc: None,
                    }),
                    _ => None,
                };
                (range, unit)
            }
            _ => (None, None),
        };

        let enum_entries = match node {
            Node::Enum(_) if !usable => self.enum_entries(name).ok(),
            Node::Enum(_) => self
                .available_enum_entries(name, io)
                .or_else(|e| {
                    warn!(%name, error = %e, "available_enum_entries eval failed");
                    self.enum_entries(name)
                })
                .ok(),
            _ => None,
        };

        Ok(FeatureState {
            kind: node.kind_name(),
            value,
            access,
            is_implemented,
            is_available,
            range,
            enum_entries,
            unit,
            polling_time: self.polling_time(name),
            caching: self.caching_mode(name).unwrap_or(CachingMode::WriteThrough),
        })
    }

    fn read_value(&self, node: &Node, io: &dyn RegisterIo) -> Result<FeatureValue, GenApiError> {
        let name = node.name();
        Ok(match node {
            Node::Integer(_) => FeatureValue::Integer(self.get_integer(name, io)?),
            Node::Float(_) => FeatureValue::Float(self.get_float(name, io)?),
            Node::Enum(_) => FeatureValue::Enum(self.get_enum(name, io)?),
            Node::Boolean(_) => FeatureValue::Boolean(self.get_bool(name, io)?),
            Node::String(_) => FeatureValue::String(self.get_string(name, io)?),
            Node::Register(reg) => FeatureValue::Register {
                len: reg.declared_len(),
            },
            Node::SwissKnife(sk) => match sk.output {
                SkOutput::Float => FeatureValue::Float(self.get_float(name, io)?),
                SkOutput::Integer => FeatureValue::Integer(self.get_integer(name, io)?),
            },
            Node::Converter(_) => FeatureValue::Float(self.get_converter(name, io)?),
            Node::IntConverter(_) => FeatureValue::Integer(self.get_int_converter(name, io)?),
            Node::Command(_) | Node::Category(_) => FeatureValue::None,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::{FeatureValue, ValueRange};
    use crate::{AccessMode, GenApiError, NodeMap, RegisterIo};

    /// Register memory; a write lands where a later read finds it. Reads of
    /// a `denied` address fail, as a camera refuses to read a register its
    /// feature has gated off.
    #[derive(Default)]
    struct MemIo {
        regs: RefCell<HashMap<u64, Vec<u8>>>,
        denied: Vec<u64>,
    }

    impl MemIo {
        fn with(regs: &[(u64, u32)]) -> Self {
            let io = Self::default();
            for &(addr, value) in regs {
                io.regs
                    .borrow_mut()
                    .insert(addr, value.to_be_bytes().to_vec());
            }
            io
        }

        fn denying(mut self, addrs: &[u64]) -> Self {
            self.denied = addrs.to_vec();
            self
        }
    }

    impl RegisterIo for MemIo {
        fn read(&self, addr: u64, len: usize) -> Result<Vec<u8>, GenApiError> {
            if self.denied.contains(&addr) {
                return Err(GenApiError::io(format!("read of {addr:#x} denied")));
            }
            Ok(self
                .regs
                .borrow()
                .get(&addr)
                .cloned()
                .unwrap_or_else(|| vec![0; len]))
        }

        fn write(&self, addr: u64, data: &[u8]) -> Result<(), GenApiError> {
            self.regs.borrow_mut().insert(addr, data.to_vec());
            Ok(())
        }
    }

    /// A write-only integer, an integer delegating to a write-only register
    /// (the `ActionDeviceKey` shape from #112), a command reaching an `RW`
    /// register through `<pValue>`, an integer over a read-only register,
    /// an enumeration whose second entry is unavailable, an integer whose
    /// maximum is read from the device, and one ordinary feature.
    const XML: &str = r#"
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
            <Integer Name="Width">
                <Address>0x100</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Min>16</Min>
                <pMax>WidthMax</pMax>
                <Inc>4</Inc>
            </Integer>
            <IntReg Name="WidthMax">
                <Address>0x104</Address>
                <Length>4</Length>
                <AccessMode>RO</AccessMode>
                <Sign>Unsigned</Sign>
                <Endianess>BigEndian</Endianess>
            </IntReg>
            <Integer Name="DeviceKey">
                <Address>0x200</Address>
                <Length>4</Length>
                <AccessMode>WO</AccessMode>
            </Integer>
            <Integer Name="ActionDeviceKey">
                <pValue>ActionDeviceKeyReg</pValue>
            </Integer>
            <IntReg Name="ActionDeviceKeyReg">
                <Address>0x300</Address>
                <Length>4</Length>
                <AccessMode>WO</AccessMode>
                <Sign>Unsigned</Sign>
                <Endianess>BigEndian</Endianess>
            </IntReg>
            <Command Name="TriggerSoftware">
                <pValue>TriggerSoftwareReg</pValue>
                <CommandValue>1</CommandValue>
            </Command>
            <IntReg Name="TriggerSoftwareReg">
                <Address>0x400</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Sign>Unsigned</Sign>
                <Endianess>BigEndian</Endianess>
            </IntReg>
            <Integer Name="SensorWidth">
                <pValue>SensorWidthReg</pValue>
            </Integer>
            <IntReg Name="SensorWidthReg">
                <Address>0x500</Address>
                <Length>4</Length>
                <AccessMode>RO</AccessMode>
                <Sign>Unsigned</Sign>
                <Endianess>BigEndian</Endianess>
            </IntReg>
            <Enumeration Name="PixelFormat">
                <EnumEntry Name="Mono8"><Value>1</Value></EnumEntry>
                <EnumEntry Name="Mono16">
                    <pIsAvailable>Mono16Available</pIsAvailable>
                    <Value>2</Value>
                </EnumEntry>
                <pValue>PixelFormatReg</pValue>
            </Enumeration>
            <IntReg Name="PixelFormatReg">
                <Address>0x600</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Sign>Unsigned</Sign>
                <Endianess>BigEndian</Endianess>
            </IntReg>
            <Float Name="FrameRate">
                <Address>0x900</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
            </Float>
            <Integer Name="Gain">
                <pIsAvailable>GainAvailable</pIsAvailable>
                <Address>0x800</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Min>0</Min>
                <pMax>GainMax</pMax>
            </Integer>
            <IntReg Name="GainMax">
                <Address>0x804</Address>
                <Length>4</Length>
                <AccessMode>RO</AccessMode>
                <Sign>Unsigned</Sign>
                <Endianess>BigEndian</Endianess>
            </IntReg>
            <IntReg Name="GainAvailable">
                <Address>0x700</Address>
                <Length>4</Length>
                <AccessMode>RO</AccessMode>
                <Sign>Unsigned</Sign>
                <Endianess>BigEndian</Endianess>
            </IntReg>
            <IntReg Name="Mono16Available">
                <Address>0x604</Address>
                <Length>4</Length>
                <AccessMode>RO</AccessMode>
                <Sign>Unsigned</Sign>
                <Endianess>BigEndian</Endianess>
            </IntReg>
        </RegisterDescription>
    "#;

    fn nodemap() -> NodeMap {
        let model = viva_genapi_xml::parse(XML).expect("parse");
        let nodemap = NodeMap::try_from_xml(model).expect("build nodemap");
        assert!(nodemap.skipped().is_empty(), "{:?}", nodemap.skipped());
        nodemap
    }

    fn io() -> MemIo {
        MemIo::with(&[(0x100, 640), (0x104, 1280), (0x500, 1920), (0x600, 1)])
    }

    #[test]
    fn an_ordinary_feature_reports_its_value_range_and_access() {
        let state = nodemap().feature_state("Width", &io()).expect("state");
        assert_eq!(state.kind, "Integer");
        assert_eq!(state.value, FeatureValue::Integer(640));
        assert_eq!(state.access, Some(AccessMode::RW));
        assert!(state.is_implemented && state.is_available);
        // `<pMax>` is read from the device, not left as the `i64` sentinel.
        assert_eq!(
            state.range,
            Some(ValueRange {
                min: 16.0,
                max: 1280.0,
                inc: Some(4.0)
            })
        );
    }

    /// Write-only nodes are reported without being read, so one of them no
    /// longer fails a whole snapshot (#135). `MemIo` would answer the read,
    /// so a value other than `None` here means the read was attempted.
    #[test]
    fn a_write_only_feature_is_reported_but_not_read() {
        let nodemap = nodemap();
        let io = io();
        for name in ["DeviceKey", "ActionDeviceKey"] {
            let state = nodemap.feature_state(name, &io).expect(name);
            assert_eq!(state.value, FeatureValue::None, "{name}");
            assert_eq!(state.access, Some(AccessMode::WO), "{name}");
        }
    }

    /// A command's backing register holds a trigger, not a value (#112).
    #[test]
    fn a_command_target_is_reported_but_not_read() {
        let state = nodemap()
            .feature_state("TriggerSoftwareReg", &io())
            .expect("state");
        assert_eq!(state.value, FeatureValue::None);
        assert_eq!(state.access, Some(AccessMode::RW));
    }

    /// An integer that declares no access of its own is as writable as the
    /// register it delegates to — here, not at all.
    #[test]
    fn a_feature_over_a_read_only_register_reports_ro() {
        let state = nodemap()
            .feature_state("SensorWidth", &io())
            .expect("state");
        assert_eq!(state.value, FeatureValue::Integer(1920));
        assert_eq!(state.access, Some(AccessMode::RO));
    }

    /// The entries offered are the ones the device accepts now (backlog
    /// ST-18): `Mono16` is gated by a register that reads 0.
    #[test]
    fn an_enumeration_offers_only_its_available_entries() {
        let nodemap = nodemap();
        let state = nodemap.feature_state("PixelFormat", &io()).expect("state");
        assert_eq!(state.value, FeatureValue::Enum("Mono8".into()));
        assert_eq!(state.enum_entries, Some(vec!["Mono8".to_string()]));

        // Negative control: once the gate opens, the entry is offered. A fresh
        // map, because the first one has cached the closed gate.
        let open = MemIo::with(&[(0x600, 1), (0x604, 1)]);
        let state = self::nodemap()
            .feature_state("PixelFormat", &open)
            .expect("state");
        assert_eq!(
            state.enum_entries,
            Some(vec!["Mono8".to_string(), "Mono16".to_string()]),
            "in the XML's order"
        );
    }

    /// A feature its `pIsAvailable` gates off is reported, not read: cameras
    /// commonly refuse to read such a register, and a failed read used to fail
    /// the whole snapshot, so the feature went missing from a bulk read
    /// instead of showing as unavailable (Codex review of #169).
    #[test]
    fn an_unavailable_feature_is_reported_but_not_read() {
        let gated_off = io().denying(&[0x800, 0x804]);
        let state = nodemap()
            .feature_state("Gain", &gated_off)
            .expect("an unavailable feature still has a state");
        assert!(!state.is_available);
        assert_eq!(state.value, FeatureValue::None);
        assert_eq!(state.access, Some(AccessMode::RO));
        // `<pMax>` went unread, so there is no maximum to report, and a range
        // made of the parser's `i64::MAX` placeholder would be a lie (Codex
        // review of #170).
        assert_eq!(state.range, None);

        // Negative control: with the gate open the value and `<pMax>` are read.
        let open = MemIo::with(&[(0x700, 1), (0x800, 6), (0x804, 24)]);
        let state = nodemap().feature_state("Gain", &open).expect("state");
        assert!(state.is_available);
        assert_eq!(state.value, FeatureValue::Integer(6));
        assert_eq!(state.range.map(|r| r.max), Some(24.0));
    }

    /// A Float that declares no bounds has none to report: the parser's
    /// `f64::MIN`/`f64::MAX` placeholders are not a range.
    #[test]
    fn a_feature_without_bounds_reports_no_range() {
        let state = nodemap().feature_state("FrameRate", &io()).expect("state");
        assert_eq!(state.value, FeatureValue::Float(0.0));
        assert_eq!(state.range, None);
    }

    #[test]
    fn an_unknown_name_is_an_error() {
        assert!(matches!(
            nodemap().feature_state("Nope", &io()),
            Err(GenApiError::NodeNotFound(_))
        ));
    }
}

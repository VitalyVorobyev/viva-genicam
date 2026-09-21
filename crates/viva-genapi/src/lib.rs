#![cfg_attr(docsrs, feature(doc_cfg))]
//! GenApi node system: typed feature access backed by register IO.

mod bitops;
mod conversions;
mod error;
mod io;
mod nodemap;
mod nodes;
pub mod swissknife;

pub use error::GenApiError;
pub use io::{NullIo, RegisterIo};
pub use nodemap::NodeMap;
pub use nodes::{
    BooleanNode, CategoryNode, CommandNode, EnumNode, FloatNode, IntegerNode, Node, NodeMeta,
    RegisterNode, Representation, SkNode, Visibility,
};
pub use swissknife::{AstNode, EvalMode, Value};
pub use viva_genapi_xml::{AccessMode, SkOutput, SkippedNode};

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use crate::conversions::{bytes_to_i64, i64_to_bytes};
    use crate::{AccessMode, GenApiError, NodeMap, RegisterIo, Visibility};
    use viva_genapi_xml::{ByteOrder, Sign};

    const FIXTURE: &str = r#"
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="2" SchemaSubMinorVersion="3">
            <Integer Name="Width">
                <Address>0x100</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Min>16</Min>
                <Max>4096</Max>
                <Inc>2</Inc>
            </Integer>
            <Float Name="ExposureTime">
                <Address>0x200</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Min>10.0</Min>
                <Max>100000.0</Max>
                <Scale>1/1000</Scale>
            </Float>
            <Enumeration Name="GainSelector">
                <Address>0x300</Address>
                <Length>2</Length>
                <AccessMode>RW</AccessMode>
                <EnumEntry Name="All" Value="0" />
                <EnumEntry Name="Red" Value="1" />
                <EnumEntry Name="Blue" Value="2" />
            </Enumeration>
            <Integer Name="Gain">
                <Length>2</Length>
                <AccessMode>RW</AccessMode>
                <Min>0</Min>
                <Max>48</Max>
                <pSelected>GainSelector</pSelected>
                <Selected>All</Selected>
                <Address>0x310</Address>
                <Selected>Red</Selected>
                <Address>0x314</Address>
                <Selected>Blue</Selected>
            </Integer>
            <Boolean Name="GammaEnable">
                <Address>0x400</Address>
                <Length>1</Length>
                <AccessMode>RW</AccessMode>
            </Boolean>
            <Command Name="AcquisitionStart">
                <Address>0x500</Address>
                <Length>4</Length>
            </Command>
        </RegisterDescription>
    "#;

    const INDIRECT_FIXTURE: &str = r#"
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
            <Integer Name="RegAddr">
                <Address>0x2000</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Min>0</Min>
                <Max>65535</Max>
            </Integer>
            <Integer Name="Gain">
                <pAddress>RegAddr</pAddress>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Min>0</Min>
                <Max>255</Max>
            </Integer>
        </RegisterDescription>
    "#;

    const ENUM_PVALUE_FIXTURE: &str = r#"
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
            <Enumeration Name="Mode">
                <Address>0x4000</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <EnumEntry Name="Fixed10">
                    <Value>10</Value>
                </EnumEntry>
                <EnumEntry Name="DynFromReg">
                    <pValue>RegModeVal</pValue>
                </EnumEntry>
            </Enumeration>
            <Integer Name="RegModeVal">
                <Address>0x4100</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Min>0</Min>
                <Max>65535</Max>
            </Integer>
        </RegisterDescription>
    "#;

    const BITFIELD_FIXTURE: &str = r#"
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
            <Integer Name="LeByte">
                <Address>0x5000</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Min>0</Min>
                <Max>65535</Max>
                <Mask>0x0000FF00</Mask>
            </Integer>
            <!-- The top three bits of a big-endian 16-bit word. Indices count
                 down from the MSB here, so the field is 0..=2 and `<LSB>` is the
                 larger of the two. This used to read `<Lsb>13</Lsb><Msb>15</Msb>`,
                 an orientation that appears nowhere in the vendor corpus and
                 encoded the issue-#120 defect rather than catching it. -->
            <Integer Name="BeBits">
                <Address>0x5004</Address>
                <Length>2</Length>
                <AccessMode>RW</AccessMode>
                <Min>0</Min>
                <Max>15</Max>
                <LSB>2</LSB>
                <MSB>0</MSB>
                <Endianess>BigEndian</Endianess>
            </Integer>
            <Boolean Name="PackedFlag">
                <Address>0x5006</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Bit>13</Bit>
            </Boolean>
        </RegisterDescription>
    "#;

    const SWISSKNIFE_FIXTURE: &str = r#"
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
            <Integer Name="GainRaw">
                <Address>0x3000</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Min>0</Min>
                <Max>1000</Max>
            </Integer>
            <Float Name="Offset">
                <Address>0x3008</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Min>-100.0</Min>
                <Max>100.0</Max>
                <Scale>1</Scale>
            </Float>
            <Integer Name="B">
                <Address>0x3010</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Min>-1000</Min>
                <Max>1000</Max>
            </Integer>
            <SwissKnife Name="ComputedGain">
                <Formula>(GainRaw * 0.5) + Offset</Formula>
                <pVariable Name="GainRaw">GainRaw</pVariable>
                <pVariable Name="Offset">Offset</pVariable>
            </SwissKnife>
            <IntSwissKnife Name="DivideInt">
                <Formula>GainRaw / 3</Formula>
                <pVariable Name="GainRaw">GainRaw</pVariable>
            </IntSwissKnife>
            <IntSwissKnife Name="Unary">
                <Formula>-GainRaw + 10</Formula>
                <pVariable Name="GainRaw">GainRaw</pVariable>
            </IntSwissKnife>
            <SwissKnife Name="DivideByZero">
                <Formula>GainRaw / B</Formula>
                <pVariable Name="GainRaw">GainRaw</pVariable>
                <pVariable Name="B">B</pVariable>
            </SwissKnife>
        </RegisterDescription>
    "#;

    #[derive(Default)]
    struct MockIo {
        regs: RefCell<HashMap<u64, Vec<u8>>>,
        reads: RefCell<HashMap<u64, usize>>,
    }

    impl MockIo {
        fn with_registers(entries: &[(u64, Vec<u8>)]) -> Self {
            let mut regs = HashMap::new();
            for (addr, data) in entries {
                regs.insert(*addr, data.clone());
            }
            MockIo {
                regs: RefCell::new(regs),
                reads: RefCell::new(HashMap::new()),
            }
        }

        fn read_count(&self, addr: u64) -> usize {
            *self.reads.borrow().get(&addr).unwrap_or(&0)
        }
    }

    impl RegisterIo for MockIo {
        fn read(&self, addr: u64, len: usize) -> Result<Vec<u8>, GenApiError> {
            let mut reads = self.reads.borrow_mut();
            *reads.entry(addr).or_default() += 1;
            let regs = self.regs.borrow();
            let data = regs
                .get(&addr)
                .ok_or_else(|| GenApiError::Io(format!("read miss at 0x{addr:08X}")))?;
            if data.len() != len {
                return Err(GenApiError::Io(format!(
                    "length mismatch at 0x{addr:08X}: expected {len}, have {}",
                    data.len()
                )));
            }
            Ok(data.clone())
        }

        fn write(&self, addr: u64, data: &[u8]) -> Result<(), GenApiError> {
            self.regs.borrow_mut().insert(addr, data.to_vec());
            Ok(())
        }
    }

    fn build_nodemap() -> NodeMap {
        let model = viva_genapi_xml::parse(FIXTURE).expect("parse fixture");
        NodeMap::try_from_xml(model).expect("build nodemap")
    }

    /// A node type we do not implement must reach the nodemap's skip list.
    ///
    /// Two separate holes used to swallow this. The XML parser dropped an
    /// unlisted tag at `skip_element` without recording it, and the nodemap
    /// then discarded whatever the XML layer *had* recorded — so a consumer
    /// holding a nodemap could not tell a feature we cannot read from one the
    /// camera does not have.
    #[test]
    fn an_unsupported_node_type_is_visible_in_the_nodemap() {
        // `<ConfRom>` is a real GenICam node type we do not implement.
        // This test used `<Register>` until GA-09 landed support for it.
        const WITH_UNKNOWN: &str = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="0">
                <Integer Name="Width">
                    <Address>0x100</Address>
                    <Length>4</Length>
                    <AccessMode>RW</AccessMode>
                </Integer>
                <ConfRom Name="DeviceConfRom">
                    <Address>0x2000</Address>
                    <Length>512</Length>
                </ConfRom>
            </RegisterDescription>
        "#;
        let model = viva_genapi_xml::parse(WITH_UNKNOWN).expect("parse");
        assert_eq!(model.skipped.len(), 1, "XML layer records the unknown tag");

        let nodemap = NodeMap::try_from_xml(model).expect("build nodemap");
        let io = MockIo::with_registers(&[(0x100, vec![0, 0, 0, 7])]);
        assert_eq!(nodemap.get_integer("Width", &io).expect("read Width"), 7);

        let skipped = nodemap.skipped();
        assert_eq!(skipped.len(), 1, "and the nodemap carries it forward");
        assert_eq!(skipped[0].tag, "ConfRom");
        assert_eq!(skipped[0].name.as_deref(), Some("DeviceConfRom"));
    }

    /// `<Register>` fixture covering the three shapes GA-09's first cut cares
    /// about: a readable/writable block on the device port, one on a chunk
    /// port, and one whose length is resolved at runtime.
    const REGISTER_FIXTURE: &str = r#"
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="0">
            <Register Name="FileAccessBuffer">
                <Address>0x2000</Address>
                <Length>8</Length>
                <AccessMode>RW</AccessMode>
            </Register>
            <Register Name="DeviceSerialBlock">
                <Address>0x3000</Address>
                <Length>4</Length>
                <AccessMode>RO</AccessMode>
                <pPort>Device</pPort>
            </Register>
            <Register Name="ChunkMeasurementResults">
                <Address>0x0</Address>
                <Length>4</Length>
                <AccessMode>RO</AccessMode>
                <pPort>Chunk4007</pPort>
            </Register>
            <Register Name="DynamicBlock">
                <Address>0x4000</Address>
                <pLength>BlockLength</pLength>
            </Register>
            <StringReg Name="DeviceVendorName">
                <Address>0x5000</Address>
                <Length>4</Length>
                <AccessMode>RO</AccessMode>
            </StringReg>
        </RegisterDescription>
    "#;

    fn build_register_nodemap() -> NodeMap {
        let model = viva_genapi_xml::parse(REGISTER_FIXTURE).expect("parse register fixture");
        NodeMap::try_from_xml(model).expect("build nodemap")
    }

    #[test]
    fn register_reads_and_writes_raw_bytes() {
        let nodemap = build_register_nodemap();
        let payload = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x02, 0x03, 0x04];
        let io = MockIo::with_registers(&[(0x2000, payload.clone())]);

        assert_eq!(
            nodemap
                .get_register("FileAccessBuffer", &io)
                .expect("read register"),
            payload
        );

        let replacement = vec![9u8; 8];
        nodemap
            .set_register("FileAccessBuffer", &replacement, &io)
            .expect("write register");
        assert_eq!(
            nodemap
                .get_register("FileAccessBuffer", &io)
                .expect("re-read register"),
            replacement
        );
    }

    /// A short write must be refused, not padded.
    ///
    /// `set_string` zero-pads to the declared length, which is right for a
    /// string. Doing the same to a file-transfer buffer would silently zero
    /// the rest of the block — data loss dressed as a convenience.
    #[test]
    fn register_write_of_the_wrong_length_is_refused() {
        let nodemap = build_register_nodemap();
        let io = MockIo::with_registers(&[(0x2000, vec![0u8; 8])]);

        let err = nodemap
            .set_register("FileAccessBuffer", &[1, 2, 3], &io)
            .expect_err("a 3-byte write into an 8-byte register must fail");
        assert!(
            matches!(err, GenApiError::Range(_)),
            "expected a range error, got {err:?}"
        );
        assert_eq!(
            io.read(0x2000, 8).expect("register untouched"),
            vec![0u8; 8],
            "the refused write must not have reached the device"
        );
    }

    /// A chunk-port register is listed but cannot be read.
    ///
    /// Its address is relative to a port we do not route. Reading it through
    /// the device port would not error — it would return whatever lives at
    /// that address, which for a scanCONTROL's `0x0` is the GVCP bootstrap
    /// area. Silent wrong answers are what ADR-0018 exists to refuse (GA-12).
    #[test]
    fn register_on_a_non_device_port_is_listed_but_not_readable() {
        let nodemap = build_register_nodemap();
        let io = MockIo::with_registers(&[(0x0, vec![1, 2, 3, 4])]);

        assert!(
            nodemap.node_names().any(|n| n == "ChunkMeasurementResults"),
            "the node must still be visible for introspection"
        );

        let err = nodemap
            .get_register("ChunkMeasurementResults", &io)
            .expect_err("a chunk-port register must not be read through the device port");
        let text = err.to_string();
        assert!(
            text.contains("Chunk4007") && text.contains("GA-12"),
            "the error must name the port and the reason: {text}"
        );

        // An explicit <pPort>Device</pPort> is the device port and reads fine.
        let io = MockIo::with_registers(&[(0x3000, vec![5, 6, 7, 8])]);
        assert_eq!(
            nodemap
                .get_register("DeviceSerialBlock", &io)
                .expect("explicit Device port reads"),
            vec![5, 6, 7, 8]
        );
    }

    /// `register_address` (#92) keeps working, and now covers `<Register>`.
    #[test]
    fn register_address_resolves_for_a_register_node() {
        let nodemap = build_register_nodemap();
        let io = MockIo::default();
        assert_eq!(
            nodemap
                .register_address("FileAccessBuffer", &io)
                .expect("resolve address"),
            (0x2000, 8)
        );
    }

    #[test]
    fn register_accessors_reject_the_wrong_node_type() {
        let nodemap = build_register_nodemap();
        let io = MockIo::with_registers(&[(0x5000, b"AVT\0".to_vec())]);

        assert!(matches!(
            nodemap.get_register("DeviceVendorName", &io),
            Err(GenApiError::Type(_))
        ));
        assert!(matches!(
            nodemap.get_register("NoSuchNode", &io),
            Err(GenApiError::NodeNotFound(_))
        ));
    }

    /// The deferred half of GA-09 must name itself in the skip reason.
    ///
    /// The corpus allowlist matches on this substring to tell a known gap from
    /// a regression, so the wording is load-bearing, not cosmetic.
    #[test]
    fn a_register_with_p_length_is_skipped_and_says_why() {
        let nodemap = build_register_nodemap();
        let skipped = nodemap.skipped();

        let entry = skipped
            .iter()
            .find(|s| s.name.as_deref() == Some("DynamicBlock"))
            .expect("a <pLength> register must be recorded, not silently dropped");
        assert_eq!(entry.tag, "Register");
        assert!(
            entry.error.contains("<pLength>"),
            "the skip reason must name <pLength>: {}",
            entry.error
        );

        // The other four nodes still built.
        assert_eq!(nodemap.node_names().count(), 4);
    }

    /// Structural elements are not features and must not be reported as lost.
    #[test]
    fn elements_without_a_name_are_not_reported_as_skipped() {
        const WITH_PORT: &str = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="0">
                <Port Name="Device"/>
                <Group Comment="Standard">
                    <Integer Name="Width">
                        <Address>0x100</Address>
                        <Length>4</Length>
                        <AccessMode>RW</AccessMode>
                    </Integer>
                </Group>
            </RegisterDescription>
        "#;
        let model = viva_genapi_xml::parse(WITH_PORT).expect("parse");
        assert!(model.skipped.is_empty(), "{:?}", model.skipped);
        assert_eq!(model.nodes.len(), 1);
    }

    fn build_indirect_nodemap() -> NodeMap {
        let model = viva_genapi_xml::parse(INDIRECT_FIXTURE).expect("parse indirect fixture");
        NodeMap::try_from_xml(model).expect("build nodemap")
    }

    fn build_enum_pvalue_nodemap() -> NodeMap {
        let model = viva_genapi_xml::parse(ENUM_PVALUE_FIXTURE).expect("parse enum pvalue fixture");
        NodeMap::try_from_xml(model).expect("build nodemap")
    }

    fn build_bitfield_nodemap() -> NodeMap {
        let model = viva_genapi_xml::parse(BITFIELD_FIXTURE).expect("parse bitfield fixture");
        NodeMap::try_from_xml(model).expect("build nodemap")
    }

    fn build_swissknife_nodemap() -> NodeMap {
        let model = viva_genapi_xml::parse(SWISSKNIFE_FIXTURE).expect("parse swissknife fixture");
        NodeMap::try_from_xml(model).expect("build nodemap")
    }

    #[test]
    fn integer_roundtrip_and_cache() {
        let mut nodemap = build_nodemap();
        let io = MockIo::with_registers(&[(0x100, vec![0, 0, 4, 0])]);
        let width = nodemap.get_integer("Width", &io).expect("read width");
        assert_eq!(width, 1024);
        assert_eq!(io.read_count(0x100), 1);
        let width_again = nodemap.get_integer("Width", &io).expect("cached width");
        assert_eq!(width_again, 1024);
        assert_eq!(io.read_count(0x100), 1, "cached value should be reused");
        nodemap
            .set_integer("Width", 1030, &io)
            .expect("write width");
        let width = nodemap
            .get_integer("Width", &io)
            .expect("read updated width");
        assert_eq!(width, 1030);
        assert_eq!(io.read_count(0x100), 1, "write should update cache");
    }

    const IEEE754_FIXTURE: &str = r#"
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
            <FloatReg Name="FrameRate">
                <Address>0x100</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Min>0.0</Min>
                <Max>1000.0</Max>
                <Endianess>BigEndian</Endianess>
            </FloatReg>
            <Float Name="ExposureUs">
                <Address>0x110</Address>
                <Length>8</Length>
                <AccessMode>RW</AccessMode>
                <Min>0.0</Min>
                <Max>1000000.0</Max>
                <Endianess>BigEndian</Endianess>
            </Float>
        </RegisterDescription>
    "#;

    fn build_ieee754_nodemap() -> NodeMap {
        NodeMap::try_from_xml(viva_genapi_xml::parse(IEEE754_FIXTURE).expect("parse ieee754"))
            .expect("build nodemap")
    }

    #[test]
    fn float_ieee754_f32_roundtrip() {
        let mut nodemap = build_ieee754_nodemap();
        let io = MockIo::with_registers(&[(0x100, 30.0f32.to_be_bytes().to_vec())]);
        let v = nodemap.get_float("FrameRate", &io).expect("read rate");
        assert!((v - 30.0).abs() < 1e-3, "got {v}");

        nodemap
            .set_float("FrameRate", 42.5, &io)
            .expect("write rate");
        let raw = io.read(0x100, 4).expect("read back");
        assert_eq!(raw, 42.5f32.to_be_bytes());
    }

    #[test]
    fn float_ieee754_f64_heuristic_roundtrip() {
        let mut nodemap = build_ieee754_nodemap();
        let io = MockIo::with_registers(&[(0x110, 6000.0f64.to_be_bytes().to_vec())]);
        let v = nodemap.get_float("ExposureUs", &io).expect("read exposure");
        assert!((v - 6000.0).abs() < 1e-9, "got {v}");

        nodemap
            .set_float("ExposureUs", 5000.0, &io)
            .expect("write exposure");
        let raw = io.read(0x110, 8).expect("read back");
        assert_eq!(raw, 5000.0f64.to_be_bytes());
    }

    #[test]
    fn float_scaled_integer_preserved() {
        // The classic fixture's ExposureTime uses <Scale>1/1000</Scale>,
        // so it must stay on the scaled-integer path even after the heuristic.
        let nodemap = build_nodemap();
        let raw = 50_000i64;
        let io = MockIo::with_registers(&[(
            0x200,
            i64_to_bytes("ExposureTime", raw, 4, Sign::Signed, ByteOrder::Big).unwrap(),
        )]);
        let exposure = nodemap
            .get_float("ExposureTime", &io)
            .expect("read exposure");
        assert!((exposure - 50.0).abs() < 1e-6);
    }

    const PREDICATE_FIXTURE: &str = r#"
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
            <IntReg Name="CtrlReg">
                <Address>0x400</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Sign>Unsigned</Sign>
                <Endianess>BigEndian</Endianess>
            </IntReg>
            <IntSwissKnife Name="GateImplemented">
                <Formula>CTRL &amp; 1</Formula>
                <pVariable Name="CTRL">CtrlReg</pVariable>
                <Output>Integer</Output>
            </IntSwissKnife>
            <IntSwissKnife Name="GateLocked">
                <Formula>(CTRL &amp; 2) / 2</Formula>
                <pVariable Name="CTRL">CtrlReg</pVariable>
                <Output>Integer</Output>
            </IntSwissKnife>
            <IntSwissKnife Name="Entry8Implemented">
                <Formula>(CTRL &amp; 4) / 4</Formula>
                <pVariable Name="CTRL">CtrlReg</pVariable>
                <Output>Integer</Output>
            </IntSwissKnife>
            <Integer Name="Gated">
                <Address>0x410</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Min>0</Min>
                <Max>255</Max>
                <Sign>Unsigned</Sign>
                <Endianess>BigEndian</Endianess>
                <pIsImplemented>GateImplemented</pIsImplemented>
                <pIsLocked>GateLocked</pIsLocked>
            </Integer>
            <Enumeration Name="PixelFormat">
                <EnumEntry Name="Mono8"><Value>1</Value></EnumEntry>
                <EnumEntry Name="Mono16">
                    <Value>2</Value>
                    <pIsImplemented>Entry8Implemented</pIsImplemented>
                </EnumEntry>
                <pValue>PixelFormatReg</pValue>
            </Enumeration>
            <IntReg Name="PixelFormatReg">
                <Address>0x420</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Sign>Unsigned</Sign>
                <Endianess>BigEndian</Endianess>
            </IntReg>
        </RegisterDescription>
    "#;

    /// The FLIR gating shape from issue #120: three `<MaskedIntReg>` predicates
    /// sharing one big-endian word, plus a `<Float>` that delegates through a
    /// `<Converter>` — reproduced from `FLIR_BFS_PGE_31S4C_C.xml`.
    const BIG_ENDIAN_PREDICATE_FIXTURE: &str = r#"
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="1">
            <Float Name="ExposureTime">
                <pIsImplemented>ExposureTime_Imp</pIsImplemented>
                <pIsAvailable>ExposureTime_Avl</pIsAvailable>
                <pIsLocked>ExposureTime_Lck</pIsLocked>
                <pValue>ExposureTime_FloatVal</pValue>
            </Float>
            <Converter Name="ExposureTime_FloatVal">
                <FormulaTo>FROM</FormulaTo>
                <FormulaFrom>TO</FormulaFrom>
                <pValue>ExposureTime_Val</pValue>
                <Slope>Increasing</Slope>
            </Converter>
            <IntReg Name="ExposureTime_Val">
                <Address>0x000C1004</Address><Length>4</Length>
                <AccessMode>RW</AccessMode><Sign>Unsigned</Sign>
                <Endianess>BigEndian</Endianess>
            </IntReg>
            <MaskedIntReg Name="ExposureTime_Imp">
                <Address>0x000C1000</Address><Length>4</Length>
                <AccessMode>RO</AccessMode><Bit>0</Bit>
                <Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
            </MaskedIntReg>
            <MaskedIntReg Name="ExposureTime_Avl">
                <Address>0x000C1000</Address><Length>4</Length>
                <AccessMode>RO</AccessMode><Bit>1</Bit>
                <Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
            </MaskedIntReg>
            <MaskedIntReg Name="ExposureTime_Lck">
                <Address>0x000C1000</Address><Length>4</Length>
                <AccessMode>RO</AccessMode><Bit>3</Bit>
                <Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
            </MaskedIntReg>
        </RegisterDescription>
    "#;

    fn big_endian_predicate_io(status: u32) -> MockIo {
        MockIo::with_registers(&[
            (0x000C_1000, status.to_be_bytes().to_vec()),
            (0x000C_1004, 10_000u32.to_be_bytes().to_vec()),
        ])
    }

    /// Regression for issue #120, stated against a register whose layout the
    /// GigE Vision standard fixes rather than against our own reading of it.
    ///
    /// `AVT_Manta_G125B.xml` declares bootstrap `GevSCPSPacketSize` at `0xD04`
    /// as `<LSB>31</LSB><MSB>16</MSB>` big-endian. The standard puts the packet
    /// size in the *low* 16 bits, and the #120 reporter's own diagnostic bundle
    /// shows that register holding `0x40000578` — 1400 bytes, with bit 0 (the
    /// MSB) set for "fire test packet".
    ///
    /// So 1400 is not a number we chose: any implementation that disagrees is
    /// wrong. Before the fix this produced 1 073 742 200 — the whole word —
    /// because `<LSB>`/`<MSB>` were matched only in their mixed-case spelling
    /// and the bit range was dropped entirely.
    #[test]
    fn big_endian_lsb_msb_range_decodes_the_gige_packet_size_register() {
        const XML: &str = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="1">
                <MaskedIntReg Name="RegSCPSPacketSize">
                    <Address>0xD04</Address><Length>4</Length>
                    <AccessMode>RW</AccessMode>
                    <LSB>31</LSB><MSB>16</MSB>
                    <Endianess>BigEndian</Endianess>
                </MaskedIntReg>
            </RegisterDescription>
        "#;
        let nodemap =
            NodeMap::try_from_xml(viva_genapi_xml::parse(XML).expect("parse")).expect("nodemap");
        let io = MockIo::with_registers(&[(0x0D04, 0x4000_0578u32.to_be_bytes().to_vec())]);

        assert_eq!(
            nodemap.get_integer("RegSCPSPacketSize", &io).expect("read"),
            1400
        );
    }

    /// `<StructEntry>` and `<MaskedIntReg>` are two spellings of the same
    /// physical bit and must decode identically.
    ///
    /// They are parsed by different functions — `parsers::struct_reg` and
    /// `parsers::numeric` — and until #120 those two disagreed, with the
    /// `<StructEntry>` path correct. Every predicate in `viva-fake-gige` used
    /// `<StructEntry>` or `<IntSwissKnife>`, so the whole suite passed on the
    /// path that worked while real cameras used the one that did not.
    #[test]
    fn struct_entry_and_masked_int_reg_agree_on_a_big_endian_bit() {
        const XML: &str = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="1">
                <MaskedIntReg Name="ViaMaskedIntReg">
                    <Address>0x500</Address><Length>4</Length>
                    <AccessMode>RO</AccessMode><Bit>1</Bit>
                    <Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
                </MaskedIntReg>
                <StructReg Comment="same word">
                    <Address>0x500</Address><Length>4</Length>
                    <AccessMode>RO</AccessMode><Sign>Unsigned</Sign>
                    <Endianess>BigEndian</Endianess>
                    <StructEntry Name="ViaStructEntry"><Bit>1</Bit></StructEntry>
                </StructReg>
            </RegisterDescription>
        "#;
        let nodemap =
            NodeMap::try_from_xml(viva_genapi_xml::parse(XML).expect("parse")).expect("nodemap");

        for word in [0x4000_0000u32, 0x0000_0002, 0xFFFF_FFFF, 0x0000_0000] {
            let io = MockIo::with_registers(&[(0x500, word.to_be_bytes().to_vec())]);
            let masked = nodemap.get_integer("ViaMaskedIntReg", &io).expect("masked");
            let entry = nodemap.get_integer("ViaStructEntry", &io).expect("entry");
            assert_eq!(masked, entry, "0x{word:08X} decoded differently");
        }
    }

    /// The end-to-end shape of issue #120: a write refused locally as
    /// unavailable because the gating bits were read off the wrong end.
    ///
    /// `0xC000_0000` is implemented (bit 0 from the MSB) and available (bit 1),
    /// with the lock bit (bit 3) clear — the state a FLIR camera is in with
    /// `ExposureAuto=Off`. Read LSB-first, as before the fix, the same word says
    /// not implemented, and every setter refuses before reaching the wire.
    #[test]
    fn big_endian_gating_bits_permit_the_write_they_describe() {
        let mut nodemap = NodeMap::try_from_xml(
            viva_genapi_xml::parse(BIG_ENDIAN_PREDICATE_FIXTURE).expect("parse"),
        )
        .expect("nodemap");
        let io = big_endian_predicate_io(0xC000_0000);

        assert!(nodemap.is_implemented("ExposureTime", &io).expect("imp"));
        assert!(nodemap.is_available("ExposureTime", &io).expect("avl"));
        assert_eq!(
            nodemap
                .effective_access_mode("ExposureTime", &io)
                .expect("access"),
            AccessMode::RW
        );

        nodemap
            .set_float("ExposureTime", 12_000.0, &io)
            .expect("write must reach the wire");
        assert_eq!(
            io.regs.borrow().get(&0x000C_1004).cloned(),
            Some(12_000u32.to_be_bytes().to_vec())
        );
    }

    /// The lock bit is bit 3 counted from the MSB — `0x1000_0000`, not `0x8`.
    ///
    /// This is the guard added for issue #45. It could never have fired on that
    /// reporter's camera, because it was reading bit 3 from the wrong end and
    /// always saw zero; #120 is what exposed that.
    #[test]
    fn big_endian_lock_bit_is_counted_from_the_msb() {
        let mut nodemap = NodeMap::try_from_xml(
            viva_genapi_xml::parse(BIG_ENDIAN_PREDICATE_FIXTURE).expect("parse"),
        )
        .expect("nodemap");
        let io = big_endian_predicate_io(0xD000_0000);

        let err = nodemap
            .set_float("ExposureTime", 12_000.0, &io)
            .expect_err("locked");
        assert!(
            matches!(err, GenApiError::Locked { ref locked_by, .. } if locked_by == "ExposureTime_Lck"),
            "expected Locked by ExposureTime_Lck, got {err:?}"
        );
    }

    /// Clearing the availability bit alone must still refuse — and must say
    /// "unavailable", not "locked". This is the message the #120 reporter saw,
    /// reproduced from the state that genuinely warrants it.
    #[test]
    fn big_endian_availability_bit_refuses_the_write() {
        let mut nodemap = NodeMap::try_from_xml(
            viva_genapi_xml::parse(BIG_ENDIAN_PREDICATE_FIXTURE).expect("parse"),
        )
        .expect("nodemap");
        let io = big_endian_predicate_io(0x8000_0000);

        let err = nodemap
            .set_float("ExposureTime", 12_000.0, &io)
            .expect_err("unavailable");
        assert!(
            matches!(err, GenApiError::Unavailable(ref n) if n == "ExposureTime"),
            "expected Unavailable, got {err:?}"
        );
    }

    fn build_predicate_nodemap() -> NodeMap {
        NodeMap::try_from_xml(
            viva_genapi_xml::parse(PREDICATE_FIXTURE).expect("parse predicate fixture"),
        )
        .expect("build nodemap")
    }

    fn predicate_io(ctrl: u32) -> MockIo {
        MockIo::with_registers(&[
            (0x400, ctrl.to_be_bytes().to_vec()),
            (0x410, 0u32.to_be_bytes().to_vec()),
            (0x420, 1u32.to_be_bytes().to_vec()),
        ])
    }

    #[test]
    fn predicate_is_implemented_defaults_true() {
        let nodemap = build_predicate_nodemap();
        let io = predicate_io(0);
        // CtrlReg itself has no pIsImplemented → always implemented.
        assert!(nodemap.is_implemented("CtrlReg", &io).unwrap());
    }

    #[test]
    fn predicate_is_implemented_follows_gate() {
        let nm0 = build_predicate_nodemap();
        let io0 = predicate_io(0);
        assert!(!nm0.is_implemented("Gated", &io0).unwrap());
        let nm1 = build_predicate_nodemap();
        let io1 = predicate_io(1);
        assert!(nm1.is_implemented("Gated", &io1).unwrap());
    }

    #[test]
    fn predicate_is_available_chains_implemented() {
        let nm0 = build_predicate_nodemap();
        let io0 = predicate_io(0);
        assert!(!nm0.is_available("Gated", &io0).unwrap());
        let nm1 = build_predicate_nodemap();
        let io1 = predicate_io(1);
        assert!(nm1.is_available("Gated", &io1).unwrap());
    }

    #[test]
    fn predicate_effective_access_mode_locked_downgrade() {
        let nodemap = build_predicate_nodemap();
        // bit 0 set (implemented), bit 1 set (locked) → RW → RO
        let io = predicate_io(0b11);
        let mode = nodemap.effective_access_mode("Gated", &io).unwrap();
        assert_eq!(mode, AccessMode::RO);
    }

    #[test]
    fn write_to_a_locked_node_is_refused_locally() {
        // The #45 shape: the node's static AccessMode is RW and the whole
        // restriction lives in pIsLocked. Before GA-06 this write went to the
        // wire and the device answered ACCESS_DENIED.
        let mut nodemap = build_predicate_nodemap();
        let io = predicate_io(0b11); // implemented, locked
        let err = nodemap
            .set_integer("Gated", 7, &io)
            .expect_err("a locked node must not be written");
        match err {
            GenApiError::Locked { name, locked_by } => {
                assert_eq!(name, "Gated");
                // Naming the locking feature is the actionable part.
                assert_eq!(locked_by, "GateLocked");
            }
            other => panic!("expected Locked, got {other:?}"),
        }
    }

    #[test]
    fn write_to_an_unlocked_node_still_succeeds() {
        let mut nodemap = build_predicate_nodemap();
        let io = predicate_io(0b01); // implemented, unlocked
        nodemap
            .set_integer("Gated", 7, &io)
            .expect("an unlocked RW node must still be writable");
    }

    #[test]
    fn write_to_an_unimplemented_node_reports_unavailable_not_locked() {
        // The two conditions must stay distinguishable: `effective_access_mode`
        // collapses both into RO, which is why the setters do not use it.
        let mut nodemap = build_predicate_nodemap();
        let io = predicate_io(0b00); // not implemented
        let err = nodemap
            .set_integer("Gated", 7, &io)
            .expect_err("an unavailable node must not be written");
        assert!(
            matches!(err, GenApiError::Unavailable(ref n) if n == "Gated"),
            "expected Unavailable, got {err:?}"
        );
    }

    #[test]
    fn predicate_effective_access_mode_rw_when_unlocked() {
        let nodemap = build_predicate_nodemap();
        // implemented, unlocked → base RW
        let io = predicate_io(0b01);
        let mode = nodemap.effective_access_mode("Gated", &io).unwrap();
        assert_eq!(mode, AccessMode::RW);
    }

    #[test]
    fn predicate_effective_access_mode_na_for_unavailable() {
        let nodemap = build_predicate_nodemap();
        // not implemented → effective access reported as RO (we don't model NA).
        let io = predicate_io(0);
        let mode = nodemap.effective_access_mode("Gated", &io).unwrap();
        assert_eq!(mode, AccessMode::RO);
    }

    #[test]
    fn predicate_available_enum_entries_filters() {
        let nodemap = build_predicate_nodemap();
        // bit 2 clear → Mono16 gated out; Mono8 has no predicate so it stays.
        let io = predicate_io(0);
        let entries = nodemap
            .available_enum_entries("PixelFormat", &io)
            .expect("enum entries");
        assert_eq!(entries, vec!["Mono8".to_string()]);
    }

    #[test]
    fn predicate_available_enum_entries_full_when_allowed() {
        let nodemap = build_predicate_nodemap();
        // bit 2 set → Mono16 available.
        let io = predicate_io(0b100);
        let mut entries = nodemap
            .available_enum_entries("PixelFormat", &io)
            .expect("enum entries");
        entries.sort();
        assert_eq!(entries, vec!["Mono16".to_string(), "Mono8".to_string()]);
    }

    #[test]
    fn predicate_available_enum_entries_fallback_to_static() {
        // CtrlReg itself isn't an enum; use an enum without entry predicates.
        let xml = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
                <Enumeration Name="Mode">
                    <EnumEntry Name="A"><Value>0</Value></EnumEntry>
                    <EnumEntry Name="B"><Value>1</Value></EnumEntry>
                    <pValue>ModeReg</pValue>
                </Enumeration>
                <IntReg Name="ModeReg">
                    <Address>0x500</Address>
                    <Length>4</Length>
                    <AccessMode>RW</AccessMode>
                    <Sign>Unsigned</Sign>
                    <Endianess>BigEndian</Endianess>
                </IntReg>
            </RegisterDescription>
        "#;
        let nodemap =
            NodeMap::try_from_xml(viva_genapi_xml::parse(xml).unwrap()).expect("build nodemap");
        let io = MockIo::with_registers(&[(0x500, 0u32.to_be_bytes().to_vec())]);
        let mut entries = nodemap.available_enum_entries("Mode", &io).unwrap();
        entries.sort();
        assert_eq!(entries, vec!["A".to_string(), "B".to_string()]);
    }

    #[test]
    fn float_conversion_roundtrip() {
        let mut nodemap = build_nodemap();
        let raw = 50_000i64; // 50 ms with 1/1000 scale
        let io = MockIo::with_registers(&[(
            0x200,
            i64_to_bytes("ExposureTime", raw, 4, Sign::Signed, ByteOrder::Big).unwrap(),
        )]);
        let exposure = nodemap
            .get_float("ExposureTime", &io)
            .expect("read exposure");
        assert!((exposure - 50.0).abs() < 1e-6);
        nodemap
            .set_float("ExposureTime", 75.0, &io)
            .expect("write exposure");
        let raw_back = bytes_to_i64(
            "ExposureTime",
            &io.read(0x200, 4).unwrap(),
            Sign::Signed,
            ByteOrder::Big,
        )
        .unwrap();
        assert_eq!(raw_back, 75_000);
    }

    #[test]
    fn selector_address_switching() {
        let mut nodemap = build_nodemap();
        let io = MockIo::with_registers(&[
            (
                0x300,
                i64_to_bytes("GainSelector", 0, 2, Sign::Signed, ByteOrder::Big).unwrap(),
            ),
            (
                0x310,
                i64_to_bytes("Gain", 10, 2, Sign::Signed, ByteOrder::Big).unwrap(),
            ),
            (
                0x314,
                i64_to_bytes("Gain", 24, 2, Sign::Signed, ByteOrder::Big).unwrap(),
            ),
        ]);

        let gain_all = nodemap.get_integer("Gain", &io).expect("gain for All");
        assert_eq!(gain_all, 10);
        assert_eq!(io.read_count(0x310), 1);
        assert_eq!(io.read_count(0x314), 0);

        io.write(
            0x314,
            &i64_to_bytes("Gain", 32, 2, Sign::Signed, ByteOrder::Big).unwrap(),
        )
        .expect("update red gain");
        nodemap
            .set_enum("GainSelector", "Red", &io)
            .expect("set selector to red");
        let gain_red = nodemap.get_integer("Gain", &io).expect("gain for Red");
        assert_eq!(gain_red, 32);
        assert_eq!(
            io.read_count(0x310),
            1,
            "previous address should not be reread"
        );
        assert_eq!(io.read_count(0x314), 1);

        let gain_red_cached = nodemap.get_integer("Gain", &io).expect("cached red");
        assert_eq!(gain_red_cached, 32);
        assert_eq!(io.read_count(0x314), 1, "selector cache should be reused");

        nodemap
            .set_enum("GainSelector", "Blue", &io)
            .expect("set selector to blue");
        let err = nodemap.get_integer("Gain", &io).unwrap_err();
        match err {
            GenApiError::Unavailable(msg) => {
                assert!(msg.contains("GainSelector=Blue"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
        assert_eq!(
            io.read_count(0x314),
            1,
            "no read expected for missing mapping"
        );

        io.write(
            0x310,
            &i64_to_bytes("Gain", 12, 2, Sign::Signed, ByteOrder::Big).unwrap(),
        )
        .expect("update all gain");
        nodemap
            .set_enum("GainSelector", "All", &io)
            .expect("restore selector to all");
        let gain_all_updated = nodemap
            .get_integer("Gain", &io)
            .expect("gain for All again");
        assert_eq!(gain_all_updated, 12);
        assert_eq!(
            io.read_count(0x310),
            2,
            "address switch should invalidate cache"
        );
    }

    #[test]
    fn range_enforcement() {
        let mut nodemap = build_nodemap();
        let io = MockIo::with_registers(&[(0x100, vec![0, 0, 0, 16])]);
        let err = nodemap.set_integer("Width", 17, &io).unwrap_err();
        assert!(matches!(err, GenApiError::Range(_)));
    }

    #[test]
    fn command_exec() {
        let mut nodemap = build_nodemap();
        let io = MockIo::with_registers(&[]);
        nodemap
            .exec_command("AcquisitionStart", &io)
            .expect("exec command");
        let payload = io.read(0x500, 4).expect("command write");
        assert_eq!(payload, vec![0, 0, 0, 1]);
    }

    #[test]
    fn indirect_address_resolution() {
        let mut nodemap = build_indirect_nodemap();
        let io = MockIo::with_registers(&[
            (
                0x2000,
                i64_to_bytes("RegAddr", 0x3000, 4, Sign::Signed, ByteOrder::Big).unwrap(),
            ),
            (
                0x3000,
                i64_to_bytes("Gain", 123, 4, Sign::Signed, ByteOrder::Big).unwrap(),
            ),
            (
                0x3100,
                i64_to_bytes("Gain", 77, 4, Sign::Signed, ByteOrder::Big).unwrap(),
            ),
        ]);

        let initial = nodemap.get_integer("Gain", &io).expect("read gain");
        assert_eq!(initial, 123);
        assert_eq!(io.read_count(0x2000), 1);
        assert_eq!(io.read_count(0x3000), 1);

        nodemap
            .set_integer("RegAddr", 0x3100, &io)
            .expect("set indirect address");
        let updated = nodemap
            .get_integer("Gain", &io)
            .expect("read gain after change");
        assert_eq!(updated, 77);
        assert_eq!(io.read_count(0x2000), 1);
        assert_eq!(io.read_count(0x3000), 1);
        assert_eq!(io.read_count(0x3100), 1);
    }

    /// A `<pAddress>` term that cannot be an address at all is rejected before
    /// the read.
    ///
    /// Zero deliberately is *not* rejected: under the additive address model a
    /// `<pAddress>` supplies one term of a sum, and a base of zero next to a
    /// fixed `<Address>` offset is ordinary. Only a value that cannot be a
    /// register address — a negative one — is a modelling error rather than a
    /// device state.
    /// A `<pAddress>` term that cannot be an address at all is rejected before
    /// the read.
    ///
    /// Zero deliberately is *not* rejected: under the additive address model a
    /// `<pAddress>` supplies one term of a sum, and a base of zero next to a
    /// fixed `<Address>` offset is ordinary. Only a value that cannot be a
    /// register address — a negative one, which takes a signed provider — is a
    /// modelling error rather than a device state.
    #[test]
    fn indirect_bad_address() {
        const XML: &str = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
                <Integer Name="RegAddr">
                    <Address>0x2000</Address>
                    <Length>4</Length>
                    <AccessMode>RW</AccessMode>
                    <Sign>Signed</Sign>
                    <Min>-65535</Min>
                    <Max>65535</Max>
                </Integer>
                <Integer Name="Gain">
                    <pAddress>RegAddr</pAddress>
                    <Length>4</Length>
                    <AccessMode>RW</AccessMode>
                    <Min>0</Min>
                    <Max>255</Max>
                </Integer>
            </RegisterDescription>
        "#;

        let model = viva_genapi_xml::parse(XML).expect("parse");
        let nodemap = NodeMap::try_from_xml(model).expect("build nodemap");
        let io = MockIo::with_registers(&[(
            0x2000,
            i64_to_bytes("RegAddr", -4, 4, Sign::Signed, ByteOrder::Big).unwrap(),
        )]);

        let err = nodemap.get_integer("Gain", &io).unwrap_err();
        match err {
            GenApiError::BadIndirectAddress { name, addr } => {
                assert_eq!(name, "Gain");
                assert_eq!(addr, -4);
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    /// GenICam registers are unsigned unless `<Sign>Signed</Sign>` says
    /// otherwise. Sign-extending everything turned an IPv4 address into a
    /// negative number and broke every mask comparison downstream.
    #[test]
    fn unsigned_registers_do_not_sign_extend() {
        const XML: &str = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
                <Integer Name="GevCurrentIPAddress">
                    <Address>0x1000</Address>
                    <Length>4</Length>
                    <AccessMode>RO</AccessMode>
                    <Min>0</Min>
                    <Max>4294967295</Max>
                </Integer>
                <Integer Name="TemperatureOffset">
                    <Address>0x1004</Address>
                    <Length>4</Length>
                    <AccessMode>RO</AccessMode>
                    <Sign>Signed</Sign>
                    <Min>-2147483648</Min>
                    <Max>2147483647</Max>
                </Integer>
            </RegisterDescription>
        "#;

        let model = viva_genapi_xml::parse(XML).expect("parse");
        let nodemap = NodeMap::try_from_xml(model).expect("build nodemap");
        let io = MockIo::with_registers(&[
            // 192.168.1.160 — the top bit is set.
            (0x1000, vec![0xC0, 0xA8, 0x01, 0xA0]),
            (0x1004, vec![0xFF, 0xFF, 0xFF, 0xFB]),
        ]);

        assert_eq!(
            nodemap
                .get_integer("GevCurrentIPAddress", &io)
                .expect("read address"),
            0xC0A8_01A0
        );
        assert_eq!(
            nodemap
                .get_integer("TemperatureOffset", &io)
                .expect("read offset"),
            -5
        );
    }

    /// `<FormulaFrom>` is the read direction and `<FormulaTo>` the write one.
    ///
    /// With an identity converter both directions look the same, which is how
    /// this stayed inverted: reads evaluated `<FormulaTo>` and every non-trivial
    /// converter — a Hikrobot `FROM * 100` / `TO / 100` pair, for instance —
    /// came back wrong by the square of its scale factor.
    #[test]
    fn converter_directions_are_not_swapped() {
        const XML: &str = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
                <IntReg Name="ExposureTimeRaw">
                    <Address>0x1000</Address>
                    <Length>4</Length>
                    <AccessMode>RW</AccessMode>
                </IntReg>
                <Converter Name="ExposureTime">
                    <FormulaTo>FROM * 100</FormulaTo>
                    <FormulaFrom>TO / 100</FormulaFrom>
                    <pValue>ExposureTimeRaw</pValue>
                    <Unit>us</Unit>
                </Converter>
            </RegisterDescription>
        "#;

        let model = viva_genapi_xml::parse(XML).expect("parse");
        let mut nodemap = NodeMap::try_from_xml(model).expect("build nodemap");
        let io = MockIo::with_registers(&[(
            0x1000,
            i64_to_bytes("ExposureTimeRaw", 5000, 4, Sign::Unsigned, ByteOrder::Big).unwrap(),
        )]);

        // Read goes through FormulaFrom: 5000 / 100.
        let value = nodemap.get_float("ExposureTime", &io).expect("read");
        assert!((value - 50.0).abs() < 1e-9, "got {value}");

        // Write goes through FormulaTo: 75 * 100.
        nodemap.set_float("ExposureTime", 75.0, &io).expect("write");
        assert_eq!(
            nodemap
                .get_integer("ExposureTimeRaw", &io)
                .expect("read raw back"),
            7500
        );
        let roundtrip = nodemap.get_float("ExposureTime", &io).expect("re-read");
        assert!((roundtrip - 75.0).abs() < 1e-9, "got {roundtrip}");
    }

    /// A read-modify-write `<FormulaTo>` sees the register's current contents
    /// through `OLD`.
    #[test]
    fn int_converter_write_preserves_other_bits() {
        const XML: &str = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
                <IntReg Name="Binning_Reg">
                    <Address>0x1000</Address>
                    <Length>4</Length>
                    <AccessMode>RW</AccessMode>
                </IntReg>
                <IntConverter Name="BinningHorizontal">
                    <FormulaTo>FROM | (OLD &amp; 0xffff0000)</FormulaTo>
                    <FormulaFrom>TO &amp; 0x0000ffff</FormulaFrom>
                    <pVariable Name="OLD">Binning_Reg</pVariable>
                    <pValue>Binning_Reg</pValue>
                </IntConverter>
            </RegisterDescription>
        "#;

        let model = viva_genapi_xml::parse(XML).expect("parse");
        let mut nodemap = NodeMap::try_from_xml(model).expect("build nodemap");
        let io = MockIo::with_registers(&[(
            0x1000,
            i64_to_bytes(
                "Binning_Reg",
                0x0004_0002,
                4,
                Sign::Unsigned,
                ByteOrder::Big,
            )
            .unwrap(),
        )]);

        assert_eq!(
            nodemap
                .get_integer("BinningHorizontal", &io)
                .expect("read low half"),
            2
        );

        nodemap
            .set_integer("BinningHorizontal", 3, &io)
            .expect("write low half");
        // The vertical half in the high word must survive.
        assert_eq!(
            nodemap
                .get_integer("Binning_Reg", &io)
                .expect("read raw back"),
            0x0004_0003
        );
    }

    /// Signedness comes from `<Sign>` alone — `<Min>` says nothing about it.
    ///
    /// `<Min>` is optional and defaults to `i64::MIN`, and **no** `<IntReg>` or
    /// `<MaskedIntReg>` in the entire vendor corpus declares one: all 9 779 of
    /// them omit it. Inferring "signed" from a negative minimum therefore fires
    /// on every register-backed integer on every real camera — which is exactly
    /// the case `<Sign>` exists to decide. Note the registers here carry no
    /// `<Min>`, which is the shape real documents actually use.
    #[test]
    fn sign_does_not_depend_on_min() {
        const XML: &str = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
                <IntReg Name="GevCurrentIPAddress">
                    <Address>0x1000</Address>
                    <Length>4</Length>
                    <AccessMode>RO</AccessMode>
                    <Sign>Unsigned</Sign>
                </IntReg>
                <IntReg Name="DeviceTemperatureRaw">
                    <Address>0x1004</Address>
                    <Length>4</Length>
                    <AccessMode>RO</AccessMode>
                    <Sign>Signed</Sign>
                </IntReg>
                <IntReg Name="ImplicitlyUnsigned">
                    <Address>0x1008</Address>
                    <Length>4</Length>
                    <AccessMode>RO</AccessMode>
                </IntReg>
            </RegisterDescription>
        "#;

        let model = viva_genapi_xml::parse(XML).expect("parse");
        let nodemap = NodeMap::try_from_xml(model).expect("build nodemap");
        let io = MockIo::with_registers(&[
            (0x1000, vec![0xC0, 0xA8, 0x01, 0xA0]),
            (0x1004, vec![0xFF, 0xFF, 0xFF, 0xFB]),
            (0x1008, vec![0xFF, 0xFF, 0xFF, 0xFF]),
        ]);

        assert_eq!(
            nodemap
                .get_integer("GevCurrentIPAddress", &io)
                .expect("read address"),
            0xC0A8_01A0
        );
        assert_eq!(
            nodemap
                .get_integer("DeviceTemperatureRaw", &io)
                .expect("read temperature"),
            -5
        );
        // No `<Sign>` at all: GenICam's default is unsigned.
        assert_eq!(
            nodemap
                .get_integer("ImplicitlyUnsigned", &io)
                .expect("read default-signed register"),
            0xFFFF_FFFFu32 as i64
        );
    }

    /// A `<StructReg>` bit reads as 1, not -1.
    ///
    /// Entries used to declare the full `i64` range, which looked like a
    /// signed field; a set bit then sign-extended to -1 and every
    /// `(INQ = 1)` test in the document silently failed.
    #[test]
    fn struct_entry_bits_read_unsigned() {
        const XML: &str = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
                <StructReg Comment="Gain Inquiry Register">
                    <Address>0x520</Address>
                    <Length>4</Length>
                    <AccessMode>RO</AccessMode>
                    <Endianess>BigEndian</Endianess>
                    <StructEntry Name="GainPresInq_Bit"><Bit>0</Bit></StructEntry>
                    <StructEntry Name="GainAutoInq_Bit"><Bit>6</Bit></StructEntry>
                </StructReg>
            </RegisterDescription>
        "#;

        let model = viva_genapi_xml::parse(XML).expect("parse");
        let nodemap = NodeMap::try_from_xml(model).expect("build nodemap");
        // `<Bit>` indices count from the MSB on a big-endian register (the
        // same rule the reference implementation applies), so bits 0 and 6 are
        // the top byte's 0x80 and 0x02.
        let io = MockIo::with_registers(&[(0x520, vec![0x82, 0x00, 0x00, 0x00])]);

        assert_eq!(
            nodemap
                .get_integer("GainPresInq_Bit", &io)
                .expect("read presence bit"),
            1
        );
        assert_eq!(
            nodemap
                .get_integer("GainAutoInq_Bit", &io)
                .expect("read auto bit"),
            1
        );
    }

    /// The whole point of the additive model: `<Address>` and `<pAddress>` on
    /// the same register add up. Keeping only one of them silently read the
    /// wrong register on FLIR, Point Grey and Hikrobot cameras (issue #35).
    #[test]
    fn address_terms_sum() {
        const XML: &str = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
                <Integer Name="RegBase">
                    <Address>0x2000</Address>
                    <Length>4</Length>
                    <AccessMode>RW</AccessMode>
                    <Min>0</Min>
                    <Max>65535</Max>
                </Integer>
                <Integer Name="Gain">
                    <pAddress>RegBase</pAddress>
                    <Address>0x8</Address>
                    <Length>4</Length>
                    <AccessMode>RW</AccessMode>
                    <Min>0</Min>
                    <Max>255</Max>
                </Integer>
            </RegisterDescription>
        "#;

        let model = viva_genapi_xml::parse(XML).expect("parse");
        let nodemap = NodeMap::try_from_xml(model).expect("build nodemap");
        let io = MockIo::with_registers(&[
            (
                0x2000,
                i64_to_bytes("RegBase", 0x3000, 4, Sign::Signed, ByteOrder::Big).unwrap(),
            ),
            // Only the summed address holds the value we expect.
            (
                0x3008,
                i64_to_bytes("Gain", 77, 4, Sign::Signed, ByteOrder::Big).unwrap(),
            ),
            (
                0x3000,
                i64_to_bytes("Gain", 11, 4, Sign::Signed, ByteOrder::Big).unwrap(),
            ),
        ]);

        assert_eq!(nodemap.get_integer("Gain", &io).expect("read gain"), 77);

        // The same addressing, reached by name for raw register access.
        assert_eq!(
            nodemap
                .register_address("Gain", &io)
                .expect("resolve summed address"),
            (0x3008, 4)
        );
        // An unknown name is a named error, not a panic.
        assert!(matches!(
            nodemap.register_address("NoSuchNode", &io),
            Err(GenApiError::NodeNotFound(_))
        ));
    }

    /// `<pIndex Offset="N">` scales an index node into the address. AVT Manta
    /// and Prosilica use it for every per-trigger inquiry register.
    #[test]
    fn p_index_scales_into_the_address() {
        const XML: &str = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
                <Integer Name="TriggerSelectorIdx">
                    <Address>0x2000</Address>
                    <Length>4</Length>
                    <AccessMode>RW</AccessMode>
                    <Min>0</Min>
                    <Max>7</Max>
                </Integer>
                <Integer Name="TriggerInqDelay">
                    <Address>0x13400</Address>
                    <pIndex Offset="64">TriggerSelectorIdx</pIndex>
                    <Length>4</Length>
                    <AccessMode>RO</AccessMode>
                    <Min>0</Min>
                    <Max>255</Max>
                </Integer>
            </RegisterDescription>
        "#;

        let model = viva_genapi_xml::parse(XML).expect("parse");
        let mut nodemap = NodeMap::try_from_xml(model).expect("build nodemap");
        let io = MockIo::with_registers(&[
            (
                0x2000,
                i64_to_bytes("TriggerSelectorIdx", 2, 4, Sign::Signed, ByteOrder::Big).unwrap(),
            ),
            (
                0x13400,
                i64_to_bytes("TriggerInqDelay", 1, 4, Sign::Signed, ByteOrder::Big).unwrap(),
            ),
            (
                0x13480,
                i64_to_bytes("TriggerInqDelay", 42, 4, Sign::Signed, ByteOrder::Big).unwrap(),
            ),
        ]);

        // index 2 * stride 64 = 0x80 past the base.
        assert_eq!(
            nodemap
                .get_integer("TriggerInqDelay", &io)
                .expect("read indexed register"),
            42
        );

        // Changing the index moves the register.
        nodemap
            .set_integer("TriggerSelectorIdx", 0, &io)
            .expect("select index 0");
        assert_eq!(
            nodemap
                .get_integer("TriggerInqDelay", &io)
                .expect("read indexed register"),
            1
        );
    }

    #[test]
    fn enum_literal_entry_read() {
        let nodemap = build_enum_pvalue_nodemap();
        let io = MockIo::with_registers(&[
            (
                0x4000,
                i64_to_bytes("Mode", 10, 4, Sign::Signed, ByteOrder::Big).unwrap(),
            ),
            (
                0x4100,
                i64_to_bytes("RegModeVal", 42, 4, Sign::Signed, ByteOrder::Big).unwrap(),
            ),
        ]);

        let value = nodemap.get_enum("Mode", &io).expect("read mode");
        assert_eq!(value, "Fixed10");
        assert_eq!(
            io.read_count(0x4100),
            1,
            "provider should be read once for mapping"
        );
    }

    #[test]
    fn enum_provider_entry_read() {
        let nodemap = build_enum_pvalue_nodemap();
        let io = MockIo::with_registers(&[
            (
                0x4000,
                i64_to_bytes("Mode", 42, 4, Sign::Signed, ByteOrder::Big).unwrap(),
            ),
            (
                0x4100,
                i64_to_bytes("RegModeVal", 42, 4, Sign::Signed, ByteOrder::Big).unwrap(),
            ),
        ]);

        let value = nodemap.get_enum("Mode", &io).expect("read dynamic mode");
        assert_eq!(value, "DynFromReg");
        assert_eq!(io.read_count(0x4100), 1);
    }

    #[test]
    fn enum_set_uses_provider_value() {
        let mut nodemap = build_enum_pvalue_nodemap();
        let io = MockIo::with_registers(&[
            (
                0x4000,
                i64_to_bytes("Mode", 0, 4, Sign::Signed, ByteOrder::Big).unwrap(),
            ),
            (
                0x4100,
                i64_to_bytes("RegModeVal", 42, 4, Sign::Signed, ByteOrder::Big).unwrap(),
            ),
        ]);

        nodemap
            .set_enum("Mode", "DynFromReg", &io)
            .expect("write enum");
        let raw = bytes_to_i64(
            "Mode",
            &io.read(0x4000, 4).unwrap(),
            Sign::Signed,
            ByteOrder::Big,
        )
        .unwrap();
        assert_eq!(raw, 42);
        assert_eq!(io.read_count(0x4100), 1);
    }

    #[test]
    fn enum_provider_update_invalidates_mapping() {
        let mut nodemap = build_enum_pvalue_nodemap();
        let io = MockIo::with_registers(&[
            (
                0x4000,
                i64_to_bytes("Mode", 42, 4, Sign::Signed, ByteOrder::Big).unwrap(),
            ),
            (
                0x4100,
                i64_to_bytes("RegModeVal", 42, 4, Sign::Signed, ByteOrder::Big).unwrap(),
            ),
        ]);

        assert_eq!(nodemap.get_enum("Mode", &io).unwrap(), "DynFromReg");
        assert_eq!(io.read_count(0x4100), 1);

        nodemap
            .set_integer("RegModeVal", 17, &io)
            .expect("update provider");
        io.write(
            0x4000,
            &i64_to_bytes("Mode", 0, 4, Sign::Signed, ByteOrder::Big).unwrap(),
        )
        .expect("reset mode register");

        nodemap
            .set_enum("Mode", "DynFromReg", &io)
            .expect("write enum after provider change");
        let raw = bytes_to_i64(
            "Mode",
            &io.read(0x4000, 4).unwrap(),
            Sign::Signed,
            ByteOrder::Big,
        )
        .unwrap();
        assert_eq!(raw, 17);
    }

    #[test]
    fn enum_unknown_value_error() {
        let nodemap = build_enum_pvalue_nodemap();
        let io = MockIo::with_registers(&[
            (
                0x4000,
                i64_to_bytes("Mode", 99, 4, Sign::Signed, ByteOrder::Big).unwrap(),
            ),
            (
                0x4100,
                i64_to_bytes("RegModeVal", 42, 4, Sign::Signed, ByteOrder::Big).unwrap(),
            ),
        ]);

        let err = nodemap.get_enum("Mode", &io).unwrap_err();
        match err {
            GenApiError::EnumValueUnknown { node, value } => {
                assert_eq!(node, "Mode");
                assert_eq!(value, 99);
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn enum_entries_are_sorted() {
        let nodemap = build_enum_pvalue_nodemap();
        let entries = nodemap.enum_entries("Mode").expect("entries");
        assert_eq!(
            entries,
            vec!["DynFromReg".to_string(), "Fixed10".to_string()]
        );
    }

    #[test]
    fn bitfield_le_integer_roundtrip() {
        let mut nodemap = build_bitfield_nodemap();
        let io = MockIo::with_registers(&[(0x5000, vec![0xAA, 0xBB, 0xCC, 0xDD])]);

        let value = nodemap
            .get_integer("LeByte", &io)
            .expect("read little-endian field");
        assert_eq!(value, 0xBB);

        nodemap
            .set_integer("LeByte", 0x55, &io)
            .expect("write little-endian field");
        let data = io.read(0x5000, 4).expect("read back register");
        assert_eq!(data, vec![0xAA, 0x55, 0xCC, 0xDD]);
    }

    #[test]
    fn bitfield_be_integer_roundtrip() {
        let mut nodemap = build_bitfield_nodemap();
        let io = MockIo::with_registers(&[(0x5004, vec![0b1010_0000, 0b0000_0000])]);

        let value = nodemap
            .get_integer("BeBits", &io)
            .expect("read big-endian bits");
        assert_eq!(value, 0b101);

        nodemap
            .set_integer("BeBits", 0b010, &io)
            .expect("write big-endian bits");
        let data = io.read(0x5004, 2).expect("read back register");
        assert_eq!(data, vec![0b0100_0000, 0b0000_0000]);
    }

    #[test]
    fn bitfield_boolean_toggle() {
        let mut nodemap = build_bitfield_nodemap();
        let io = MockIo::with_registers(&[(0x5006, vec![0x00, 0x20, 0x00, 0x00])]);

        assert!(nodemap.get_bool("PackedFlag", &io).expect("read flag"));

        nodemap
            .set_bool("PackedFlag", false, &io)
            .expect("clear flag");
        let data = io.read(0x5006, 4).expect("read cleared");
        assert_eq!(data, vec![0x00, 0x00, 0x00, 0x00]);

        nodemap.set_bool("PackedFlag", true, &io).expect("set flag");
        let data = io.read(0x5006, 4).expect("read set");
        assert_eq!(data, vec![0x00, 0x20, 0x00, 0x00]);
    }

    #[test]
    fn bitfield_value_too_wide() {
        let mut nodemap = build_bitfield_nodemap();
        let io = MockIo::with_registers(&[(0x5004, vec![0x00, 0x00])]);

        let err = nodemap
            .set_integer("BeBits", 8, &io)
            .expect_err("value too wide");
        match err {
            GenApiError::ValueTooWide {
                name, bit_length, ..
            } => {
                assert_eq!(name, "BeBits");
                assert_eq!(bit_length, 3);
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }
    #[test]
    fn swissknife_evaluates_and_invalidates() {
        let mut nodemap = build_swissknife_nodemap();
        let io = MockIo::with_registers(&[
            (
                0x3000,
                i64_to_bytes("GainRaw", 100, 4, Sign::Signed, ByteOrder::Big).unwrap(),
            ),
            (
                0x3008,
                i64_to_bytes("Offset", 3, 4, Sign::Signed, ByteOrder::Big).unwrap(),
            ),
            (
                0x3010,
                i64_to_bytes("B", 1, 4, Sign::Signed, ByteOrder::Big).unwrap(),
            ),
        ]);

        let value = nodemap
            .get_float("ComputedGain", &io)
            .expect("compute gain");
        assert!((value - 53.0).abs() < 1e-6);

        nodemap
            .set_integer("GainRaw", 120, &io)
            .expect("update raw gain");
        let updated = nodemap
            .get_float("ComputedGain", &io)
            .expect("recompute gain");
        assert!((updated - 63.0).abs() < 1e-6);
    }

    #[test]
    fn swissknife_integer_rounding_and_unary() {
        let mut nodemap = build_swissknife_nodemap();
        let io = MockIo::with_registers(&[
            (
                0x3000,
                i64_to_bytes("GainRaw", 5, 4, Sign::Signed, ByteOrder::Big).unwrap(),
            ),
            (
                0x3008,
                i64_to_bytes("Offset", 0, 4, Sign::Signed, ByteOrder::Big).unwrap(),
            ),
            (
                0x3010,
                i64_to_bytes("B", 1, 4, Sign::Signed, ByteOrder::Big).unwrap(),
            ),
        ]);

        // `<IntSwissKnife>` evaluates in integer arithmetic, so 5 / 3 truncates
        // to 1. Rounding to 2 would mean the division ran in floating point.
        let divided = nodemap
            .get_integer("DivideInt", &io)
            .expect("integer division");
        assert_eq!(divided, 1);

        nodemap
            .set_integer("GainRaw", 3, &io)
            .expect("update gain raw");
        let unary = nodemap.get_integer("Unary", &io).expect("unary expression");
        assert_eq!(unary, 7);
    }

    #[test]
    fn swissknife_unknown_variable_error() {
        const XML: &str = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
                <Integer Name="A">
                    <Address>0x2000</Address>
                    <Length>4</Length>
                    <AccessMode>RW</AccessMode>
                    <Min>0</Min>
                    <Max>100</Max>
                </Integer>
                <SwissKnife Name="Bad">
                    <Expression>A + Missing</Expression>
                    <pVariable Name="A">A</pVariable>
                </SwissKnife>
            </RegisterDescription>
        "#;

        let model = viva_genapi_xml::parse(XML).expect("parse invalid swissknife");
        let nodemap = NodeMap::try_from_xml(model).expect("model builds despite the bad node");

        // The unusable node is dropped and recorded, not fatal: issues #35 and
        // #45 were both a single odd node making an entire camera unopenable.
        let skipped = nodemap.skipped();
        assert_eq!(skipped.len(), 1, "expected exactly one dropped node");
        assert_eq!(skipped[0].name.as_deref(), Some("Bad"));
        assert!(
            skipped[0].error.contains("Missing"),
            "the record should name the unresolved variable: {}",
            skipped[0].error
        );
        assert!(nodemap.node("Bad").is_none());

        // Everything else still works.
        assert!(nodemap.node("A").is_some());
    }

    /// A formula our parser cannot handle costs that feature, not the camera.
    #[test]
    fn unparsable_formula_is_isolated() {
        const XML: &str = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
                <Integer Name="Width">
                    <Address>0x2000</Address>
                    <Length>4</Length>
                    <AccessMode>RW</AccessMode>
                    <Min>0</Min>
                    <Max>4096</Max>
                </Integer>
                <IntSwissKnife Name="Broken">
                    <Formula>Width +</Formula>
                    <pVariable Name="Width">Width</pVariable>
                </IntSwissKnife>
            </RegisterDescription>
        "#;

        let model = viva_genapi_xml::parse(XML).expect("parse model");
        let nodemap = NodeMap::try_from_xml(model).expect("model builds despite the bad formula");
        assert_eq!(nodemap.skipped().len(), 1);
        assert_eq!(nodemap.skipped()[0].name.as_deref(), Some("Broken"));
        assert!(nodemap.node("Width").is_some());
    }

    #[test]
    fn swissknife_division_by_zero() {
        let nodemap = build_swissknife_nodemap();
        let io = MockIo::with_registers(&[
            (
                0x3000,
                i64_to_bytes("GainRaw", 10, 4, Sign::Signed, ByteOrder::Big).unwrap(),
            ),
            (
                0x3008,
                i64_to_bytes("Offset", 0, 4, Sign::Signed, ByteOrder::Big).unwrap(),
            ),
            (
                0x3010,
                i64_to_bytes("B", 0, 4, Sign::Signed, ByteOrder::Big).unwrap(),
            ),
        ]);

        let err = nodemap
            .get_float("DivideByZero", &io)
            .expect_err("division by zero");
        match err {
            GenApiError::ExprEval { name, msg } => {
                assert_eq!(name, "DivideByZero");
                assert_eq!(msg, "division by zero");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // nodes_at_visibility
    // -----------------------------------------------------------------------

    const VISIBILITY_FIXTURE: &str = r#"
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
            <Integer Name="BeginnerNode">
                <Address>0x6000</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Visibility>Beginner</Visibility>
                <Min>0</Min>
                <Max>100</Max>
            </Integer>
            <Integer Name="ExpertNode">
                <Address>0x6010</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Visibility>Expert</Visibility>
                <Min>0</Min>
                <Max>100</Max>
            </Integer>
            <Integer Name="GuruNode">
                <Address>0x6020</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Visibility>Guru</Visibility>
                <Min>0</Min>
                <Max>100</Max>
            </Integer>
            <Integer Name="InvisibleNode">
                <Address>0x6030</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Visibility>Invisible</Visibility>
                <Min>0</Min>
                <Max>100</Max>
            </Integer>
        </RegisterDescription>
    "#;

    #[test]
    fn nodes_at_visibility_beginner_returns_only_beginner() {
        let model = viva_genapi_xml::parse(VISIBILITY_FIXTURE).expect("parse visibility fixture");
        let nodemap = NodeMap::try_from_xml(model).expect("build nodemap");

        let visible = nodemap.nodes_at_visibility(Visibility::Beginner);
        assert!(
            visible.contains(&"BeginnerNode"),
            "Beginner node must be visible at Beginner level"
        );
        assert!(
            !visible.contains(&"ExpertNode"),
            "Expert node must NOT be visible at Beginner level"
        );
        assert!(
            !visible.contains(&"GuruNode"),
            "Guru node must NOT be visible at Beginner level"
        );
        assert!(
            !visible.contains(&"InvisibleNode"),
            "Invisible node must NOT be visible at Beginner level"
        );
    }

    #[test]
    fn nodes_at_visibility_guru_includes_beginner_and_expert_but_not_invisible() {
        let model = viva_genapi_xml::parse(VISIBILITY_FIXTURE).expect("parse visibility fixture");
        let nodemap = NodeMap::try_from_xml(model).expect("build nodemap");

        let visible = nodemap.nodes_at_visibility(Visibility::Guru);
        assert!(
            visible.contains(&"BeginnerNode"),
            "Beginner node must be visible at Guru level"
        );
        assert!(
            visible.contains(&"ExpertNode"),
            "Expert node must be visible at Guru level"
        );
        assert!(
            visible.contains(&"GuruNode"),
            "Guru node must be visible at Guru level"
        );
        assert!(
            !visible.contains(&"InvisibleNode"),
            "Invisible node must NOT be visible at Guru level"
        );
    }
}

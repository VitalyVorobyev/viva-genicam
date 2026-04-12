#![cfg_attr(docsrs, feature(doc_cfg))]
//! GenApi node system: typed feature access backed by register IO.

mod bitops;
mod conversions;
mod error;
mod io;
mod nodemap;
mod nodes;
mod swissknife;

pub use error::GenApiError;
pub use io::{NullIo, RegisterIo};
pub use nodemap::NodeMap;
pub use nodes::{
    BooleanNode, CategoryNode, CommandNode, EnumNode, FloatNode, IntegerNode, Node, NodeMeta,
    Representation, SkNode, Visibility,
};
pub use viva_genapi_xml::{AccessMode, SkOutput};

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use crate::conversions::{bytes_to_i64, i64_to_bytes};
    use crate::{AccessMode, GenApiError, NodeMap, RegisterIo, Visibility};

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
            <Integer Name="BeBits">
                <Address>0x5004</Address>
                <Length>2</Length>
                <AccessMode>RW</AccessMode>
                <Min>0</Min>
                <Max>15</Max>
                <Lsb>13</Lsb>
                <Msb>15</Msb>
                <Endianness>BigEndian</Endianness>
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
                <Expression>(GainRaw * 0.5) + Offset</Expression>
                <pVariable Name="GainRaw">GainRaw</pVariable>
                <pVariable Name="Offset">Offset</pVariable>
                <Output>Float</Output>
            </SwissKnife>
            <SwissKnife Name="DivideInt">
                <Expression>GainRaw / 3</Expression>
                <pVariable Name="GainRaw">GainRaw</pVariable>
                <Output>Integer</Output>
            </SwissKnife>
            <SwissKnife Name="Unary">
                <Expression>-GainRaw + 10</Expression>
                <pVariable Name="GainRaw">GainRaw</pVariable>
                <Output>Integer</Output>
            </SwissKnife>
            <SwissKnife Name="DivideByZero">
                <Expression>GainRaw / B</Expression>
                <pVariable Name="GainRaw">GainRaw</pVariable>
                <pVariable Name="B">B</pVariable>
                <Output>Float</Output>
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
        NodeMap::from(model)
    }

    fn build_indirect_nodemap() -> NodeMap {
        let model = viva_genapi_xml::parse(INDIRECT_FIXTURE).expect("parse indirect fixture");
        NodeMap::from(model)
    }

    fn build_enum_pvalue_nodemap() -> NodeMap {
        let model = viva_genapi_xml::parse(ENUM_PVALUE_FIXTURE).expect("parse enum pvalue fixture");
        NodeMap::from(model)
    }

    fn build_bitfield_nodemap() -> NodeMap {
        let model = viva_genapi_xml::parse(BITFIELD_FIXTURE).expect("parse bitfield fixture");
        NodeMap::from(model)
    }

    fn build_swissknife_nodemap() -> NodeMap {
        let model = viva_genapi_xml::parse(SWISSKNIFE_FIXTURE).expect("parse swissknife fixture");
        NodeMap::from(model)
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
        NodeMap::from(viva_genapi_xml::parse(IEEE754_FIXTURE).expect("parse ieee754"))
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
        let io = MockIo::with_registers(&[(0x200, i64_to_bytes("ExposureTime", raw, 4).unwrap())]);
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

    fn build_predicate_nodemap() -> NodeMap {
        NodeMap::from(viva_genapi_xml::parse(PREDICATE_FIXTURE).expect("parse predicate fixture"))
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
        let nodemap = NodeMap::from(viva_genapi_xml::parse(xml).unwrap());
        let io = MockIo::with_registers(&[(0x500, 0u32.to_be_bytes().to_vec())]);
        let mut entries = nodemap.available_enum_entries("Mode", &io).unwrap();
        entries.sort();
        assert_eq!(entries, vec!["A".to_string(), "B".to_string()]);
    }

    #[test]
    fn float_conversion_roundtrip() {
        let mut nodemap = build_nodemap();
        let raw = 50_000i64; // 50 ms with 1/1000 scale
        let io = MockIo::with_registers(&[(0x200, i64_to_bytes("ExposureTime", raw, 4).unwrap())]);
        let exposure = nodemap
            .get_float("ExposureTime", &io)
            .expect("read exposure");
        assert!((exposure - 50.0).abs() < 1e-6);
        nodemap
            .set_float("ExposureTime", 75.0, &io)
            .expect("write exposure");
        let raw_back = bytes_to_i64("ExposureTime", &io.read(0x200, 4).unwrap()).unwrap();
        assert_eq!(raw_back, 75_000);
    }

    #[test]
    fn selector_address_switching() {
        let mut nodemap = build_nodemap();
        let io = MockIo::with_registers(&[
            (0x300, i64_to_bytes("GainSelector", 0, 2).unwrap()),
            (0x310, i64_to_bytes("Gain", 10, 2).unwrap()),
            (0x314, i64_to_bytes("Gain", 24, 2).unwrap()),
        ]);

        let gain_all = nodemap.get_integer("Gain", &io).expect("gain for All");
        assert_eq!(gain_all, 10);
        assert_eq!(io.read_count(0x310), 1);
        assert_eq!(io.read_count(0x314), 0);

        io.write(0x314, &i64_to_bytes("Gain", 32, 2).unwrap())
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

        io.write(0x310, &i64_to_bytes("Gain", 12, 2).unwrap())
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
            (0x2000, i64_to_bytes("RegAddr", 0x3000, 4).unwrap()),
            (0x3000, i64_to_bytes("Gain", 123, 4).unwrap()),
            (0x3100, i64_to_bytes("Gain", 77, 4).unwrap()),
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

    #[test]
    fn indirect_bad_address() {
        let mut nodemap = build_indirect_nodemap();
        let io = MockIo::with_registers(&[(0x2000, vec![0, 0, 0, 0])]);

        nodemap
            .set_integer("RegAddr", 0, &io)
            .expect("write zero address");
        let err = nodemap.get_integer("Gain", &io).unwrap_err();
        match err {
            GenApiError::BadIndirectAddress { name, addr } => {
                assert_eq!(name, "Gain");
                assert_eq!(addr, 0);
            }
            other => panic!("unexpected error: {other:?}"),
        }
        assert_eq!(io.read_count(0x2000), 0);
    }

    #[test]
    fn enum_literal_entry_read() {
        let nodemap = build_enum_pvalue_nodemap();
        let io = MockIo::with_registers(&[
            (0x4000, i64_to_bytes("Mode", 10, 4).unwrap()),
            (0x4100, i64_to_bytes("RegModeVal", 42, 4).unwrap()),
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
            (0x4000, i64_to_bytes("Mode", 42, 4).unwrap()),
            (0x4100, i64_to_bytes("RegModeVal", 42, 4).unwrap()),
        ]);

        let value = nodemap.get_enum("Mode", &io).expect("read dynamic mode");
        assert_eq!(value, "DynFromReg");
        assert_eq!(io.read_count(0x4100), 1);
    }

    #[test]
    fn enum_set_uses_provider_value() {
        let mut nodemap = build_enum_pvalue_nodemap();
        let io = MockIo::with_registers(&[
            (0x4000, i64_to_bytes("Mode", 0, 4).unwrap()),
            (0x4100, i64_to_bytes("RegModeVal", 42, 4).unwrap()),
        ]);

        nodemap
            .set_enum("Mode", "DynFromReg", &io)
            .expect("write enum");
        let raw = bytes_to_i64("Mode", &io.read(0x4000, 4).unwrap()).unwrap();
        assert_eq!(raw, 42);
        assert_eq!(io.read_count(0x4100), 1);
    }

    #[test]
    fn enum_provider_update_invalidates_mapping() {
        let mut nodemap = build_enum_pvalue_nodemap();
        let io = MockIo::with_registers(&[
            (0x4000, i64_to_bytes("Mode", 42, 4).unwrap()),
            (0x4100, i64_to_bytes("RegModeVal", 42, 4).unwrap()),
        ]);

        assert_eq!(nodemap.get_enum("Mode", &io).unwrap(), "DynFromReg");
        assert_eq!(io.read_count(0x4100), 1);

        nodemap
            .set_integer("RegModeVal", 17, &io)
            .expect("update provider");
        io.write(0x4000, &i64_to_bytes("Mode", 0, 4).unwrap())
            .expect("reset mode register");

        nodemap
            .set_enum("Mode", "DynFromReg", &io)
            .expect("write enum after provider change");
        let raw = bytes_to_i64("Mode", &io.read(0x4000, 4).unwrap()).unwrap();
        assert_eq!(raw, 17);
    }

    #[test]
    fn enum_unknown_value_error() {
        let nodemap = build_enum_pvalue_nodemap();
        let io = MockIo::with_registers(&[
            (0x4000, i64_to_bytes("Mode", 99, 4).unwrap()),
            (0x4100, i64_to_bytes("RegModeVal", 42, 4).unwrap()),
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
            (0x3000, i64_to_bytes("GainRaw", 100, 4).unwrap()),
            (0x3008, i64_to_bytes("Offset", 3, 4).unwrap()),
            (0x3010, i64_to_bytes("B", 1, 4).unwrap()),
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
            (0x3000, i64_to_bytes("GainRaw", 5, 4).unwrap()),
            (0x3008, i64_to_bytes("Offset", 0, 4).unwrap()),
            (0x3010, i64_to_bytes("B", 1, 4).unwrap()),
        ]);

        let divided = nodemap
            .get_integer("DivideInt", &io)
            .expect("integer division");
        assert_eq!(divided, 2);

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
        let err = NodeMap::try_from_xml(model).expect_err("unknown variable");
        match err {
            GenApiError::UnknownVariable { name, var } => {
                assert_eq!(name, "Bad");
                assert_eq!(var, "Missing");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn swissknife_division_by_zero() {
        let nodemap = build_swissknife_nodemap();
        let io = MockIo::with_registers(&[
            (0x3000, i64_to_bytes("GainRaw", 10, 4).unwrap()),
            (0x3008, i64_to_bytes("Offset", 0, 4).unwrap()),
            (0x3010, i64_to_bytes("B", 0, 4).unwrap()),
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
        let nodemap = NodeMap::from(model);

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
        let nodemap = NodeMap::from(model);

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

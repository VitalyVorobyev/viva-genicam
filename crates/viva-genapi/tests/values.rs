//! What a feature write sends and what a feature reports: command values,
//! limits and selectors.
//!
//! Every fixture here is a cut-down copy of a shape the vendor corpus ships,
//! with the document cited, and every expectation is a literal derived from the
//! XML in the test, never from the code under test. The GenICam Standard
//! v2.1.1 sections each test pins are named in its comment.

use std::cell::RefCell;
use std::collections::HashMap;

use viva_genapi::{FeatureValue, GenApiError, NodeMap, RegisterIo, ValueRange};

/// Register memory that records every write in order. A read of an address
/// nothing wrote returns zeros, as an idle device register would.
#[derive(Default)]
struct MemIo {
    regs: RefCell<HashMap<u64, Vec<u8>>>,
    writes: RefCell<Vec<(u64, Vec<u8>)>>,
    reads: RefCell<Vec<u64>>,
}

impl MemIo {
    fn with(regs: &[(u64, Vec<u8>)]) -> Self {
        let io = Self::default();
        for (addr, bytes) in regs {
            io.regs.borrow_mut().insert(*addr, bytes.clone());
        }
        io
    }

    /// Change a register the way the device would, without a write being
    /// recorded.
    fn device_sets(&self, addr: u64, bytes: &[u8]) {
        self.regs.borrow_mut().insert(addr, bytes.to_vec());
    }

    fn writes(&self) -> Vec<(u64, Vec<u8>)> {
        self.writes.borrow().clone()
    }

    fn reads_of(&self, addr: u64) -> usize {
        self.reads.borrow().iter().filter(|a| **a == addr).count()
    }

    fn writes_to(&self, addr: u64) -> Vec<Vec<u8>> {
        self.writes
            .borrow()
            .iter()
            .filter(|(a, _)| *a == addr)
            .map(|(_, bytes)| bytes.clone())
            .collect()
    }
}

impl RegisterIo for MemIo {
    fn read(&self, addr: u64, len: usize) -> Result<Vec<u8>, GenApiError> {
        self.reads.borrow_mut().push(addr);
        Ok(self
            .regs
            .borrow()
            .get(&addr)
            .cloned()
            .unwrap_or_else(|| vec![0; len]))
    }

    fn write(&self, addr: u64, data: &[u8]) -> Result<(), GenApiError> {
        self.regs.borrow_mut().insert(addr, data.to_vec());
        self.writes.borrow_mut().push((addr, data.to_vec()));
        Ok(())
    }
}

fn nodemap(xml: &str) -> NodeMap {
    let model = viva_genapi_xml::parse(xml).expect("parse");
    NodeMap::try_from_xml(model).expect("build nodemap")
}

fn be32(value: u32) -> Vec<u8> {
    value.to_be_bytes().to_vec()
}

// ── Commands ────────────────────────────────────────────────────────────────

/// The Hikrobot MV-CS050-10GC user-set block
/// (`Hikrobot_MV-CS050-10GC.xml:23033-23101`), addresses shortened: the
/// command's value is the selected user set's index, read through an
/// `<IntSwissKnife>` over the selector register, and saving is unavailable
/// while `Default` is selected.
const USER_SET_XML: &str = r#"
    <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="0">
        <Enumeration Name="UserSetSelector">
            <pSelected>UserSetSave</pSelected>
            <EnumEntry Name="Default"><Value>0</Value></EnumEntry>
            <EnumEntry Name="UserSet1"><Value>1</Value></EnumEntry>
            <EnumEntry Name="UserSet2"><Value>2</Value></EnumEntry>
            <pValue>UserSetSelector_Reg</pValue>
        </Enumeration>
        <IntReg Name="UserSetSelector_Reg">
            <Address>0x1000</Address><Length>4</Length><AccessMode>RW</AccessMode>
            <Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
        </IntReg>
        <Command Name="UserSetSave">
            <pIsAvailable>IsDefaultSet</pIsAvailable>
            <ImposedAccessMode>WO</ImposedAccessMode>
            <pValue>UserSetSave_Reg</pValue>
            <pCommandValue>UserSetSave_CmdValueFao</pCommandValue>
        </Command>
        <IntSwissKnife Name="UserSetSave_CmdValueFao">
            <pVariable Name="VALUE">UserSetSelector_Reg</pVariable>
            <Formula>VALUE</Formula>
        </IntSwissKnife>
        <IntSwissKnife Name="IsDefaultSet">
            <pVariable Name="VAL">UserSetSelector_Reg</pVariable>
            <Formula>(VAL = 0)? 0:1</Formula>
        </IntSwissKnife>
        <IntReg Name="UserSetSave_Reg">
            <Address>0x1004</Address><Length>4</Length><AccessMode>WO</AccessMode>
            <Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
        </IntReg>
    </RegisterDescription>
"#;

/// GA-35. GenICam Standard v2.1.1 §2.8.8: "The CommandValue can
/// alternatively also be taken from pCommandValue." With `UserSet2`
/// selected, saving must write 2 — the value of `UserSetSave_CmdValueFao` —
/// and not a constant 1, which saves into a different user set.
#[test]
fn a_command_writes_the_value_its_p_command_value_names() {
    let mut nodemap = nodemap(USER_SET_XML);
    let io = MemIo::default();
    nodemap
        .set_enum("UserSetSelector", "UserSet2", &io)
        .expect("select UserSet2");
    nodemap.exec_command("UserSetSave", &io).expect("save");
    assert_eq!(io.writes_to(0x1004), [be32(2)]);
}

/// GA-35: the command value is read when the command runs, so selecting a
/// different user set afterwards changes what the next save writes.
#[test]
fn a_command_value_follows_its_node_between_executions() {
    let mut nodemap = nodemap(USER_SET_XML);
    let io = MemIo::default();
    nodemap
        .set_enum("UserSetSelector", "UserSet1", &io)
        .expect("select UserSet1");
    nodemap.exec_command("UserSetSave", &io).expect("save 1");
    nodemap
        .set_enum("UserSetSelector", "UserSet2", &io)
        .expect("select UserSet2");
    nodemap.exec_command("UserSetSave", &io).expect("save 2");
    assert_eq!(io.writes_to(0x1004), [be32(1), be32(2)]);
}

/// GA-35: the node a command takes its value from is one of its
/// dependencies, so the graph a viewer walks names it.
#[test]
fn the_p_command_value_node_is_a_dependency_of_the_command() {
    let nodemap = nodemap(USER_SET_XML);
    assert!(
        nodemap
            .dependents("UserSetSave_CmdValueFao")
            .contains(&"UserSetSave".to_string()),
        "{:?}",
        nodemap.dependents("UserSetSave_CmdValueFao")
    );
}

/// GA-35. §2.8.8 gives a command its value through `<CommandValue>` or
/// `<pCommandValue>`, and §3.2's table of defaults has no entry for either:
/// there is no value to fall back on. All 490 commands in the vendor corpus
/// declare one. A command declaring neither is refused rather than executed
/// with an invented 1.
#[test]
fn a_command_without_a_value_is_refused() {
    const XML: &str = r#"
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="0">
            <Command Name="NoValue">
                <pValue>NoValueReg</pValue>
            </Command>
            <IntReg Name="NoValueReg">
                <Address>0x2000</Address><Length>4</Length><AccessMode>WO</AccessMode>
                <Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
            </IntReg>
        </RegisterDescription>
    "#;
    let mut nodemap = nodemap(XML);
    let io = MemIo::default();
    assert!(nodemap.exec_command("NoValue", &io).is_err());
    assert!(io.writes().is_empty(), "{:?}", io.writes());
    let skipped = nodemap.skipped();
    assert!(
        skipped.iter().any(|s| s.name.as_deref() == Some("NoValue")
            && s.error.contains("CommandValue")
            && s.error.contains("pCommandValue")),
        "{skipped:?}"
    );
}

/// GA-10. A command's own `<pIsAvailable>` gates it: Hikrobot's
/// `UserSetSave` is unavailable while `Default` is selected
/// (`Hikrobot_MV-CS050-10GC.xml:23039`), and executing it then must not reach
/// the register. Only the target register's guards were checked before.
#[test]
fn a_pvalue_command_honours_its_own_availability() {
    let mut nodemap = nodemap(USER_SET_XML);
    let io = MemIo::default();
    let err = nodemap
        .exec_command("UserSetSave", &io)
        .expect_err("Default is selected");
    assert!(matches!(err, GenApiError::Unavailable(_)), "{err:?}");
    assert!(io.writes_to(0x1004).is_empty(), "{:?}", io.writes());
}

/// GA-10: a command's own `<pIsLocked>` refuses it, on the `<pValue>` path.
#[test]
fn a_pvalue_command_honours_its_own_lock() {
    const XML: &str = r#"
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="0">
            <Command Name="TriggerSoftware">
                <pIsLocked>TriggerLocked</pIsLocked>
                <pValue>TriggerSoftwareReg</pValue>
                <CommandValue>1</CommandValue>
            </Command>
            <IntReg Name="TriggerSoftwareReg">
                <Address>0x2000</Address><Length>4</Length><AccessMode>WO</AccessMode>
                <Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
            </IntReg>
            <IntReg Name="TriggerLocked">
                <Address>0x2004</Address><Length>4</Length><AccessMode>RO</AccessMode>
                <Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
            </IntReg>
        </RegisterDescription>
    "#;
    let mut nodemap = nodemap(XML);
    let io = MemIo::with(&[(0x2004, be32(1))]);
    let err = nodemap
        .exec_command("TriggerSoftware", &io)
        .expect_err("locked");
    assert!(
        matches!(err, GenApiError::Locked { ref locked_by, .. } if locked_by == "TriggerLocked"),
        "{err:?}"
    );
    assert!(io.writes().is_empty(), "{:?}", io.writes());

    // Negative control: unlocked, the same command writes its value.
    io.device_sets(0x2004, &be32(0));
    let mut nodemap = self::nodemap(XML);
    nodemap
        .exec_command("TriggerSoftware", &io)
        .expect("unlocked");
    assert_eq!(io.writes_to(0x2000), [be32(1)]);
}

/// GA-10: the direct-address command — a `<Command>` with its own
/// `<Address>`, which this crate accepts although no corpus document uses it —
/// goes through the same guards as the `<pValue>` one.
#[test]
fn a_direct_address_command_honours_its_own_availability() {
    const XML: &str = r#"
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="0">
            <Command Name="Reset">
                <pIsAvailable>ResetAvailable</pIsAvailable>
                <Address>0x3000</Address>
                <Length>4</Length>
                <CommandValue>1</CommandValue>
            </Command>
            <IntReg Name="ResetAvailable">
                <Address>0x3004</Address><Length>4</Length><AccessMode>RO</AccessMode>
                <Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
            </IntReg>
        </RegisterDescription>
    "#;
    let mut nodemap = nodemap(XML);
    let io = MemIo::default();
    let err = nodemap.exec_command("Reset", &io).expect_err("unavailable");
    assert!(matches!(err, GenApiError::Unavailable(_)), "{err:?}");
    assert!(io.writes().is_empty(), "{:?}", io.writes());
}

// ── Command completion ──────────────────────────────────────────────────────

/// A self-clearing command: its register reads back the command value until
/// the device has finished, then clears. Basler's `UserSetLoad` reaches an
/// `RW` register this way (`Basler_acA1600_20gm.xml`, `UserSetLoad_CtrlValueFao`),
/// as 213 of the 490 corpus command targets are `RW`.
const SELF_CLEARING_XML: &str = r#"
    <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="0">
        <Command Name="UserSetLoad">
            <pValue>UserSetLoadReg</pValue>
            <CommandValue>1</CommandValue>
        </Command>
        <IntReg Name="UserSetLoadReg">
            <Address>0x4000</Address><Length>4</Length><AccessMode>RW</AccessMode>
            <Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
        </IntReg>
        <Float Name="ExposureTime">
            <pValue>ExposureTimeReg</pValue>
        </Float>
        <FloatReg Name="ExposureTimeReg">
            <Address>0x4008</Address><Length>8</Length><AccessMode>RW</AccessMode>
            <Endianess>BigEndian</Endianess>
            <pInvalidator>UserSetLoadReg</pInvalidator>
        </FloatReg>
        <Command Name="TriggerSoftware">
            <pValue>TriggerSoftwareReg</pValue>
            <CommandValue>1</CommandValue>
        </Command>
        <IntReg Name="TriggerSoftwareReg">
            <Address>0x4010</Address><Length>4</Length><AccessMode>WO</AccessMode>
            <Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
        </IntReg>
        <Command Name="UserSetSave">
            <ImposedAccessMode>WO</ImposedAccessMode>
            <pValue>UserSetSaveReg</pValue>
            <CommandValue>1</CommandValue>
        </Command>
        <IntReg Name="UserSetSaveReg">
            <Address>0x4014</Address><Length>4</Length><AccessMode>RW</AccessMode>
            <Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
        </IntReg>
    </RegisterDescription>
"#;

/// GA-36. §2.8.8: "IsDone reads the value back and returns false as long as
/// return value equals the command value", and "always read[s] the device
/// register regardless of the state of the caches" — the write just cached
/// the command value, so answering from the cache would never finish.
#[test]
fn is_done_reads_the_register_until_it_no_longer_holds_the_command_value() {
    let mut nodemap = nodemap(SELF_CLEARING_XML);
    let io = MemIo::default();
    nodemap.exec_command("UserSetLoad", &io).expect("execute");
    assert!(!nodemap.is_done("UserSetLoad", &io).expect("running"));
    assert!(!nodemap.is_done("UserSetLoad", &io).expect("still running"));
    io.device_sets(0x4000, &be32(0));
    assert!(nodemap.is_done("UserSetLoad", &io).expect("finished"));
    assert_eq!(io.reads_of(0x4000), 3, "every IsDone reads the device");
}

/// GA-36: "If the node is WriteOnly IsDone always returns true" — whether
/// the register says so or the command imposes it (Hikrobot's `UserSetSave`,
/// `Hikrobot_MV-CS050-10GC.xml:23040`). Nothing is read: the device refuses
/// to read a write-only register.
#[test]
fn a_write_only_command_is_done_at_once() {
    let mut nodemap = nodemap(SELF_CLEARING_XML);
    let io = MemIo::default();
    for (name, addr) in [("TriggerSoftware", 0x4010), ("UserSetSave", 0x4014)] {
        nodemap.exec_command(name, &io).expect(name);
        assert!(nodemap.is_done(name, &io).expect(name), "{name}");
        assert_eq!(io.reads_of(addr), 0, "{name}");
    }
}

/// GA-36: "The state of other nodes can depend on the completion of the
/// Command". A feature read while the command ran must be read again once it
/// has finished, not answered from what was cached mid-way.
#[test]
fn completion_refreshes_what_depends_on_the_command() {
    let mut nodemap = nodemap(SELF_CLEARING_XML);
    let io = MemIo::with(&[(0x4008, 20000.0f64.to_be_bytes().to_vec())]);
    nodemap.exec_command("UserSetLoad", &io).expect("execute");
    assert_eq!(
        nodemap.get_float("ExposureTime", &io).expect("mid-way"),
        20000.0
    );
    // The device finishes loading: the exposure changes, the command clears.
    io.device_sets(0x4008, &5000.0f64.to_be_bytes());
    io.device_sets(0x4000, &be32(0));
    assert!(nodemap.is_done("UserSetLoad", &io).expect("finished"));
    assert_eq!(
        nodemap.get_float("ExposureTime", &io).expect("after"),
        5000.0
    );
}

/// GA-36: `is_done` is a command's query.
#[test]
fn is_done_on_a_feature_that_is_not_a_command_is_a_type_error() {
    let nodemap = nodemap(SELF_CLEARING_XML);
    let err = nodemap
        .is_done("ExposureTime", &MemIo::default())
        .expect_err("not a command");
    assert!(matches!(err, GenApiError::Type { .. }), "{err:?}");
}

// ── Limits ──────────────────────────────────────────────────────────────────

/// GA-37. §2.8.5: an `<Integer>` merges the value of its `<pValue>` with its
/// own restriction parameters. A `<Min>`, `<Max>` or `<Inc>` declared on the
/// front node binds the write even though the register behind it declares
/// none — the shape of SFNC `Width` on 1 861 corpus `<Integer>` nodes.
#[test]
fn limits_on_a_pvalue_integer_bound_its_writes() {
    const XML: &str = r#"
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="0">
            <Integer Name="Width">
                <pValue>WidthReg</pValue>
                <Min>16</Min><Max>4096</Max><Inc>4</Inc>
            </Integer>
            <IntReg Name="WidthReg">
                <Address>0x100</Address><Length>4</Length><AccessMode>RW</AccessMode>
                <Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
            </IntReg>
        </RegisterDescription>
    "#;
    let mut nodemap = nodemap(XML);
    let io = MemIo::default();
    for refused in [8, 17, 8192] {
        let err = nodemap
            .set_integer("Width", refused, &io)
            .expect_err("outside Min/Max/Inc");
        assert!(
            matches!(err, GenApiError::Range { .. }),
            "{refused}: {err:?}"
        );
    }
    assert!(io.writes().is_empty(), "{:?}", io.writes());
    nodemap.set_integer("Width", 20, &io).expect("16 + 1 * 4");
    assert_eq!(io.writes(), [(0x100, be32(20))]);
}

/// GA-04. A bound read from the device through `<pMin>`/`<pMax>` binds a
/// write as a declared one does, both on a register node and through
/// `<pValue>` (the fake camera's `Width` and FLIR's `Width` shapes).
#[test]
fn dynamic_bounds_bind_a_write() {
    const XML: &str = r#"
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="0">
            <Integer Name="Direct">
                <Address>0x100</Address><Length>4</Length><AccessMode>RW</AccessMode>
                <pMin>MinReg</pMin><pMax>MaxReg</pMax>
            </Integer>
            <Integer Name="Delegating">
                <pValue>DelegatingReg</pValue>
                <pMin>MinReg</pMin><pMax>MaxReg</pMax>
            </Integer>
            <IntReg Name="DelegatingReg">
                <Address>0x104</Address><Length>4</Length><AccessMode>RW</AccessMode>
                <Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
            </IntReg>
            <IntReg Name="MinReg">
                <Address>0x108</Address><Length>4</Length><AccessMode>RO</AccessMode>
                <Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
            </IntReg>
            <IntReg Name="MaxReg">
                <Address>0x10C</Address><Length>4</Length><AccessMode>RO</AccessMode>
                <Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
            </IntReg>
        </RegisterDescription>
    "#;
    let io = MemIo::with(&[(0x108, be32(32)), (0x10C, be32(1024))]);
    for name in ["Direct", "Delegating"] {
        let mut nodemap = nodemap(XML);
        for refused in [16, 2048] {
            let err = nodemap
                .set_integer(name, refused, &io)
                .expect_err("outside [32, 1024]");
            assert!(matches!(err, GenApiError::Range { .. }), "{name}: {err:?}");
        }
        nodemap.set_integer(name, 512, &io).expect(name);
    }
    assert_eq!(io.writes(), [(0x100, be32(512)), (0x104, be32(512))]);
}

/// GA-33. `<pInc>` on an `<Integer>` binds the write and is the increment the
/// feature reports (377 corpus nodes, 27 documents).
#[test]
fn an_integer_p_inc_binds_the_write_and_the_reported_range() {
    const XML: &str = r#"
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="0">
            <Integer Name="OffsetX">
                <pValue>OffsetXReg</pValue>
                <Min>16</Min><Max>1024</Max>
                <pInc>OffsetXInc</pInc>
            </Integer>
            <IntReg Name="OffsetXReg">
                <Address>0x100</Address><Length>4</Length><AccessMode>RW</AccessMode>
                <Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
            </IntReg>
            <IntReg Name="OffsetXInc">
                <Address>0x104</Address><Length>4</Length><AccessMode>RO</AccessMode>
                <Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
            </IntReg>
        </RegisterDescription>
    "#;
    let mut nodemap = nodemap(XML);
    let io = MemIo::with(&[(0x100, be32(32)), (0x104, be32(8))]);
    let state = nodemap.feature_state("OffsetX", &io).expect("state");
    assert_eq!(
        state.range,
        Some(ValueRange {
            min: 16.0,
            max: 1024.0,
            inc: Some(8.0)
        })
    );
    let err = nodemap
        .set_integer("OffsetX", 20, &io)
        .expect_err("16 + 4 is not a step of 8");
    assert!(matches!(err, GenApiError::Range { .. }), "{err:?}");
    nodemap.set_integer("OffsetX", 24, &io).expect("16 + 8");
    assert_eq!(io.writes(), [(0x100, be32(24))]);
}

/// GA-33. §2.8.9: a `<Float>` has `<pMin>`/`<pMax>` too (390/372 corpus
/// nodes). FLIR's `ExposureTime` takes both from `<IntSwissKnife>`s over
/// device registers; here they are plain registers.
#[test]
fn float_dynamic_bounds_bind_the_write_and_the_reported_range() {
    const XML: &str = r#"
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="0">
            <Float Name="ExposureTime">
                <pValue>ExposureTimeReg</pValue>
                <pMin>ExposureTimeMin</pMin>
                <pMax>ExposureTimeMax</pMax>
            </Float>
            <FloatReg Name="ExposureTimeReg">
                <Address>0x100</Address><Length>8</Length><AccessMode>RW</AccessMode>
                <Endianess>BigEndian</Endianess>
            </FloatReg>
            <FloatReg Name="ExposureTimeMin">
                <Address>0x108</Address><Length>8</Length><AccessMode>RO</AccessMode>
                <Endianess>BigEndian</Endianess>
            </FloatReg>
            <FloatReg Name="ExposureTimeMax">
                <Address>0x110</Address><Length>8</Length><AccessMode>RO</AccessMode>
                <Endianess>BigEndian</Endianess>
            </FloatReg>
        </RegisterDescription>
    "#;
    let mut nodemap = nodemap(XML);
    let io = MemIo::with(&[
        (0x100, 5000.0f64.to_be_bytes().to_vec()),
        (0x108, 12.5f64.to_be_bytes().to_vec()),
        (0x110, 30000.0f64.to_be_bytes().to_vec()),
    ]);
    let state = nodemap.feature_state("ExposureTime", &io).expect("state");
    assert_eq!(state.value, FeatureValue::Float(5000.0));
    assert_eq!(
        state.range,
        Some(ValueRange {
            min: 12.5,
            max: 30000.0,
            inc: None
        })
    );
    for refused in [10.0, 30000.5] {
        let err = nodemap
            .set_float("ExposureTime", refused, &io)
            .expect_err("outside [12.5, 30000]");
        assert!(
            matches!(err, GenApiError::Range { .. }),
            "{refused}: {err:?}"
        );
    }
    assert!(io.writes().is_empty(), "{:?}", io.writes());
    nodemap
        .set_float("ExposureTime", 20000.0, &io)
        .expect("inside");
    assert_eq!(io.writes(), [(0x100, 20000.0f64.to_be_bytes().to_vec())]);
}

/// GA-07. §2.8.13: a `<Converter>` reports its `<pValue>`'s range through
/// `<FormulaFrom>`, and `<Slope>Decreasing</Slope>` means the raw maximum
/// becomes the feature's minimum. `Increasing` keeps the order and
/// `Automatic` (the §3.2 default) finds it by probing.
#[test]
fn a_converter_reports_its_range_through_its_slope() {
    const XML: &str = r#"
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="0">
            <Integer Name="Raw">
                <pValue>RawReg</pValue>
                <Min>10</Min><Max>50</Max>
            </Integer>
            <IntReg Name="RawReg">
                <Address>0x100</Address><Length>4</Length><AccessMode>RW</AccessMode>
                <Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
            </IntReg>
            <Converter Name="Down">
                <FormulaTo>100 - FROM</FormulaTo>
                <FormulaFrom>100 - TO</FormulaFrom>
                <pValue>Raw</pValue>
                <Slope>Decreasing</Slope>
            </Converter>
            <Converter Name="Up">
                <FormulaTo>FROM / 2</FormulaTo>
                <FormulaFrom>TO * 2</FormulaFrom>
                <pValue>Raw</pValue>
                <Slope>Increasing</Slope>
            </Converter>
            <Converter Name="Probed">
                <FormulaTo>200 - FROM</FormulaTo>
                <FormulaFrom>200 - TO</FormulaFrom>
                <pValue>Raw</pValue>
            </Converter>
            <SwissKnife Name="DownSpan">
                <pVariable Name="D">Down</pVariable>
                <Formula>D.Max - D.Min</Formula>
            </SwissKnife>
        </RegisterDescription>
    "#;
    let nodemap = nodemap(XML);
    let io = MemIo::with(&[(0x100, be32(20))]);
    let range = |name: &str| nodemap.feature_state(name, &io).expect(name).range;
    let span = |min: f64, max: f64| {
        Some(ValueRange {
            min,
            max,
            inc: None,
        })
    };
    // 100 - 50 = 50 is the smallest feature value, 100 - 10 = 90 the largest.
    assert_eq!(range("Down"), span(50.0, 90.0));
    assert_eq!(range("Up"), span(20.0, 100.0));
    assert_eq!(range("Probed"), span(150.0, 190.0));
    assert_eq!(nodemap.get_float("DownSpan", &io).expect("span"), 40.0);
}

/// GA-34. A bound declared as exactly `i64::MIN`/`i64::MAX` is a declared
/// bound, not a missing one, so the feature reports a range.
#[test]
fn a_declared_full_domain_bound_is_reported() {
    const XML: &str = r#"
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="0">
            <Integer Name="Counter">
                <Address>0x100</Address><Length>8</Length><AccessMode>RW</AccessMode>
                <Min>-9223372036854775808</Min>
                <Max>9223372036854775807</Max>
                <Sign>Signed</Sign>
            </Integer>
        </RegisterDescription>
    "#;
    let nodemap = nodemap(XML);
    let state = nodemap
        .feature_state("Counter", &MemIo::default())
        .expect("state");
    assert_eq!(
        state.range,
        Some(ValueRange {
            min: i64::MIN as f64,
            max: i64::MAX as f64,
            inc: None
        })
    );
}

// ── Floating values ─────────────────────────────────────────────────────────

/// GenICam Standard v2.1.1 §2.8.5's own example: an `<Integer>` with a
/// `<Value>` and no `<pValue>` "is a 'floating' variable that can be set by
/// the user to any value allowed by the restriction parameters. The given
/// constant is the start value." 319 features in 32 corpus documents write
/// through one — AVT's `TriggerSelector` (`AVT_Manta_G125B.xml:867`), Sony's
/// `UserSetSelector` (`Sony_CVC_EH6300.xml:4778`) — and every such write
/// failed with "no addressing or pValue".
#[test]
fn a_floating_integer_holds_what_is_written_within_its_limits() {
    const XML: &str = r#"
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="0">
            <Integer Name="Index">
                <Value>0</Value>
                <Min>0</Min>
                <Max>255</Max>
                <Inc>2</Inc>
            </Integer>
            <IntSwissKnife Name="Doubled">
                <pVariable Name="I">Index</pVariable>
                <Formula>I * 2</Formula>
            </IntSwissKnife>
        </RegisterDescription>
    "#;
    let mut nodemap = nodemap(XML);
    let io = MemIo::default();
    assert_eq!(nodemap.get_integer("Index", &io).expect("start"), 0);
    assert_eq!(nodemap.get_integer("Doubled", &io).expect("start"), 0);
    nodemap
        .set_integer("Index", 4, &io)
        .expect("0, 2, 4, ..., 254");
    assert_eq!(nodemap.get_integer("Index", &io).expect("written"), 4);
    assert_eq!(nodemap.get_integer("Doubled", &io).expect("follows"), 8);
    for refused in [3, 256] {
        let err = nodemap
            .set_integer("Index", refused, &io)
            .expect_err("outside the restriction parameters");
        assert!(
            matches!(err, GenApiError::Range { .. }),
            "{refused}: {err:?}"
        );
    }
    assert_eq!(nodemap.get_integer("Index", &io).expect("unchanged"), 4);
    // A floating value lives in the nodemap, not in the device: invalidation
    // must not reset it to the start value.
    nodemap.invalidate("Index");
    assert_eq!(nodemap.get_integer("Index", &io).expect("kept"), 4);
    assert!(io.writes().is_empty(), "{:?}", io.writes());
}

// ── Selectors ───────────────────────────────────────────────────────────────

/// The Basler acA1600-20gm gain block (`Basler_acA1600_20gm.xml:518-547`, the
/// `GainSelector` that names `GainRaw` in `<pSelected>`), cut down: the
/// selected feature's register is not addressed through the selector, so
/// `<pSelected>` is the only link between the two.
const SELECTOR_XML: &str = r#"
    <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="0">
        <Enumeration Name="GainSelector">
            <pSelected>GainRaw</pSelected>
            <EnumEntry Name="All"><Value>0</Value></EnumEntry>
            <EnumEntry Name="Red"><Value>1</Value></EnumEntry>
            <pValue>GainSelectorReg</pValue>
        </Enumeration>
        <IntReg Name="GainSelectorReg">
            <Address>0x100</Address><Length>4</Length><AccessMode>RW</AccessMode>
            <Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
        </IntReg>
        <Integer Name="GainRaw">
            <pValue>GainRawReg</pValue>
        </Integer>
        <IntReg Name="GainRawReg">
            <Address>0x104</Address><Length>4</Length><AccessMode>RW</AccessMode>
            <Sign>Unsigned</Sign><Endianess>BigEndian</Endianess>
        </IntReg>
    </RegisterDescription>
"#;

/// GA-03. §2.8.4: `<pSelected>` sits on the selector and names the features
/// it selects, which "will show a different value depending on the value of
/// selector node". So writing the selector must make the selected feature
/// read the device again.
#[test]
fn writing_a_selector_refreshes_the_features_it_selects() {
    let mut nodemap = nodemap(SELECTOR_XML);
    let io = MemIo::with(&[(0x104, be32(7))]);
    assert_eq!(nodemap.get_integer("GainRaw", &io).expect("All"), 7);

    // The device now shows the red channel's gain behind the same register.
    io.device_sets(0x104, &be32(9));
    assert_eq!(
        nodemap.get_integer("GainRaw", &io).expect("cached"),
        7,
        "negative control: nothing has invalidated GainRaw yet"
    );
    nodemap
        .set_enum("GainSelector", "Red", &io)
        .expect("select Red");
    assert_eq!(nodemap.get_integer("GainRaw", &io).expect("Red"), 9);
}

/// GA-03: the edge runs selector → selected and not the other way round.
#[test]
fn p_selected_edges_run_from_the_selector_to_the_selected() {
    let nodemap = nodemap(SELECTOR_XML);
    assert!(
        nodemap
            .invalidated_by("GainSelector")
            .contains(&"GainRaw".to_string()),
        "{:?}",
        nodemap.invalidated_by("GainSelector")
    );
    assert!(
        !nodemap
            .invalidated_by("GainRaw")
            .contains(&"GainSelector".to_string()),
        "{:?}",
        nodemap.invalidated_by("GainRaw")
    );
}

//! What a `GenApiError` says, and how a caller tells its cases apart
//! (ADR-0030, backlog API-03 and GA-30).

use std::cell::RefCell;
use std::collections::HashMap;

use viva_genapi::{GenApiError, NodeMap, RegisterIo};

const XML: &str = r#"
<RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="0">
  <IntReg Name="Counter">
    <Address>0x100</Address>
    <Length>4</Length>
    <AccessMode>RO</AccessMode>
  </IntReg>
  <IntReg Name="Key">
    <Address>0x104</Address>
    <Length>4</Length>
    <AccessMode>WO</AccessMode>
  </IntReg>
  <Float Name="Gain">
    <Address>0x108</Address>
    <Length>4</Length>
    <AccessMode>RW</AccessMode>
    <Scale>0/1</Scale>
  </Float>
</RegisterDescription>
"#;

/// Register memory that answers a read of `short` with no bytes at all: a
/// transport that came back empty-handed.
#[derive(Default)]
struct Io {
    regs: RefCell<HashMap<u64, Vec<u8>>>,
    short: Option<u64>,
}

impl RegisterIo for Io {
    fn read(&self, addr: u64, len: usize) -> Result<Vec<u8>, GenApiError> {
        if Some(addr) == self.short {
            return Ok(Vec::new());
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

fn nodemap() -> NodeMap {
    NodeMap::try_from_xml(viva_genapi_xml::parse(XML).expect("parse")).expect("build")
}

/// The variant's name, from its `Debug` form.
fn variant(err: &GenApiError) -> String {
    format!("{err:?}")
        .split(|c: char| !c.is_alphanumeric())
        .next()
        .unwrap_or_default()
        .to_string()
}

/// GA-30: register bytes that do not decode are not bad XML. The two used to
/// be one `Parse` variant, printed "parse error", so a device that answered a
/// read with nothing read as a broken document.
#[test]
fn a_register_that_does_not_decode_is_not_a_bad_declaration() {
    let map = nodemap();
    let io = Io {
        short: Some(0x100),
        ..Io::default()
    };
    let decode = map.get_integer("Counter", &io).expect_err("no bytes");
    assert!(
        !decode.to_string().contains("parse error"),
        "a failed read reads as a parse error: {decode}"
    );
    assert!(
        matches!(decode, GenApiError::RegisterEncoding { ref name, width: 0, .. } if name == "Counter"),
        "{decode:?}"
    );

    // A zero scale numerator is the document's: it cannot be inverted for a
    // write, whatever the device holds.
    let mut map = nodemap();
    let declaration = map
        .set_float("Gain", 1.0, &Io::default())
        .expect_err("a zero scale cannot be written through");
    assert!(
        matches!(declaration, GenApiError::Declaration { ref name, .. } if name == "Gain"),
        "{declaration:?}"
    );
    assert_ne!(
        variant(&decode),
        variant(&declaration),
        "a decode failure and a declaration problem are one case: {decode:?} / {declaration:?}"
    );
}

/// API-03: an access refusal says which way it was refused.
#[test]
fn an_access_refusal_names_its_direction() {
    let mut map = nodemap();
    let io = Io::default();

    let read = map.get_integer("Key", &io).expect_err("Key is write-only");
    assert!(read.to_string().contains("cannot be read"), "{read}");
    assert!(
        matches!(read, GenApiError::Access { ref name, refused: viva_genapi::AccessDirection::Read } if name == "Key"),
        "{read:?}"
    );

    let write = map
        .set_integer("Counter", 1, &io)
        .expect_err("Counter is read-only");
    assert!(write.to_string().contains("cannot be written"), "{write}");
    assert!(
        matches!(write, GenApiError::Access { ref name, refused: viva_genapi::AccessDirection::Write } if name == "Counter"),
        "{write:?}"
    );
}

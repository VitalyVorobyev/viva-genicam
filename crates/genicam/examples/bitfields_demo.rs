use std::cell::RefCell;
use std::collections::HashMap;
use std::error::Error;

use genicam::genapi::{GenApiError, Node, NodeMap, RegisterIo};

const XML: &str = r#"
<RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
    <Integer Name="LeByte">
        <Address>0x6000</Address>
        <Length>4</Length>
        <AccessMode>RW</AccessMode>
        <Min>0</Min>
        <Max>65535</Max>
        <Mask>0x0000FF00</Mask>
    </Integer>
    <Integer Name="BeBits">
        <Address>0x6004</Address>
        <Length>2</Length>
        <AccessMode>RW</AccessMode>
        <Min>0</Min>
        <Max>15</Max>
        <Lsb>0</Lsb>
        <Msb>2</Msb>
        <Endianness>BigEndian</Endianness>
    </Integer>
    <Boolean Name="PackedFlag">
        <Address>0x6006</Address>
        <Length>4</Length>
        <AccessMode>RW</AccessMode>
        <Bit>13</Bit>
    </Boolean>
</RegisterDescription>
"#;

#[derive(Default)]
struct MockIo {
    regs: RefCell<HashMap<u64, Vec<u8>>>,
}

impl MockIo {
    fn new(initial: &[(u64, Vec<u8>)]) -> Self {
        let mut regs = HashMap::new();
        for (addr, data) in initial {
            regs.insert(*addr, data.clone());
        }
        MockIo {
            regs: RefCell::new(regs),
        }
    }

    fn dump(&self, addr: u64) -> Vec<u8> {
        self.regs.borrow().get(&addr).cloned().unwrap_or_default()
    }
}

impl RegisterIo for MockIo {
    fn read(&self, addr: u64, len: usize) -> Result<Vec<u8>, GenApiError> {
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

fn main() -> Result<(), Box<dyn Error>> {
    let model = genapi_xml::parse(XML)?;
    let mut nodemap = NodeMap::from(model);
    let io = MockIo::new(&[
        (0x6000, vec![0xAA, 0xBB, 0xCC, 0xDD]),
        (0x6004, vec![0b1010_0000, 0b0000_0000]),
        (0x6006, vec![0x00, 0x20, 0x00, 0x00]),
    ]);

    println!("Bitfield demo\n============\n");

    describe_node(&nodemap, "LeByte");
    let before = io.dump(0x6000);
    println!("LeByte register before: {:02X?}", before);
    let le_value = nodemap.get_integer("LeByte", &io)?;
    println!("LeByte read value     : {le_value:#04X}");
    nodemap.set_integer("LeByte", 0x55, &io)?;
    let after = io.dump(0x6000);
    println!("LeByte register after : {:02X?}\n", after);

    describe_node(&nodemap, "BeBits");
    let be_before = io.dump(0x6004);
    println!("BeBits register before: {:02X?}", be_before);
    let be_value = nodemap.get_integer("BeBits", &io)?;
    println!("BeBits read value     : {be_value:#05b}");
    nodemap.set_integer("BeBits", 0b010, &io)?;
    let be_after = io.dump(0x6004);
    println!("BeBits register after : {:02X?}\n", be_after);

    describe_node(&nodemap, "PackedFlag");
    let flag_before = io.dump(0x6006);
    println!("PackedFlag before     : {:02X?}", flag_before);
    let flag = nodemap.get_bool("PackedFlag", &io)?;
    println!("PackedFlag read value : {flag}");
    nodemap.set_bool("PackedFlag", !flag, &io)?;
    let flag_after = io.dump(0x6006);
    println!("PackedFlag after toggle: {:02X?}", flag_after);

    Ok(())
}

fn describe_node(nodemap: &NodeMap, name: &str) {
    if let Some(node) = nodemap.node(name) {
        match node {
            Node::Integer(meta) => {
                if let Some(field) = meta.bitfield {
                    println!(
                        "{name} -> Integer, {:?}, offset {}, length {}",
                        field.byte_order, field.bit_offset, field.bit_length
                    );
                } else {
                    println!("{name} -> Integer, full register");
                }
            }
            Node::Boolean(meta) => {
                if let Some(field) = meta.bitfield {
                    println!(
                        "{name} -> Boolean, {:?}, offset {}, length {}",
                        field.byte_order, field.bit_offset, field.bit_length
                    );
                } else {
                    println!("{name} -> Boolean (pValue-backed)");
                }
            }
            other => println!("{name} -> {:?}", other),
        }
    }
}

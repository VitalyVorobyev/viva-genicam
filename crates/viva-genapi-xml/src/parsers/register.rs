//! Parser for the `<Register>` node type — raw byte-array register access.

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use super::{
    NodeMetaBuilder, handle_addressing_empty, handle_addressing_start, handle_invalidator_start,
    handle_predicate_start,
};
use crate::builders::AddressingBuilder;
use crate::util::{attribute_value_required, read_text_start};
use crate::{AccessMode, NodeDecl, PredicateRefs, RegisterDecl, XmlError};

/// Parse a `<Register>` element into a [`NodeDecl::Register`].
///
/// `<Register>` is the base register type: an address, a byte count and no
/// value interpretation at all. `<StringReg>` is this plus UTF-8/NUL decoding,
/// which is why this parser looks so much like [`super::parse_string`].
///
/// It delegates the address elements to [`handle_addressing_start`] rather than
/// handling them inline, so `<pIndex>` works. `aravis_genicam.xml`'s
/// `IndexedRegister` is the case that needs it: a bare `<pIndex>` strides by the
/// register's own length.
///
/// # `<pLength>`
///
/// A `<Register>` may take its length from another node at runtime (GenICam
/// Standard v2.1.1 §2.8.3); 22 of the vendor corpus's `<Register>`
/// declarations do, and the mvBlueFOX3-2004G's `mvDeviceFirmwareHashCode`.
/// [`handle_addressing_start`] records it in the [`crate::Addressing`], and
/// the GenApi layer reads the length before each access.
pub(crate) fn parse_register(
    reader: &mut Reader<&[u8]>,
    start: BytesStart<'_>,
) -> Result<NodeDecl, XmlError> {
    let name = attribute_value_required(&start, "Name")?;
    let mut addressing = AddressingBuilder::default();
    let mut access = AccessMode::RO;
    let mut predicates = PredicateRefs::default();
    let mut invalidators = Vec::new();
    let node_name = start.name().as_ref().to_string();
    let mut buf = Vec::new();
    let mut meta_builder = NodeMetaBuilder::default();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match e.name().as_ref() {
                "AccessMode" => {
                    let text = read_text_start(reader, e)?;
                    access = AccessMode::parse(&text)?;
                }
                _ => {
                    let handled = handle_addressing_start(reader, e, &name, &mut addressing)?
                        || handle_predicate_start(reader, e, &mut predicates)?
                        || handle_invalidator_start(reader, e, &mut invalidators)?;
                    if !handled {
                        meta_builder.handle_or_ignore(reader, e)?;
                    }
                }
            },
            Ok(Event::Empty(ref e)) => {
                if !handle_addressing_empty(e, &mut addressing)? {
                    meta_builder.ignore_empty(e);
                }
            }
            Ok(Event::End(ref e)) if e.name().as_ref() == node_name.as_str() => break,
            Ok(Event::Eof) => {
                return Err(XmlError::invalid(&name, "ends before its closing tag"));
            }
            Err(err) => return Err(XmlError::malformed(err)),
            _ => {}
        }
        buf.clear();
    }

    Ok(NodeDecl::Register(RegisterDecl {
        name,
        meta: meta_builder.build(),
        addressing: addressing.build(),
        access,
        predicates,
        invalidators,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AddressTerm, Addressing, IndexOffset};

    fn parse_one(xml: &str) -> Result<RegisterDecl, XmlError> {
        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);
        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(e)) if e.name().as_ref() == "Register" => {
                    let owned = e.to_owned();
                    return match parse_register(&mut reader, owned)? {
                        NodeDecl::Register(decl) => Ok(decl),
                        other => panic!("expected a Register decl, got {}", other.kind()),
                    };
                }
                Ok(Event::Eof) => panic!("no <Register> element in fixture"),
                Err(err) => return Err(XmlError::malformed(err)),
                _ => {}
            }
            buf.clear();
        }
    }

    fn terms(addressing: &Addressing) -> &[AddressTerm] {
        match addressing {
            Addressing::Sum { terms, .. } => terms,
        }
    }

    fn len_of(addressing: &Addressing) -> u32 {
        match addressing {
            Addressing::Sum { len, .. } => *len,
        }
    }

    #[test]
    fn plain_address_and_length_are_parsed() {
        let decl = parse_one(
            r#"<Register Name="FileAccessBuffer">
                 <Address>0x10003C50</Address>
                 <Length>100000</Length>
                 <AccessMode>RW</AccessMode>
               </Register>"#,
        )
        .expect("parse");

        assert_eq!(decl.name, "FileAccessBuffer");
        assert_eq!(decl.access, AccessMode::RW);
        assert_eq!(len_of(&decl.addressing), 100_000);
        assert_eq!(terms(&decl.addressing), &[AddressTerm::Fixed(0x1000_3C50)]);
    }

    /// Matches `<StringReg>`: an absent `<AccessMode>` is read-only.
    #[test]
    fn access_mode_defaults_to_read_only() {
        let decl =
            parse_one(r#"<Register Name="R"><Address>0x10</Address><Length>4</Length></Register>"#)
                .expect("parse");
        assert_eq!(decl.access, AccessMode::RO);
    }

    /// The regression test for delegating to `handle_addressing_start` instead
    /// of copying `parse_string`'s reduced address handling, which has no
    /// `<pIndex>` branch. Modelled on `aravis_genicam.xml`'s `IndexedRegister`,
    /// where a bare `<pIndex>` strides by the register's own length.
    #[test]
    fn p_index_and_p_address_terms_survive_in_declaration_order() {
        let decl = parse_one(
            r#"<Register Name="IndexedRegister">
                 <Address>0x1000</Address>
                 <pAddress>BaseAddress</pAddress>
                 <pIndex Offset="1000">IndexA</pIndex>
                 <pIndex>IndexB</pIndex>
                 <Length>8</Length>
               </Register>"#,
        )
        .expect("parse");

        assert_eq!(
            terms(&decl.addressing),
            &[
                AddressTerm::Fixed(0x1000),
                AddressTerm::Node("BaseAddress".to_string()),
                AddressTerm::Index {
                    node: "IndexA".to_string(),
                    offset: IndexOffset::Fixed(1000),
                },
                AddressTerm::Index {
                    node: "IndexB".to_string(),
                    offset: IndexOffset::Length,
                },
            ]
        );
        assert_eq!(len_of(&decl.addressing), 8);
    }

    /// The port is kept with the address; routing the read to the chunk is
    /// the GenApi layer's job (ADR-0026).
    #[test]
    fn p_port_is_captured_verbatim() {
        let decl = parse_one(
            r#"<Register Name="ChunkMeasurementResults">
                 <Address>0x0</Address>
                 <Length>104</Length>
                 <pPort>Chunk4007</pPort>
               </Register>"#,
        )
        .expect("parse");
        assert_eq!(decl.addressing.port(), Some("Chunk4007"));
    }

    #[test]
    fn absent_p_port_means_the_device_port() {
        let decl =
            parse_one(r#"<Register Name="R"><Address>0x10</Address><Length>4</Length></Register>"#)
                .expect("parse");
        assert_eq!(decl.addressing.port(), None);
    }

    /// GenICam Standard v2.1.1 §2.8.3: the length "can be read from another
    /// node using an <pLength> entry". Once refused as GA-09; the
    /// mvBlueFOX3-2004G's `mvDeviceFirmwareHashCode` is one.
    #[test]
    fn p_length_names_the_node_the_length_is_read_from() {
        let decl = parse_one(
            r#"<Register Name="mvDeviceFirmwareHashCode">
                 <Address>0x10001a0</Address>
                 <pLength>mvDeviceFirmwareHash_InqLength</pLength>
                 <AccessMode>RO</AccessMode>
               </Register>"#,
        )
        .expect("a <pLength> register parses");

        assert_eq!(
            decl.addressing.length_node(),
            Some("mvDeviceFirmwareHash_InqLength")
        );
        assert_eq!(len_of(&decl.addressing), 0, "no static length");
        assert!(
            decl.addressing
                .referenced_nodes()
                .contains(&"mvDeviceFirmwareHash_InqLength"),
            "a change of the length node must reach the register"
        );
    }
}

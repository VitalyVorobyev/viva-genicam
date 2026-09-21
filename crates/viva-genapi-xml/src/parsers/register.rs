//! Parser for the `<Register>` node type — raw byte-array register access.

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use super::{
    NodeMetaBuilder, handle_addressing_empty, handle_addressing_start, handle_predicate_start,
};
use crate::builders::AddressingBuilder;
use crate::util::{attribute_value_required, read_text_start, skip_element};
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
/// # `<pLength>` is rejected, deliberately and loudly
///
/// A `<Register>` may take its length from another node at runtime. Nothing in
/// this workspace resolves a dynamic length yet, and 21 of the vendor corpus's
/// 63 `<Register>` declarations use it. Rather than parse such a node and
/// silently read the wrong number of bytes, this returns an error naming
/// `<pLength>`, which `parse` records as a [`crate::SkippedNode`]. Per ADR-0018
/// an incomplete implementation is acceptable where the gap is *visible*; this
/// is what makes it visible.
pub fn parse_register(
    reader: &mut Reader<&[u8]>,
    start: BytesStart<'_>,
) -> Result<NodeDecl, XmlError> {
    let name = attribute_value_required(&start, "Name")?;
    let mut addressing = AddressingBuilder::new(&name);
    let mut access = AccessMode::RO;
    let mut predicates = PredicateRefs::default();
    let mut port: Option<String> = None;
    let mut p_length: Option<String> = None;
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
                "pPort" => {
                    let text = read_text_start(reader, e)?;
                    let target = text.trim();
                    if !target.is_empty() {
                        port = Some(target.to_string());
                    }
                }
                "pLength" => {
                    let text = read_text_start(reader, e)?;
                    p_length = Some(text.trim().to_string());
                }
                _ => {
                    let handled = handle_addressing_start(reader, e, &name, &mut addressing)?
                        || handle_predicate_start(reader, e, &mut predicates)?
                        || meta_builder.handle_start(reader, e)?;
                    if !handled {
                        skip_element(reader, e.name().as_ref())?;
                    }
                }
            },
            Ok(Event::Empty(ref e)) => {
                handle_addressing_empty(e, &mut addressing)?;
            }
            Ok(Event::End(ref e)) if e.name().as_ref() == node_name.as_str() => break,
            Ok(Event::Eof) => {
                return Err(XmlError::Invalid(format!(
                    "unterminated Register node {name}"
                )));
            }
            Err(err) => return Err(XmlError::Xml(err.to_string())),
            _ => {}
        }
        buf.clear();
    }

    if let Some(target) = p_length {
        return Err(XmlError::Invalid(format!(
            "node {name} declares <pLength>{target}</pLength>; \
             a register length resolved from another node is not supported yet (GA-09)"
        )));
    }

    Ok(NodeDecl::Register(RegisterDecl {
        name,
        meta: meta_builder.build(),
        addressing: addressing.build(),
        access,
        port,
        predicates,
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
                Err(err) => return Err(XmlError::Xml(err.to_string())),
                _ => {}
            }
            buf.clear();
        }
    }

    fn terms(addressing: &Addressing) -> &[AddressTerm] {
        match addressing {
            Addressing::Sum { terms, .. } => terms,
            other => panic!("expected Addressing::Sum, got {other:?}"),
        }
    }

    fn len_of(addressing: &Addressing) -> u32 {
        match addressing {
            Addressing::Sum { len, .. } => *len,
            other => panic!("expected Addressing::Sum, got {other:?}"),
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

    /// The node is kept, so it can be listed; refusing to *read* it is the
    /// GenApi layer's job (GA-12).
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
        assert_eq!(decl.port.as_deref(), Some("Chunk4007"));
    }

    #[test]
    fn absent_p_port_means_the_device_port() {
        let decl =
            parse_one(r#"<Register Name="R"><Address>0x10</Address><Length>4</Length></Register>"#)
                .expect("parse");
        assert_eq!(decl.port, None);
    }

    /// The gap must name itself. A `<Register>` dropped for any *other* reason
    /// is a regression, and the corpus test relies on this substring to tell
    /// the two apart.
    #[test]
    fn p_length_is_rejected_with_a_reason_naming_it() {
        let err = parse_one(
            r#"<Register Name="DynamicLen">
                 <Address>0x10</Address>
                 <pLength>LengthNode</pLength>
               </Register>"#,
        )
        .expect_err("a dynamic length must not be accepted");

        let text = err.to_string();
        assert!(
            text.contains("<pLength>"),
            "the error must name <pLength> so the skip can be attributed: {text}"
        );
        assert!(
            text.contains("LengthNode"),
            "the error should name the node supplying the length: {text}"
        );
    }
}

//! A declaration the library cannot build reaches the UI as skipped, not as
//! silence, so a missing feature reads as dropped rather than absent (DX-05).

use viva_xml_model::parse_genicam_xml;

#[test]
fn a_dropped_declaration_is_listed_on_the_graph() {
    const XML: &str = r#"
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
    let graph = parse_genicam_xml(XML).expect("parse");
    assert!(graph.nodes_by_name.contains_key("Width"));
    assert_eq!(graph.skipped.len(), 1, "{:?}", graph.skipped);
    assert_eq!(graph.skipped[0].tag, "ConfRom");
    assert_eq!(graph.skipped[0].name.as_deref(), Some("DeviceConfRom"));

    // Nothing to say about a document the library fully understands.
    let json = serde_json::to_value(
        parse_genicam_xml(&XML.replace(
            r#"<ConfRom Name="DeviceConfRom">
                <Address>0x2000</Address>
                <Length>512</Length>
            </ConfRom>"#,
            "",
        ))
        .unwrap(),
    )
    .unwrap();
    assert!(json.get("skipped").is_none(), "{json}");
}

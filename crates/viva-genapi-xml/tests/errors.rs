//! Errors carry what caused them as their `source`, never as text
//! (ADR-0030).

use std::error::Error;

/// A document the XML reader itself rejects: the reader's error is the
/// source, and the message still says what went wrong.
#[test]
fn malformed_xml_carries_the_readers_error() {
    let err = viva_genapi_xml::parse(
        r#"<RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="0">
             <Category Name="Root"><pFeature>A</pFeature></Categor>
           </RegisterDescription>"#,
    )
    .expect_err("a mismatched end tag is not XML");
    let source = err.source().expect("the XML reader's error as the source");
    assert!(
        err.to_string().contains(&source.to_string()),
        "{err} does not say what the reader found: {source}"
    );
}

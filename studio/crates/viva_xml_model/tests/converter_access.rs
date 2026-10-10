use viva_xml_model::parse_genicam_xml;

/// A converter declares no `<AccessMode>`; the UI model must report the one it
/// inherits through `<pValue>`, or Studio refuses to write a feature the
/// camera accepts (AVT and Prosilica expose `AcquisitionFrameRateAbs` this
/// way).
#[test]
fn a_converter_reports_the_access_mode_of_its_pvalue() {
    const XML: &str = r#"
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
            <Category Name="Root">
                <pFeature>AcquisitionFrameRateAbs</pFeature>
                <pFeature>Temperature</pFeature>
            </Category>
            <IntReg Name="FrameRateRaw">
                <Address>0x1000</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
            </IntReg>
            <Converter Name="AcquisitionFrameRateAbs">
                <FormulaTo>FROM * 100</FormulaTo>
                <FormulaFrom>TO / 100</FormulaFrom>
                <pValue>FrameRateRaw</pValue>
            </Converter>
            <IntReg Name="TemperatureRaw">
                <Address>0x2000</Address>
                <Length>4</Length>
                <AccessMode>RO</AccessMode>
            </IntReg>
            <IntConverter Name="Temperature">
                <FormulaTo>FROM</FormulaTo>
                <FormulaFrom>TO / 10</FormulaFrom>
                <pValue>TemperatureRaw</pValue>
            </IntConverter>
        </RegisterDescription>
    "#;
    let graph = parse_genicam_xml(XML).expect("parse");

    let frame_rate = &graph.nodes_by_name["AcquisitionFrameRateAbs"];
    assert_eq!(frame_rate.access_mode.as_deref(), Some("RW"));
    // Negative control: over a read-only register it stays read-only.
    let temperature = &graph.nodes_by_name["Temperature"];
    assert_eq!(temperature.access_mode.as_deref(), Some("RO"));
}

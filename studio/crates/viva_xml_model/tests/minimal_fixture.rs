use serde_json::Value;
use viva_xml_model::{UiNodeKind, parse_genicam_xml};

#[test]
fn parse_minimal_fixture() {
    let xml = include_str!("../fixtures/minimal.xml");
    let graph = parse_genicam_xml(xml).expect("parse minimal fixture");

    assert_eq!(graph.root_category, "Root");

    let root_category = graph.categories.get("Root").expect("root category present");
    assert_eq!(
        root_category.features,
        vec![
            "ExposureTime",
            "PixelFormat",
            "AcquisitionEnable",
            "TriggerSoftware",
            "MagicCalc"
        ]
    );

    let exposure = graph
        .nodes_by_name
        .get("ExposureTime")
        .expect("exposure node present");
    assert!(matches!(exposure.kind, UiNodeKind::Integer));
    let constraints = exposure.constraints.as_ref().expect("constraints present");
    assert_eq!(constraints.min, Some(10.0));
    assert_eq!(constraints.max, Some(1000.0));
    assert_eq!(constraints.inc, Some(5.0));

    let pixel_format = graph
        .nodes_by_name
        .get("PixelFormat")
        .expect("enumeration present");
    assert!(matches!(pixel_format.kind, UiNodeKind::Enumeration));
    assert_eq!(pixel_format.enum_entries.len(), 3);
    assert_eq!(pixel_format.enum_entries[0].name, "Mono8");
    assert_eq!(pixel_format.enum_entries[1].name, "Mono12");
    assert_eq!(pixel_format.enum_entries[2].name, "RGB8");

    let unknown = graph
        .nodes_by_name
        .get("MagicCalc")
        .expect("unknown node present");
    assert!(matches!(
        &unknown.kind,
        UiNodeKind::Unknown { tag } if tag == "SwissKnife"
    ));
}

#[test]
fn snapshot_minimal_fixture() {
    let xml = include_str!("../fixtures/minimal.xml");
    let graph = parse_genicam_xml(xml).expect("parse minimal fixture");
    let actual = normalize_json(serde_json::to_value(&graph).expect("json value"));
    let actual_string = serde_json::to_string_pretty(&actual).expect("json string");
    let expected = include_str!("../fixtures/expected_minimal.json").trim();

    assert_eq!(actual_string, expected);
}

fn normalize_json(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<_> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));

            let mut normalized = serde_json::Map::new();
            for (key, value) in entries {
                normalized.insert(key, normalize_json(value));
            }

            Value::Object(normalized)
        }
        Value::Array(values) => Value::Array(values.into_iter().map(normalize_json).collect()),
        other => other,
    }
}

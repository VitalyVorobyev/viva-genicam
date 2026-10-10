use std::collections::HashMap;

use viva_genapi::NodeMap;
use viva_genapi_xml::{AccessMode, EnumValueSrc, NodeDecl, NodeMeta, SkOutput, XmlModel};

use crate::error::ParseError;
use crate::model::*;

/// Build Integer constraints, returning `None` when the XML declared no
/// explicit range (both `min == i64::MIN` AND `max == i64::MAX` — the parser
/// sentinels for "no explicit bound") AND no `inc`. In that case the UI
/// renders "range unknown" instead of showing the `i64::MIN..i64::MAX`
/// bounds — these are parser sentinels, not device-reported ranges.
///
/// When `pMin`/`pMax` references exist, the live-mode [`FeatureState::numeric`]
/// from the backend overrides whatever static hint we produce here, so we do
/// not need special handling for that case — sentinels mean "unknown at
/// parse time" whether or not runtime can resolve them later.
fn integer_constraints(min: i64, max: i64, inc: Option<i64>) -> Option<NumericConstraints> {
    let min_is_sentinel = min == i64::MIN;
    let max_is_sentinel = max == i64::MAX;
    if min_is_sentinel && max_is_sentinel && inc.is_none() {
        return None;
    }
    Some(NumericConstraints {
        min: if min_is_sentinel {
            None
        } else {
            Some(min as f64)
        },
        max: if max_is_sentinel {
            None
        } else {
            Some(max as f64)
        },
        inc: inc.map(|i| i as f64),
        value: None,
    })
}

/// Same idea as [`integer_constraints`] but for Float nodes. Float sentinels
/// are `f64::MIN`/`f64::MAX`.
fn float_constraints(min: f64, max: f64) -> Option<NumericConstraints> {
    let min_is_sentinel = min == f64::MIN;
    let max_is_sentinel = max == f64::MAX;
    if min_is_sentinel && max_is_sentinel {
        return None;
    }
    Some(NumericConstraints {
        min: if min_is_sentinel { None } else { Some(min) },
        max: if max_is_sentinel { None } else { Some(max) },
        inc: None,
        value: None,
    })
}

/// Parse GenICam XML into a [`UiGraph`] using the full genapi pipeline.
///
/// Produces a UiGraph with resolved dependencies, precise integer constraints,
/// expression strings, and full node metadata (visibility, display name, etc.).
///
/// If the full NodeMap validation fails (e.g. unsupported expressions), falls
/// back to building the graph directly from the parsed XML model without
/// dependency tracking.
pub fn parse_genicam_xml(xml: &str) -> Result<UiGraph, ParseError> {
    let xml_model: XmlModel =
        viva_genapi_xml::parse(xml).map_err(|e| ParseError::Xml(e.to_string()))?;

    match NodeMap::try_from_xml(xml_model.clone()) {
        Ok(nodemap) => build_from_nodemap(&xml_model, &nodemap),
        Err(_) => build_from_xml_model(&xml_model),
    }
}

/// Build UiGraph using the full NodeMap (dependencies, dependents resolved).
fn build_from_nodemap(xml_model: &XmlModel, nodemap: &NodeMap) -> Result<UiGraph, ParseError> {
    let mut nodes_by_name = HashMap::new();
    let mut categories = HashMap::new();
    let mut root_category = String::from("Root");

    // Build categories from NodeMap
    for (cat_name, children) in nodemap.categories() {
        let cat_meta = nodemap.node(cat_name).map(|n| n.meta());
        categories.insert(
            cat_name.to_string(),
            UiCategory {
                name: cat_name.to_string(),
                display_name: cat_meta
                    .and_then(|m| m.display_name.clone())
                    .unwrap_or_else(|| cat_name.to_string()),
                features: children.to_vec(),
                tooltip: cat_meta.and_then(|m| m.tooltip.clone()),
                comment: cat_meta.and_then(|m| m.description.clone()),
            },
        );
        if cat_name == "Root" {
            root_category = "Root".to_string();
        }
    }

    // If no "Root" category found, use the first one
    if !categories.contains_key("Root")
        && let Some(first) = nodemap.categories().first()
    {
        root_category = first.0.to_string();
    }

    // Map each node from NodeDecl to UiNode
    for decl in &xml_model.nodes {
        let mut ui_node = node_decl_to_ui_node(decl, Some(nodemap));
        // The access the whole `<pValue>` chain allows, ceilings included, as
        // the runtime enforces it; the node's own is only the first link
        // (Codex review of #186).
        // `None` from a node that has an access mode is a chain that permits
        // neither reading nor writing: `NA`, as the live feature state says,
        // not the first node's own mode (Codex review of #187).
        if ui_node.access_mode.is_some() {
            ui_node.access_mode = Some(
                nodemap
                    .declared_access_mode(&ui_node.name)
                    .map_or_else(|| "NA".to_string(), access_mode_str),
            );
        }
        nodes_by_name.insert(ui_node.name.clone(), ui_node);
    }

    Ok(UiGraph {
        nodes_by_name,
        categories,
        root_category,
        skipped: nodemap.skipped().iter().map(UiSkippedNode::from).collect(),
    })
}

/// Fallback: build UiGraph directly from XmlModel without NodeMap.
///
/// Dependency tracking and expression evaluation are not available, but
/// node metadata and basic structure are preserved.
fn build_from_xml_model(xml_model: &XmlModel) -> Result<UiGraph, ParseError> {
    let mut nodes_by_name = HashMap::new();
    let mut categories = HashMap::new();
    let mut root_category = String::from("Root");

    for decl in &xml_model.nodes {
        let name = decl_name(decl);
        let ui_node = node_decl_to_ui_node(decl, None);

        if let NodeDecl::Category {
            name: cat_name,
            meta,
            children,
            ..
        } = decl
        {
            categories.insert(
                cat_name.clone(),
                UiCategory {
                    name: cat_name.clone(),
                    display_name: meta
                        .display_name
                        .clone()
                        .unwrap_or_else(|| cat_name.clone()),
                    features: children.clone(),
                    tooltip: meta.tooltip.clone(),
                    comment: meta.description.clone(),
                },
            );
            if cat_name == "Root" {
                root_category = "Root".to_string();
            }
        }

        nodes_by_name.insert(name, ui_node);
    }

    if !categories.contains_key("Root")
        && let Some(first_cat) = categories.keys().next()
    {
        root_category = first_cat.clone();
    }

    Ok(UiGraph {
        nodes_by_name,
        categories,
        root_category,
        skipped: xml_model.skipped.iter().map(UiSkippedNode::from).collect(),
    })
}

fn access_mode_str(am: AccessMode) -> String {
    match am {
        AccessMode::RO => "RO".to_string(),
        AccessMode::WO => "WO".to_string(),
        AccessMode::RW => "RW".to_string(),
    }
}

fn visibility_str(meta: &NodeMeta) -> Option<String> {
    Some(format!("{:?}", meta.visibility))
}

fn representation_str(meta: &NodeMeta) -> Option<String> {
    meta.representation.map(|r| format!("{r:?}"))
}

/// One `UiNode` per declaration. With the `NodeMap` the library built, a
/// node's dependents and a converter's access come from it; without one — a
/// document the library could not build — they stay empty.
fn node_decl_to_ui_node(decl: &NodeDecl, nodemap: Option<&NodeMap>) -> UiNode {
    let name = decl_name(decl);
    let mut node = match decl {
        NodeDecl::Integer {
            meta,
            access,
            min,
            max,
            inc,
            unit,
            pvalue,
            p_min,
            p_max,
            ..
        } => UiNode {
            access_mode: Some(
                access
                    .capped_by(meta.imposed_access)
                    .map_or_else(|| "NA".to_string(), access_mode_str),
            ),
            unit: unit.clone(),
            constraints: integer_constraints(*min, *max, *inc),
            dependencies: [pvalue, p_min, p_max]
                .into_iter()
                .flatten()
                .cloned()
                .collect(),
            int_min: Some(*min),
            int_max: Some(*max),
            int_inc: *inc,
            ..described(&name, UiNodeKind::Integer, meta)
        },
        NodeDecl::Float {
            meta,
            access,
            min,
            max,
            unit,
            pvalue,
            ..
        } => UiNode {
            access_mode: Some(
                access
                    .capped_by(meta.imposed_access)
                    .map_or_else(|| "NA".to_string(), access_mode_str),
            ),
            unit: unit.clone(),
            constraints: float_constraints(*min, *max),
            dependencies: pvalue.iter().cloned().collect(),
            ..described(&name, UiNodeKind::Float, meta)
        },
        NodeDecl::Enum {
            meta,
            access,
            entries,
            pvalue,
            ..
        } => UiNode {
            access_mode: Some(
                access
                    .capped_by(meta.imposed_access)
                    .map_or_else(|| "NA".to_string(), access_mode_str),
            ),
            enum_entries: entries
                .iter()
                .map(|e| EnumEntry {
                    name: e.name.clone(),
                    value: match &e.value {
                        EnumValueSrc::Literal(v) => Some(v.to_string()),
                        EnumValueSrc::FromNode(_) => None,
                    },
                    display_name: e.display_name.clone(),
                })
                .collect(),
            dependencies: pvalue.iter().cloned().collect(),
            ..described(&name, UiNodeKind::Enumeration, meta)
        },
        NodeDecl::Boolean {
            meta,
            access,
            pvalue,
            ..
        } => UiNode {
            access_mode: Some(
                access
                    .capped_by(meta.imposed_access)
                    .map_or_else(|| "NA".to_string(), access_mode_str),
            ),
            dependencies: pvalue.iter().cloned().collect(),
            ..described(&name, UiNodeKind::Boolean, meta)
        },
        NodeDecl::Command { meta, pvalue, .. } => UiNode {
            access_mode: Some("WO".to_string()),
            representation: None,
            dependencies: pvalue.iter().cloned().collect(),
            ..described(&name, UiNodeKind::Command, meta)
        },
        NodeDecl::Category { meta, children, .. } => UiNode {
            representation: None,
            dependencies: children.clone(),
            ..described(&name, UiNodeKind::Category, meta)
        },
        // A computed, read-only value of its declared output type.
        NodeDecl::SwissKnife(sk) => {
            let kind = match sk.output {
                SkOutput::Integer => UiNodeKind::Integer,
                SkOutput::Float => UiNodeKind::Float,
            };
            UiNode {
                access_mode: Some("RO".to_string()),
                dependencies: sk
                    .variables
                    .iter()
                    .map(|(_, target)| target.clone())
                    .collect(),
                expression: Some(sk.expr.clone()),
                ..described(&name, kind, &sk.meta)
            }
        }
        NodeDecl::Converter(cv) => UiNode {
            access_mode: converter_access(nodemap, &cv.name),
            unit: cv.unit.clone(),
            dependencies: converter_dependencies(&cv.p_value, &cv.variables_to, &cv.variables_from),
            expression: Some(cv.formula_to.clone()),
            ..described(&name, UiNodeKind::Float, &cv.meta)
        },
        NodeDecl::IntConverter(cv) => UiNode {
            access_mode: converter_access(nodemap, &cv.name),
            dependencies: converter_dependencies(&cv.p_value, &cv.variables_to, &cv.variables_from),
            expression: Some(cv.formula_to.clone()),
            ..described(&name, UiNodeKind::Integer, &cv.meta)
        },
        NodeDecl::String(s) => UiNode {
            access_mode: s
                .access
                .capped_by(s.meta.imposed_access)
                .map(access_mode_str),
            ..described(&name, UiNodeKind::String, &s.meta)
        },
        NodeDecl::Register(r) => UiNode {
            access_mode: r
                .access
                .capped_by(r.meta.imposed_access)
                .map(access_mode_str),
            ..described(&name, UiNodeKind::Register, &r.meta)
        },
        // `NodeDecl` is `#[non_exhaustive]`, so a node type added upstream no
        // longer breaks this build. Surface it with its own kind rather than
        // dropping it, so an unmodelled node is visible in the tree instead of
        // silently absent.
        other => bare(
            &name,
            UiNodeKind::Unknown {
                tag: other.kind().to_string(),
            },
        ),
    };
    // A category's members are its dependencies; nothing depends on it.
    if !matches!(decl, NodeDecl::Category { .. }) {
        node.dependents = nodemap
            .map(|map| map.dependents(&name).to_vec())
            .unwrap_or_default();
    }
    node
}

/// A node with only its name and kind.
fn bare(name: &str, kind: UiNodeKind) -> UiNode {
    UiNode {
        name: name.to_string(),
        kind,
        display_name: None,
        comment: None,
        tooltip: None,
        description: None,
        visibility: None,
        access_mode: None,
        unit: None,
        representation: None,
        constraints: None,
        enum_entries: vec![],
        dependencies: vec![],
        dependents: vec![],
        expression: None,
        int_min: None,
        int_max: None,
        int_inc: None,
    }
}

/// A node with its name, kind and the metadata every declaration carries.
fn described(name: &str, kind: UiNodeKind, meta: &NodeMeta) -> UiNode {
    UiNode {
        display_name: meta.display_name.clone(),
        tooltip: meta.tooltip.clone(),
        description: meta.description.clone(),
        visibility: visibility_str(meta),
        representation: representation_str(meta),
        ..bare(name, kind)
    }
}

/// A converter's access is its `<pValue>` chain's, which only the `NodeMap`
/// resolves.
fn converter_access(nodemap: Option<&NodeMap>, name: &str) -> Option<String> {
    nodemap
        .and_then(|map| map.declared_access_mode(name))
        .map(access_mode_str)
}

fn converter_dependencies(
    p_value: &str,
    variables_to: &[(String, String)],
    variables_from: &[(String, String)],
) -> Vec<String> {
    std::iter::once(p_value.to_string())
        .chain(variables_to.iter().map(|(_, target)| target.clone()))
        .chain(variables_from.iter().map(|(_, target)| target.clone()))
        .collect()
}

fn decl_name(decl: &NodeDecl) -> String {
    match decl {
        NodeDecl::Integer { name, .. }
        | NodeDecl::Float { name, .. }
        | NodeDecl::Enum { name, .. }
        | NodeDecl::Boolean { name, .. }
        | NodeDecl::Command { name, .. }
        | NodeDecl::Category { name, .. } => name.clone(),
        NodeDecl::SwissKnife(sk) => sk.name.clone(),
        NodeDecl::Converter(cv) => cv.name.clone(),
        NodeDecl::IntConverter(cv) => cv.name.clone(),
        NodeDecl::String(s) => s.name.clone(),
        NodeDecl::Register(r) => r.name.clone(),
        other => other.name().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="2" SchemaSubMinorVersion="3">
            <Category Name="Root">
                <pFeature>Width</pFeature>
                <pFeature>ExposureTime</pFeature>
                <pFeature>GainSelector</pFeature>
                <pFeature>GammaEnable</pFeature>
                <pFeature>AcquisitionStart</pFeature>
            </Category>
            <Integer Name="Width">
                <Address>0x0000_0100</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Min>16</Min>
                <Max>4096</Max>
                <Inc>2</Inc>
            </Integer>
            <Float Name="ExposureTime">
                <Address>0x0000_0200</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Min>10.0</Min>
                <Max>200000.0</Max>
                <Scale>1/1000</Scale>
                <Offset>0.0</Offset>
            </Float>
            <Enumeration Name="GainSelector">
                <Address>0x0000_0300</Address>
                <Length>2</Length>
                <AccessMode>RW</AccessMode>
                <EnumEntry Name="AnalogAll" Value="0" />
                <EnumEntry Name="DigitalAll" Value="1" />
            </Enumeration>
            <Boolean Name="GammaEnable">
                <Address>0x0000_0400</Address>
                <Length>1</Length>
                <AccessMode>RW</AccessMode>
            </Boolean>
            <Command Name="AcquisitionStart">
                <Address>0x0000_0500</Address>
                <Length>4</Length>
            </Command>
        </RegisterDescription>
    "#;

    #[test]
    fn test_parse_basic_fixture() {
        let graph = parse_genicam_xml(FIXTURE).expect("parse fixture");
        assert_eq!(graph.root_category, "Root");
        assert!(graph.categories.contains_key("Root"));

        let root = &graph.categories["Root"];
        assert_eq!(root.features.len(), 5);

        let width = graph.nodes_by_name.get("Width").expect("Width present");
        assert!(matches!(width.kind, UiNodeKind::Integer));
        assert_eq!(width.int_min, Some(16));
        assert_eq!(width.int_max, Some(4096));
        assert_eq!(width.int_inc, Some(2));
        assert_eq!(width.access_mode.as_deref(), Some("RW"));

        // Metadata populated from NodeMeta
        assert_eq!(width.visibility.as_deref(), Some("Beginner"));

        let constraints = width.constraints.as_ref().expect("constraints");
        assert_eq!(constraints.min, Some(16.0));
        assert_eq!(constraints.max, Some(4096.0));
        assert_eq!(constraints.inc, Some(2.0));
    }

    #[test]
    fn test_parse_enum_entries() {
        let graph = parse_genicam_xml(FIXTURE).expect("parse fixture");

        let gain_sel = graph
            .nodes_by_name
            .get("GainSelector")
            .expect("GainSelector present");
        assert!(matches!(gain_sel.kind, UiNodeKind::Enumeration));
        assert_eq!(gain_sel.enum_entries.len(), 2);
        assert_eq!(gain_sel.enum_entries[0].name, "AnalogAll");
        assert_eq!(gain_sel.enum_entries[0].value.as_deref(), Some("0"));
        assert_eq!(gain_sel.enum_entries[1].name, "DigitalAll");
    }

    #[test]
    fn test_command_access_mode() {
        let graph = parse_genicam_xml(FIXTURE).expect("parse fixture");

        let cmd = graph
            .nodes_by_name
            .get("AcquisitionStart")
            .expect("AcquisitionStart present");
        assert!(matches!(cmd.kind, UiNodeKind::Command));
        assert_eq!(cmd.access_mode.as_deref(), Some("WO"));
    }

    #[test]
    fn test_parse_empty_xml() {
        let xml = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
            </RegisterDescription>
        "#;
        let graph = parse_genicam_xml(xml).expect("parse empty xml");
        assert!(graph.nodes_by_name.is_empty());
        assert!(graph.categories.is_empty());
    }

    /// A SwissKnife is a read-only value of its declared output type. It was
    /// an `Unknown` node, which the category list skips: in the vendor corpus
    /// 55 SwissKnives listed under a category never showed — `WidthMax`,
    /// `HeightMax` and `UserSetCurrent` among them (backlog ST-34).
    /// A feature imposed `RO` shows read-only, whatever its own
    /// `<AccessMode>` says (backlog GA-23).
    #[test]
    fn an_imposed_access_mode_caps_a_features_access() {
        let xml = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="0">
                <IntReg Name="SensorWidth">
                    <ImposedAccessMode>RO</ImposedAccessMode>
                    <Address>0x100</Address><Length>4</Length><AccessMode>RW</AccessMode>
                    <Endianess>BigEndian</Endianess>
                </IntReg>
            </RegisterDescription>
        "#;
        let graph = parse_genicam_xml(xml).expect("parse");
        assert_eq!(
            graph.nodes_by_name["SensorWidth"].access_mode.as_deref(),
            Some("RO")
        );
    }

    /// A command imposed `RO` permits nothing — its own access is `WO` — and
    /// shows as `NA`, not as executable (Codex review of #187).
    #[test]
    fn a_node_left_with_no_access_shows_na() {
        let xml = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="0">
                <Command Name="Reset">
                    <ImposedAccessMode>RO</ImposedAccessMode>
                    <Address>0x104</Address><Length>4</Length>
                    <CommandValue>1</CommandValue>
                </Command>
            </RegisterDescription>
        "#;
        let graph = parse_genicam_xml(xml).expect("parse");
        assert_eq!(
            graph.nodes_by_name["Reset"].access_mode.as_deref(),
            Some("NA")
        );
    }

    /// A feature takes the ceiling of the register it delegates to, as the
    /// runtime does: an `RW` integer over an imposed-`RO` register is `RO`
    /// (Codex review of #186).
    #[test]
    fn a_ceiling_down_the_pvalue_chain_caps_the_feature() {
        let xml = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="1" SchemaSubMinorVersion="0">
                <Integer Name="Width">
                    <pValue>WidthReg</pValue>
                </Integer>
                <IntReg Name="WidthReg">
                    <ImposedAccessMode>RO</ImposedAccessMode>
                    <Address>0x100</Address><Length>4</Length><AccessMode>RW</AccessMode>
                    <Endianess>BigEndian</Endianess>
                </IntReg>
            </RegisterDescription>
        "#;
        let graph = parse_genicam_xml(xml).expect("parse");
        assert_eq!(
            graph.nodes_by_name["Width"].access_mode.as_deref(),
            Some("RO")
        );
    }

    #[test]
    fn test_swissknife_is_a_read_only_value_of_its_output_type() {
        let xml = r#"
            <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
                <Integer Name="GainRaw">
                    <Address>0x3000</Address>
                    <Length>4</Length>
                    <AccessMode>RW</AccessMode>
                    <Min>0</Min>
                    <Max>1000</Max>
                </Integer>
                <SwissKnife Name="ComputedGain">
                    <Expression>(GainRaw * 0.5)</Expression>
                    <pVariable Name="GainRaw">GainRaw</pVariable>
                    <Output>Float</Output>
                </SwissKnife>
                <IntSwissKnife Name="WidthMax">
                    <Expression>GainRaw - 16</Expression>
                    <pVariable Name="GainRaw">GainRaw</pVariable>
                </IntSwissKnife>
            </RegisterDescription>
        "#;
        let graph = parse_genicam_xml(xml).expect("parse swissknife");

        let sk = &graph.nodes_by_name["ComputedGain"];
        assert_eq!(sk.kind, UiNodeKind::Float);
        assert_eq!(sk.access_mode.as_deref(), Some("RO"));
        assert_eq!(sk.expression.as_deref(), Some("(GainRaw * 0.5)"));
        assert_eq!(sk.dependencies, vec!["GainRaw"]);

        let int_sk = &graph.nodes_by_name["WidthMax"];
        assert_eq!(int_sk.kind, UiNodeKind::Integer);
        assert_eq!(int_sk.access_mode.as_deref(), Some("RO"));
    }

    #[test]
    fn test_access_mode_str_all_variants() {
        assert_eq!(access_mode_str(AccessMode::RO), "RO");
        assert_eq!(access_mode_str(AccessMode::WO), "WO");
        assert_eq!(access_mode_str(AccessMode::RW), "RW");
    }

    #[test]
    fn test_decl_name_all_variants() {
        let int_decl = NodeDecl::Integer {
            name: "W".to_string(),
            meta: NodeMeta::default(),
            addressing: None,
            len: 4,
            access: AccessMode::RW,
            min: 0,
            max: 100,
            inc: None,
            unit: None,
            bitfield: None,
            sign: Default::default(),
            byte_order: viva_genapi_xml::ByteOrder::Big,
            selectors: vec![],
            selected_if: vec![],
            pvalue: None,
            p_max: None,
            p_min: None,
            value: None,
            predicates: Default::default(),
            invalidators: vec![],
        };
        assert_eq!(decl_name(&int_decl), "W");

        let cat_decl = NodeDecl::Category {
            name: "Root".to_string(),
            meta: NodeMeta::default(),
            children: vec![],
            predicates: Default::default(),
        };
        assert_eq!(decl_name(&cat_decl), "Root");
    }

    // ── Integer / Float sentinel fallback ──────────────────────────────────

    #[test]
    fn integer_constraints_returns_none_when_both_bounds_are_sentinels() {
        // No explicit <Min>/<Max> → "range unknown". Live mode overrides
        // via FeatureState.numeric.
        let c = integer_constraints(i64::MIN, i64::MAX, None);
        assert!(
            c.is_none(),
            "sentinel-only integer must not leak i64::MIN/MAX: {c:?}"
        );
    }

    #[test]
    fn integer_constraints_preserves_explicit_min() {
        let c = integer_constraints(1, i64::MAX, None);
        let c = c.expect("explicit min should produce Some constraints");
        assert_eq!(c.min, Some(1.0));
        assert!(c.max.is_none(), "sentinel max should not leak");
    }

    #[test]
    fn integer_constraints_returns_some_when_inc_present_even_if_bounds_sentinels() {
        // Inc alone is still useful information for the UI (step size).
        let c = integer_constraints(i64::MIN, i64::MAX, Some(8));
        let c = c.expect("inc alone should still produce constraints");
        assert_eq!(c.inc, Some(8.0));
        assert!(c.min.is_none());
        assert!(c.max.is_none());
    }

    #[test]
    fn float_constraints_returns_none_for_f64_sentinels() {
        let c = float_constraints(f64::MIN, f64::MAX);
        assert!(c.is_none(), "sentinel-only float must not leak: {c:?}");
    }

    #[test]
    fn float_constraints_preserves_explicit_bounds() {
        let c = float_constraints(0.0, 100.0);
        let c = c.expect("explicit bounds should produce Some constraints");
        assert_eq!(c.min, Some(0.0));
        assert_eq!(c.max, Some(100.0));
    }
}

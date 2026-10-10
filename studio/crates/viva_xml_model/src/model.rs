use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct UiGraph {
    pub nodes_by_name: HashMap<String, UiNode>,
    pub categories: HashMap<String, UiCategory>,
    pub root_category: String,
    /// Declarations in the XML that could not become features, so a viewer
    /// can say a feature was dropped rather than absent (backlog DX-05).
    /// Omitted when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<UiSkippedNode>,
}

/// A declaration the library could not turn into a feature.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
pub struct UiSkippedNode {
    /// The XML element, such as `Register`.
    pub tag: String,
    /// The node's `Name`, if it had one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Why it could not be built.
    pub error: String,
}

impl From<&viva_genapi_xml::SkippedNode> for UiSkippedNode {
    fn from(node: &viva_genapi_xml::SkippedNode) -> Self {
        Self {
            tag: node.tag.clone(),
            name: node.name.clone(),
            error: node.error.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
pub struct UiNode {
    pub name: String,
    pub kind: UiNodeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tooltip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub representation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub constraints: Option<NumericConstraints>,
    // Omitted when empty; `nullable` adds the `?` without unwrapping a type that
    // is not an `Option`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub enum_entries: Vec<EnumEntry>,
    /// Nodes that this node depends on (pValue, pMin, pMax references).
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub dependencies: Vec<String>,
    /// Nodes that are invalidated when this node changes.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub dependents: Vec<String>,
    /// SwissKnife/Converter expression string (for display, not evaluation).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expression: Option<String>,
    /// For Integer nodes: static integer minimum from XML.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub int_min: Option<i64>,
    /// For Integer nodes: static integer maximum from XML.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub int_max: Option<i64>,
    /// For Integer nodes: static integer increment from XML.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub int_inc: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum UiNodeKind {
    Category,
    Integer,
    Float,
    Boolean,
    String,
    Enumeration,
    Command,
    Register,
    Unknown { tag: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
pub struct UiCategory {
    pub name: String,
    pub display_name: String,
    pub features: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tooltip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
pub struct NumericConstraints {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inc: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
pub struct EnumEntry {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

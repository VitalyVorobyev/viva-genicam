use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct UiGraph {
    pub nodes_by_name: HashMap<String, UiNode>,
    pub categories: HashMap<String, UiCategory>,
    pub root_category: String,
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
    pub raw: RawNode,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RawNode {
    pub tag: String,
    pub attributes: HashMap<String, String>,
    pub children_text: HashMap<String, String>,
}

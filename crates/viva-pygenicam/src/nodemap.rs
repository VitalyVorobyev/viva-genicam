//! NodeMap introspection helpers.
//!
//! Everything here is read with the GIL released — a node's effective access
//! reads the camera — into plain Rust values, which become Python objects
//! only once the reads are done.

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use viva_genapi::{CachingMode, FeatureState, FeatureValue, Node, NodeMap};
use viva_genapi_xml::{AccessMode, Visibility};

pub(crate) fn access_str(a: Option<AccessMode>) -> Option<&'static str> {
    a.map(|a| match a {
        AccessMode::RO => "RO",
        AccessMode::RW => "RW",
        AccessMode::WO => "WO",
    })
}

fn visibility_str(v: Visibility) -> &'static str {
    match v {
        Visibility::Beginner => "Beginner",
        Visibility::Expert => "Expert",
        Visibility::Guru => "Guru",
        Visibility::Invisible => "Invisible",
        _ => "Unknown",
    }
}

fn caching_str(mode: CachingMode) -> &'static str {
    match mode {
        CachingMode::NoCache => "NoCache",
        CachingMode::WriteThrough => "WriteThrough",
        CachingMode::WriteAround => "WriteAround",
    }
}

/// What `node_info` reports about one node.
pub(crate) struct NodeInfo {
    name: String,
    kind: &'static str,
    access: Option<&'static str>,
    visibility: &'static str,
    display_name: Option<String>,
    description: Option<String>,
    tooltip: Option<String>,
    /// `None` both when not asked for and when it could not be resolved.
    effective_access: Option<&'static str>,
    resolved: bool,
}

impl NodeInfo {
    fn of(name: &str, node: &Node, nodemap: &NodeMap) -> Self {
        Self {
            name: name.to_string(),
            kind: node.kind_name(),
            // Declared through the nodemap: a converter takes its `<pValue>`'s mode.
            access: access_str(nodemap.declared_access_mode(name)),
            visibility: visibility_str(node.visibility()),
            display_name: node.display_name().map(str::to_string),
            description: node.description().map(str::to_string),
            tooltip: node.tooltip().map(str::to_string),
            effective_access: None,
            resolved: false,
        }
    }

    pub(crate) fn into_dict(self, py: Python<'_>) -> PyResult<Bound<'_, PyDict>> {
        let dict = PyDict::new(py);
        dict.set_item("name", self.name)?;
        dict.set_item("kind", self.kind)?;
        dict.set_item("access", self.access)?;
        dict.set_item("visibility", self.visibility)?;
        dict.set_item("display_name", self.display_name)?;
        dict.set_item("description", self.description)?;
        dict.set_item("tooltip", self.tooltip)?;
        if self.resolved {
            dict.set_item("effective_access", self.effective_access)?;
        }
        Ok(dict)
    }
}

/// One node's information with the device's *current* access mode.
///
/// `access` is the value declared in the XML. That is genuinely useful — it is
/// what an offline browser over `NullIo` can report — but it is not what the
/// device will allow right now: a node with no `<AccessMode>` defaults to `RW`
/// and may still be locked by `pIsLocked`. Reporting only the static value told
/// #45's reporter their `ExposureTime` was writable while the camera was
/// refusing every write to it.
///
/// Only the single-node lookup gets this. `all_node_info()` would need one
/// predicate evaluation — and so at least one register read — per node, which
/// on a 2 500-node description is not something an introspection call should do
/// silently.
///
/// A predicate that cannot be evaluated (no device, unreadable register) leaves
/// the field `None` rather than failing the call: introspection should degrade,
/// not raise.
pub(crate) fn node_info_with_state(
    name: &str,
    nodemap: &NodeMap,
    io: &dyn viva_genapi::RegisterIo,
) -> Option<NodeInfo> {
    let node = nodemap.node(name)?;
    let mut info = NodeInfo::of(name, node, nodemap);
    info.effective_access = nodemap
        .effective_access_mode(name, io)
        .ok()
        .and_then(|a| access_str(Some(a)));
    info.resolved = true;
    Some(info)
}

pub(crate) fn collect_node_names(nodemap: &NodeMap) -> Vec<String> {
    nodemap.node_names().map(|s| s.to_string()).collect()
}

pub(crate) fn collect_node_info(nodemap: &NodeMap) -> Vec<NodeInfo> {
    nodemap
        .node_names()
        .filter_map(|name| {
            nodemap
                .node(name)
                .map(|node| NodeInfo::of(name, node, nodemap))
        })
        .collect()
}

pub(crate) fn node_info_list(py: Python<'_>, infos: Vec<NodeInfo>) -> PyResult<Bound<'_, PyList>> {
    let list = PyList::empty(py);
    for info in infos {
        list.append(info.into_dict(py)?)?;
    }
    Ok(list)
}

pub(crate) fn collect_categories(nodemap: &NodeMap) -> Vec<(String, Vec<String>)> {
    nodemap
        .categories()
        .into_iter()
        .map(|(cat, children)| (cat.to_string(), children.to_vec()))
        .collect()
}

/// The declarations this camera has that the library could not turn into a
/// feature, as `(tag, name, error)` (backlog DX-05).
pub(crate) fn collect_skipped(nodemap: &NodeMap) -> Vec<(String, Option<String>, String)> {
    nodemap
        .skipped()
        .iter()
        .map(|node| (node.tag.clone(), node.name.clone(), node.error.clone()))
        .collect()
}

pub(crate) fn skipped_list(
    py: Python<'_>,
    skipped: Vec<(String, Option<String>, String)>,
) -> PyResult<Bound<'_, PyList>> {
    let list = PyList::empty(py);
    for (tag, name, error) in skipped {
        let dict = PyDict::new(py);
        dict.set_item("tag", tag)?;
        dict.set_item("name", name)?;
        dict.set_item("error", error)?;
        list.append(dict)?;
    }
    Ok(list)
}

/// A feature's state as a dict, for `FeatureState.from_dict`.
pub(crate) fn feature_state_dict(
    py: Python<'_>,
    state: FeatureState,
) -> PyResult<Bound<'_, PyDict>> {
    let dict = PyDict::new(py);
    dict.set_item("kind", state.kind)?;
    let value = match state.value {
        FeatureValue::Integer(v) => v.into_pyobject(py)?.into_any(),
        FeatureValue::Float(v) => v.into_pyobject(py)?.into_any(),
        FeatureValue::Boolean(v) => pyo3::types::PyBool::new(py, v).to_owned().into_any(),
        FeatureValue::Enum(v) | FeatureValue::String(v) => v.into_pyobject(py)?.into_any(),
        // A register is reported by its length, not its bytes; and nothing
        // else has a value to show.
        _ => py.None().into_bound(py),
    };
    dict.set_item("value", value)?;
    dict.set_item("access", access_str(state.access))?;
    dict.set_item("is_implemented", state.is_implemented)?;
    dict.set_item("is_available", state.is_available)?;
    dict.set_item(
        "range",
        state.range.map(|range| (range.min, range.max, range.inc)),
    )?;
    dict.set_item("enum_entries", state.enum_entries)?;
    dict.set_item("unit", state.unit)?;
    dict.set_item(
        "polling_time",
        state.polling_time.map(|interval| interval.as_secs_f64()),
    )?;
    dict.set_item("caching", caching_str(state.caching))?;
    Ok(dict)
}

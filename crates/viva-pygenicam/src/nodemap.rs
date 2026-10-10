//! NodeMap introspection helpers.

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use viva_genapi::{Node, NodeMap};
use viva_genapi_xml::{AccessMode, Visibility};

fn access_str(a: Option<AccessMode>) -> Option<&'static str> {
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

pub(crate) fn to_node_info<'py>(
    py: Python<'py>,
    name: &str,
    node: &Node,
    nodemap: &NodeMap,
) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    dict.set_item("name", name)?;
    dict.set_item("kind", node.kind_name())?;
    // Declared through the nodemap: a converter takes its `<pValue>`'s mode.
    dict.set_item("access", access_str(nodemap.declared_access_mode(name)))?;
    dict.set_item("visibility", visibility_str(node.visibility()))?;
    dict.set_item("display_name", node.display_name())?;
    dict.set_item("description", node.description())?;
    dict.set_item("tooltip", node.tooltip())?;
    Ok(dict)
}

/// Add the device's *current* access mode to a node-info dict.
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
fn add_effective_access(
    dict: &Bound<'_, PyDict>,
    nodemap: &NodeMap,
    name: &str,
    io: &dyn viva_genapi::RegisterIo,
) -> PyResult<()> {
    let effective = nodemap.effective_access_mode(name, io).ok();
    dict.set_item(
        "effective_access",
        effective.and_then(|a| access_str(Some(a))),
    )
}

pub(crate) fn node_info_with_state<'py>(
    py: Python<'py>,
    name: &str,
    node: &Node,
    nodemap: &NodeMap,
    io: &dyn viva_genapi::RegisterIo,
) -> PyResult<Bound<'py, PyDict>> {
    let dict = to_node_info(py, name, node, nodemap)?;
    add_effective_access(&dict, nodemap, name, io)?;
    Ok(dict)
}

pub(crate) fn collect_node_names(nodemap: &NodeMap) -> Vec<String> {
    nodemap.node_names().map(|s| s.to_string()).collect()
}

pub(crate) fn collect_node_info<'py>(
    py: Python<'py>,
    nodemap: &NodeMap,
) -> PyResult<Bound<'py, PyList>> {
    let list = PyList::empty(py);
    for name in nodemap.node_names() {
        if let Some(node) = nodemap.node(name) {
            list.append(to_node_info(py, name, node, nodemap)?)?;
        }
    }
    Ok(list)
}

pub(crate) fn collect_categories<'py>(
    py: Python<'py>,
    nodemap: &NodeMap,
) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    for (cat, children) in nodemap.categories() {
        let list = PyList::new(py, children.iter().map(|s| s.as_str()))?;
        dict.set_item(cat, list)?;
    }
    Ok(dict)
}

pub(crate) fn register(_m: &Bound<'_, PyModule>) -> PyResult<()> {
    Ok(())
}

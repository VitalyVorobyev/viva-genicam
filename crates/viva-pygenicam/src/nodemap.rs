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
) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new_bound(py);
    dict.set_item("name", name)?;
    dict.set_item("kind", node.kind_name())?;
    dict.set_item("access", access_str(node.access_mode()))?;
    dict.set_item("visibility", visibility_str(node.visibility()))?;
    dict.set_item("display_name", node.display_name())?;
    dict.set_item("description", node.description())?;
    dict.set_item("tooltip", node.tooltip())?;
    Ok(dict)
}

pub(crate) fn collect_node_names(nodemap: &NodeMap) -> Vec<String> {
    nodemap.node_names().map(|s| s.to_string()).collect()
}

pub(crate) fn collect_node_info<'py>(py: Python<'py>, nodemap: &NodeMap) -> PyResult<Bound<'py, PyList>> {
    let list = PyList::empty_bound(py);
    for name in nodemap.node_names() {
        if let Some(node) = nodemap.node(name) {
            list.append(to_node_info(py, name, node)?)?;
        }
    }
    Ok(list)
}

pub(crate) fn collect_categories<'py>(py: Python<'py>, nodemap: &NodeMap) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new_bound(py);
    for (cat, children) in nodemap.categories() {
        let list = PyList::new_bound(py, children.iter().map(|s| s.as_str()));
        dict.set_item(cat, list)?;
    }
    Ok(dict)
}

pub(crate) fn register(_m: &Bound<'_, PyModule>) -> PyResult<()> {
    Ok(())
}

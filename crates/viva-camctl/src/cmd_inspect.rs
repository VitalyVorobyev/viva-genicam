//! `features`, `describe` and `dump`: what a field engineer reads off a camera
//! before changing anything. Each works on either transport and opens a GigE
//! Vision camera as a read-only monitor.

use std::collections::HashSet;
use std::io::Write;

use anyhow::{Context, Result, anyhow};
use serde::Serialize;

use viva_genicam::Camera;
use viva_genicam::genapi::{AccessMode, FeatureValue, Node, RegisterIo, Visibility};
use viva_gige::nic::IfaceSelector;

use crate::common::{self, Target};
use crate::with_camera;

/// One node of the category tree, as `features` prints it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TreeNode {
    /// Node name.
    pub name: String,
    /// GenICam node type: `Category`, `Integer`, `Enumeration`, ...
    pub kind: &'static str,
    /// What the feature permits now — `RO`, `RW`, `WO` — or why it permits
    /// nothing: `not implemented`, `not available`. `None` for a category.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access: Option<String>,
    /// The current value, for a feature that has one to read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// A category's features and subcategories.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<TreeNode>,
}

/// The category tree from `Root` (GenICam Standard v2.1.1 §2.8.2), each
/// feature with its kind, access and value.
///
/// `filter` keeps the features whose name contains it, ignoring case, and
/// the categories on the way to them. Features the XML marks `Invisible` are
/// left out unless `all`.
pub fn feature_tree<T: RegisterIo>(
    camera: &Camera<T>,
    filter: Option<&str>,
    all: bool,
) -> Vec<TreeNode> {
    let nodemap = camera.nodemap();
    let filter = filter.map(str::to_lowercase);
    let roots: Vec<String> = if nodemap.node("Root").is_some() {
        vec!["Root".to_string()]
    } else {
        // No `Root`: the categories no other category lists.
        let listed: HashSet<&str> = nodemap
            .categories()
            .into_iter()
            .flat_map(|(_, children)| children.iter().map(String::as_str))
            .collect();
        nodemap
            .categories()
            .into_iter()
            .map(|(name, _)| name)
            .filter(|name| !listed.contains(name))
            .map(str::to_string)
            .collect()
    };
    let mut seen = HashSet::new();
    roots
        .iter()
        .filter_map(|root| tree_node(camera, root, filter.as_deref(), all, &mut seen))
        .collect()
}

fn tree_node<T: RegisterIo>(
    camera: &Camera<T>,
    name: &str,
    filter: Option<&str>,
    all: bool,
    seen: &mut HashSet<String>,
) -> Option<TreeNode> {
    let node = camera.nodemap().node(name)?;
    if !all && node.visibility() == Visibility::Invisible {
        return None;
    }
    if let Node::Category(category) = node {
        // A category listed twice, or in a cycle, is shown once.
        if !seen.insert(name.to_string()) {
            return None;
        }
        let children: Vec<TreeNode> = category
            .children
            .iter()
            .filter_map(|child| tree_node(camera, child, filter, all, seen))
            .collect();
        if filter.is_some() && children.is_empty() {
            return None;
        }
        return Some(TreeNode {
            name: name.to_string(),
            kind: node.kind_name(),
            access: None,
            value: None,
            children,
        });
    }
    if let Some(filter) = filter
        && !name.to_lowercase().contains(filter)
    {
        return None;
    }
    let (access, value) = match camera.feature_state(name) {
        Ok(state) => {
            let access = if !state.is_implemented {
                "not implemented".to_string()
            } else if !state.is_available {
                "not available".to_string()
            } else {
                access_text(state.access).to_string()
            };
            (Some(access), value_text(&state.value))
        }
        Err(err) => (Some(format!("error: {err}")), None),
    };
    Some(TreeNode {
        name: name.to_string(),
        kind: node.kind_name(),
        access,
        value,
        children: Vec::new(),
    })
}

fn access_text(access: Option<AccessMode>) -> &'static str {
    match access {
        Some(AccessMode::RO) => "RO",
        Some(AccessMode::RW) => "RW",
        Some(AccessMode::WO) => "WO",
        None => "no access",
    }
}

/// A feature value as text; `None` for a feature with nothing to read.
fn value_text(value: &FeatureValue) -> Option<String> {
    match value {
        FeatureValue::Integer(v) => Some(v.to_string()),
        FeatureValue::Float(v) => Some(v.to_string()),
        FeatureValue::Boolean(v) => Some(v.to_string()),
        FeatureValue::Enum(v) | FeatureValue::String(v) => Some(v.clone()),
        FeatureValue::Register { len } => Some(format!("<{len} bytes>")),
        _ => None,
    }
}

/// Print the tree, two spaces of indent per level:
///
/// ```text
/// ImageFormatControl
///   Integer       Width = 640  (RW)
/// ```
pub fn render_tree(nodes: &[TreeNode], out: &mut dyn Write) -> std::io::Result<()> {
    fn walk(node: &TreeNode, depth: usize, out: &mut dyn Write) -> std::io::Result<()> {
        let indent = "  ".repeat(depth);
        if node.kind == "Category" {
            writeln!(out, "{indent}{}", node.name)?;
            for child in &node.children {
                walk(child, depth + 1, out)?;
            }
            return Ok(());
        }
        write!(out, "{indent}{:<13} {}", node.kind, node.name)?;
        if let Some(value) = &node.value {
            write!(out, " = {value}")?;
        }
        if let Some(access) = &node.access {
            write!(out, "  ({access})")?;
        }
        writeln!(out)
    }
    for node in nodes {
        walk(node, 0, out)?;
    }
    Ok(())
}

/// Everything `describe` says about one feature.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Description {
    pub name: String,
    pub kind: &'static str,
    pub display_name: Option<String>,
    pub tooltip: Option<String>,
    pub description: Option<String>,
    pub visibility: String,
    /// What the feature permits now.
    pub access: Option<String>,
    /// What its XML declares, along its `<pValue>` chain.
    pub declared_access: Option<String>,
    pub implemented: bool,
    pub available: bool,
    pub value: Option<String>,
    pub unit: Option<String>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub inc: Option<f64>,
    /// An enumeration's entries, as declared.
    pub entries: Vec<String>,
    /// Those of them available now.
    pub available_entries: Vec<String>,
    pub representation: Option<String>,
    pub streamable: bool,
    /// The features this one selects (`<pSelected>`).
    pub selects: Vec<String>,
    /// The selectors that select this one.
    pub selected_by: Vec<String>,
    /// The nodes this one is computed or gated from: its `<pValue>`, its
    /// bounds' providers, its predicates, its address's terms.
    pub depends_on: Vec<String>,
}

/// Describe `name`: its documentation, access, value, bounds, entries,
/// selectors, and the nodes it depends on.
pub fn describe<T: RegisterIo>(camera: &Camera<T>, name: &str) -> Result<Description> {
    let nodemap = camera.nodemap();
    let node = nodemap
        .node(name)
        .ok_or_else(|| anyhow!("the camera has no feature named {name}"))?;
    let state = camera.feature_state(name).ok();
    let meta = node.meta();
    let entries = match node {
        Node::Enum(_) => nodemap.enum_entries(name).unwrap_or_default(),
        _ => Vec::new(),
    };
    let depends_on: Vec<String> = nodemap
        .node_names()
        .filter(|provider| nodemap.dependents(provider).iter().any(|d| d == name))
        .map(str::to_string)
        .collect();
    let range = state.as_ref().and_then(|s| s.range);
    Ok(Description {
        name: name.to_string(),
        kind: node.kind_name(),
        display_name: meta.display_name.clone(),
        tooltip: meta.tooltip.clone(),
        description: meta.description.clone(),
        visibility: format!("{:?}", meta.visibility),
        access: state.as_ref().map(|s| access_text(s.access).to_string()),
        declared_access: nodemap
            .declared_access_mode(name)
            .map(|mode| access_text(Some(mode)).to_string()),
        implemented: state.as_ref().is_some_and(|s| s.is_implemented),
        available: state.as_ref().is_some_and(|s| s.is_available),
        value: state.as_ref().and_then(|s| value_text(&s.value)),
        unit: node.unit().map(str::to_string),
        min: range.map(|r| r.min),
        max: range.map(|r| r.max),
        inc: range.and_then(|r| r.inc),
        available_entries: state
            .as_ref()
            .and_then(|s| s.enum_entries.clone())
            .unwrap_or_default(),
        entries,
        representation: meta.representation.map(|r| format!("{r:?}")),
        streamable: meta.streamable,
        selects: node.selected().to_vec(),
        selected_by: nodemap
            .selectors_of(name)
            .into_iter()
            .map(str::to_string)
            .collect(),
        depends_on,
    })
}

/// Print a description, one labelled line per fact that is present.
pub fn render_description(d: &Description, out: &mut dyn Write) -> std::io::Result<()> {
    writeln!(out, "{} ({})", d.name, d.kind)?;
    let mut line = |label: &str, value: String| writeln!(out, "  {label:<15} {value}");
    let list = |items: &[String]| items.join(", ");
    if let Some(v) = &d.display_name {
        line("Display name:", v.clone())?;
    }
    if let Some(v) = &d.tooltip {
        line("Tooltip:", v.clone())?;
    }
    if let Some(v) = &d.description {
        line("Description:", v.clone())?;
    }
    line("Visibility:", d.visibility.clone())?;
    line(
        "Access:",
        format!(
            "{} now, {} declared",
            d.access.as_deref().unwrap_or("none"),
            d.declared_access.as_deref().unwrap_or("none")
        ),
    )?;
    line(
        "State:",
        format!(
            "{}, {}",
            if d.implemented {
                "implemented"
            } else {
                "not implemented"
            },
            if d.available {
                "available"
            } else {
                "not available"
            }
        ),
    )?;
    if let Some(v) = &d.value {
        line("Value:", v.clone())?;
    }
    if let Some(v) = &d.unit {
        line("Unit:", v.clone())?;
    }
    if let (Some(min), Some(max)) = (d.min, d.max) {
        let inc = d
            .inc
            .map(|i| format!(", increment {i}"))
            .unwrap_or_default();
        line("Range:", format!("{min} to {max}{inc}"))?;
    }
    if !d.entries.is_empty() {
        line("Entries:", list(&d.entries))?;
        line("Available now:", list(&d.available_entries))?;
    }
    if let Some(v) = &d.representation {
        line("Representation:", v.clone())?;
    }
    line(
        "Streamable:",
        if d.streamable { "yes" } else { "no" }.into(),
    )?;
    if !d.selects.is_empty() {
        line("Selects:", list(&d.selects))?;
    }
    if !d.selected_by.is_empty() {
        line("Selected by:", list(&d.selected_by))?;
    }
    if !d.depends_on.is_empty() {
        line("Depends on:", list(&d.depends_on))?;
    }
    Ok(())
}

/// One readable value, as `dump` prints it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DumpedValue {
    pub name: String,
    pub value: String,
}

/// Every node with a value to read now, in document order: features and
/// the registers and formulas behind them alike. Categories, commands and
/// the registers commands write through, write-only nodes, and nodes not
/// implemented or not available now are left out; a register shows its
/// size, not its bytes.
pub fn dump<T: RegisterIo>(camera: &Camera<T>) -> Vec<DumpedValue> {
    let nodemap = camera.nodemap();
    nodemap
        .node_names()
        .filter(|name| {
            !matches!(
                nodemap.node(name),
                Some(Node::Category(_) | Node::Command(_))
            )
        })
        .filter_map(|name| {
            let state = camera.feature_state(name).ok()?;
            Some(DumpedValue {
                name: name.to_string(),
                value: value_text(&state.value)?,
            })
        })
        .collect()
}

pub async fn run_features(
    target: Target,
    iface: Option<IfaceSelector>,
    filter: Option<String>,
    all: bool,
    json: bool,
) -> Result<()> {
    let camera = common::open_target(&target, iface.as_ref(), viva_genicam::Access::Monitor)
        .await
        .context("open camera for features")?;
    let tree = with_camera!(&camera, camera => feature_tree(camera, filter.as_deref(), all));
    if json {
        return common::print_json(&tree);
    }
    render_tree(&tree, &mut std::io::stdout().lock())?;
    Ok(())
}

pub async fn run_describe(
    target: Target,
    iface: Option<IfaceSelector>,
    name: String,
    json: bool,
) -> Result<()> {
    let camera = common::open_target(&target, iface.as_ref(), viva_genicam::Access::Monitor)
        .await
        .context("open camera for describe")?;
    let description = with_camera!(&camera, camera => describe(camera, &name))?;
    if json {
        return common::print_json(&description);
    }
    render_description(&description, &mut std::io::stdout().lock())?;
    Ok(())
}

pub async fn run_dump(target: Target, iface: Option<IfaceSelector>, json: bool) -> Result<()> {
    let camera = common::open_target(&target, iface.as_ref(), viva_genicam::Access::Monitor)
        .await
        .context("open camera for dump")?;
    let values = with_camera!(&camera, camera => dump(camera));
    if json {
        return common::print_json(&values);
    }
    let mut out = std::io::stdout().lock();
    for value in &values {
        writeln!(out, "{} = {}", value.name, value.value)?;
    }
    Ok(())
}

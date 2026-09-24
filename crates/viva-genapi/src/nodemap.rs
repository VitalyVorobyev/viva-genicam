//! NodeMap implementation for runtime feature access.

use std::cell::Cell;
use std::collections::{HashMap, HashSet, hash_map::Entry as HashMapEntry};

use tracing::{debug, trace, warn};
use viva_genapi_xml::{
    AccessMode, AddressTerm, Addressing, ByteOrder, EnumEntryDecl, EnumValueSrc, FloatEncoding,
    FormulaBindings, IndexOffset, NodeDecl, PredicateRefs, Sign, SkippedNode, Visibility, XmlModel,
};

use crate::bitops::{extract, insert};
use crate::conversions::{
    apply_scale, bytes_to_i64, decode_ieee754, encode_bitfield_value, encode_float, encode_ieee754,
    get_raw_or_read, i64_to_bytes, interpret_bitfield_value, map_bitops_error, round_to_i64,
};
use crate::nodes::{
    BooleanNode, CategoryNode, CommandNode, ConverterNode, EnumMapping, EnumNode, FloatNode,
    FormulaVar, IntConverterNode, IntegerNode, Node, RegisterNode, SkNode, StringNode,
};
use crate::swissknife::{
    AstNode as SkAst, EvalError as SkEvalError, EvalMode, Value as SkValue, VarAttr,
    collect_identifiers, evaluate as eval_ast, is_builtin_constant, parse_expression,
    split_qualified, substitute,
};
use crate::{GenApiError, RegisterIo, SkOutput};

/// Runtime nodemap built from an [`XmlModel`] capable of reading and writing
/// feature values via a [`RegisterIo`] transport.
#[derive(Debug)]
pub struct NodeMap {
    version: String,
    nodes: HashMap<String, Node>,
    dependents: HashMap<String, Vec<String>>,
    skipped: Vec<SkippedNode>,
    generation: Cell<u64>,
}

fn register_addressing_dependency(
    dependents: &mut HashMap<String, Vec<String>>,
    node_name: &str,
    addressing: &Addressing,
) {
    for provider in addressing.referenced_nodes() {
        dependents
            .entry(provider.to_string())
            .or_default()
            .push(node_name.to_string());
    }
}

fn register_predicate_dependencies(
    dependents: &mut HashMap<String, Vec<String>>,
    node_name: &str,
    predicates: &PredicateRefs,
) {
    for provider in predicates.references() {
        dependents
            .entry(provider.to_string())
            .or_default()
            .push(node_name.to_string());
    }
}

fn ensure_readable(access: &AccessMode, name: &str) -> Result<(), GenApiError> {
    if matches!(access, AccessMode::WO) {
        return Err(GenApiError::Access(name.to_string()));
    }
    Ok(())
}

/// The node `node` takes its value from through `<pValue>`, if it delegates.
fn value_provider(node: &Node) -> Option<&str> {
    match node {
        Node::Integer(n) => n.pvalue.as_deref(),
        Node::Float(n) => n.pvalue.as_deref(),
        Node::Enum(n) => n.pvalue.as_deref(),
        Node::Boolean(n) => n.pvalue.as_deref(),
        Node::Command(n) => n.pvalue.as_deref(),
        Node::Converter(n) => Some(n.p_value.as_str()),
        Node::IntConverter(n) => Some(n.p_value.as_str()),
        _ => None,
    }
}

/// Refuse a register bound to a port we do not route.
///
/// `<pPort>` selects which port a register's address is relative to. Absent, or
/// `"Device"`, means the device's own register space; anything else — a chunk
/// port, an event port, a serial port — is a different address space entirely.
///
/// Reading such a node through the device port would not fail. It would return
/// whatever happens to live at that address: the Micro-Epsilon scanCONTROL's
/// three `Chunk*Results` registers all sit at address `0x0`, so a device-port
/// read hands back GVCP bootstrap registers dressed as measurement data. That
/// is the silent-wrong-answer failure ADR-0018 exists to refuse, so the node is
/// parsed and listed but not readable until GA-12 routes ports properly.
///
/// This is deliberately stricter than the ~200 `IntReg`/`FloatReg`/`StringReg`
/// nodes on non-device ports that this crate already exposes without a guard.
/// The asymmetry is GA-12's debt, not a new inconsistency: new code conforms,
/// and the old code is on the list.
fn ensure_device_port(name: &str, port: Option<&str>) -> Result<(), GenApiError> {
    match port {
        None => Ok(()),
        Some(p) if p.eq_ignore_ascii_case("Device") => Ok(()),
        Some(p) => Err(GenApiError::Unavailable(format!(
            "register '{name}' is bound to port '{p}'; \
             non-device ports are not routed yet (GA-12)"
        ))),
    }
}

fn ensure_writable(access: &AccessMode, name: &str) -> Result<(), GenApiError> {
    if matches!(access, AccessMode::RO) {
        return Err(GenApiError::Access(name.to_string()));
    }
    Ok(())
}

impl NodeMap {
    /// Return the schema version string associated with the XML description.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Fetch a node by name for inspection.
    pub fn node(&self, name: &str) -> Option<&Node> {
        self.nodes.get(name)
    }

    /// Return an iterator over all node names in the map.
    pub fn node_names(&self) -> impl Iterator<Item = &str> {
        self.nodes.keys().map(|s| s.as_str())
    }

    /// Return the list of nodes that should be invalidated when `name` changes.
    ///
    /// Returns an empty slice if the node has no dependents.
    pub fn dependents(&self, name: &str) -> &[String] {
        self.dependents
            .get(name)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Return all category nodes as `(name, children)` pairs.
    pub fn categories(&self) -> Vec<(&str, &[String])> {
        self.nodes
            .values()
            .filter_map(|node| match node {
                Node::Category(cat) => Some((cat.name.as_str(), cat.children.as_slice())),
                _ => None,
            })
            .collect()
    }

    /// Return the names of the nodes a [`Node::Command`] executes through.
    ///
    /// A command reaches its register through `<pValue>`, and the target may
    /// delegate again through its own `<pValue>` — an integer's, or an
    /// `<IntConverter>`'s. The whole chain that [`NodeMap::exec_command`]
    /// writes along is returned, found from the declarations rather than from
    /// node names.
    ///
    /// None of these nodes holds a value worth displaying. Writing one
    /// triggers an action; reading it is at best meaningless and on some
    /// devices significant, and many declare it `WO`. A feature snapshot
    /// should report them without reading them (backlog ST-21, issue #112).
    pub fn command_targets(&self) -> HashSet<String> {
        let mut targets = HashSet::new();
        for node in self.nodes.values() {
            let Node::Command(cmd) = node else {
                continue;
            };
            if let Some(first) = cmd.pvalue.as_deref() {
                targets.extend(self.value_chain(first).into_iter().map(str::to_string));
            }
        }
        targets
    }

    /// Whether reading `name` is refused because a node on its value path is
    /// declared `WO`: the node itself, or one it takes its value from through
    /// `<pValue>`.
    ///
    /// The declared mode of `name` alone is not enough. A feature can be `RW`
    /// — or declare no access mode at all — and delegate to a `WO` register:
    /// GigE Vision's `ActionDeviceKey` does exactly that on a Vieworks camera
    /// in issue #112, and its read failed one hop down. Returns `false` for an
    /// unknown name.
    pub fn is_write_only(&self, name: &str) -> bool {
        self.value_chain(name).into_iter().any(|link| {
            self.nodes
                .get(link)
                .is_some_and(|node| matches!(node.access_mode(), Some(AccessMode::WO)))
        })
    }

    /// `start` followed by each node it takes its value from through
    /// `<pValue>`, until a node that delegates no further. A `<pValue>` cycle
    /// ends the chain rather than looping.
    fn value_chain<'a>(&'a self, start: &'a str) -> Vec<&'a str> {
        let mut chain = vec![start];
        let mut current = start;
        while let Some(next) = self.nodes.get(current).and_then(value_provider) {
            if chain.contains(&next) {
                break;
            }
            chain.push(next);
            current = next;
        }
        chain
    }

    /// Return names of nodes visible at the given level or below.
    ///
    /// A node with `Visibility::Expert` is visible at level `Expert` and `Guru`,
    /// but not at `Beginner`.
    pub fn nodes_at_visibility(&self, level: Visibility) -> Vec<&str> {
        self.nodes
            .iter()
            .filter(|(_, node)| node.visibility() <= level)
            .map(|(name, _)| name.as_str())
            .collect()
    }

    /// Construct a [`NodeMap`] from an [`XmlModel`], validating formulas.
    ///
    /// A declaration that cannot be turned into a runtime node is dropped and
    /// recorded in [`NodeMap::skipped`] rather than failing the whole model.
    /// Cameras carry thousands of nodes and only a handful of them matter to
    /// any one application; refusing to open a camera because one obscure
    /// feature is unrepresentable serves nobody.
    pub fn try_from_xml(model: XmlModel) -> Result<Self, GenApiError> {
        let mut nodes = HashMap::new();
        let mut dependents: HashMap<String, Vec<String>> = HashMap::new();
        // Losses from the XML layer travel with the ones from this layer. A
        // consumer holding a NodeMap has no access to the XmlModel it was
        // built from, so leaving them behind would make a feature the parser
        // could not read indistinguishable from one the camera does not have.
        let mut skipped = model.skipped;
        for decl in model.nodes {
            let tag = decl.kind().to_string();
            let name = Some(decl.name().to_string());
            // A node we cannot build costs that one feature, not the camera.
            // The same isolation the XML layer already applies (issue #48) --
            // a single unusual declaration must not make a camera unopenable.
            let mut local: HashMap<String, Vec<String>> = HashMap::new();
            match build_node(decl, &mut local) {
                Ok((node_name, node)) => {
                    for (provider, mut names) in local {
                        dependents.entry(provider).or_default().append(&mut names);
                    }
                    nodes.insert(node_name, node);
                }
                Err(err) => {
                    warn!(
                        tag = %tag,
                        node = name.as_deref().unwrap_or("<unnamed>"),
                        error = %err,
                        "dropping GenApi node"
                    );
                    skipped.push(SkippedNode {
                        tag,
                        name,
                        error: err.to_string(),
                    });
                }
            }
        }

        Ok(NodeMap {
            version: model.version,
            nodes,
            dependents,
            skipped,
            generation: Cell::new(0),
        })
    }

    /// Declarations this camera has that we do not expose.
    ///
    /// Covers both losses: a node the XML parser could not read, and one it
    /// read but that could not be turned into a runtime node. Empty for a
    /// document we fully understand; anything listed here is worth reporting
    /// as a bug. `viva-camctl report` prints it.
    pub fn skipped(&self) -> &[SkippedNode] {
        &self.skipped
    }

    /// Read an integer feature value using the provided transport.
    pub fn get_integer(&self, name: &str, io: &dyn RegisterIo) -> Result<i64, GenApiError> {
        if let Some(Node::IntConverter(_)) = self.nodes.get(name) {
            return self.get_int_converter(name, io);
        }
        if let Some(output) = self.nodes.get(name).and_then(|node| match node {
            Node::SwissKnife(sk) => Some(sk.output),
            _ => None,
        }) {
            return match output {
                SkOutput::Integer => {
                    let node = match self.nodes.get(name) {
                        Some(Node::SwissKnife(node)) => node,
                        _ => unreachable!("node vanished during lookup"),
                    };
                    let mut stack = HashSet::new();
                    let value = self.evaluate_swissknife(node, io, &mut stack)?;
                    sk_to_i64(name, value)
                }
                SkOutput::Float => Err(GenApiError::Type(name.to_string())),
            };
        }
        let node = self.get_integer_node(name)?;
        ensure_readable(&node.access, name)?;
        self.ensure_selectors(name, &node.selected_if, io)?;
        // Return static value if present.
        if let Some(v) = node.value {
            return Ok(v);
        }
        // Delegate to pValue node if present.
        if let Some(ref pv) = node.pvalue {
            let pv = pv.clone();
            return self.get_integer(&pv, io);
        }
        let addressing = node
            .addressing
            .as_ref()
            .ok_or_else(|| GenApiError::NodeNotFound(format!("{name}: no addressing or pValue")))?;
        let (address, len) = self.resolve_address(name, addressing, io)?;
        if let Some(value) = *node.cache.borrow() {
            return Ok(value);
        }
        let raw = io.read(address, len as usize).map_err(|err| match err {
            GenApiError::Io(_) => err,
            other => other,
        })?;
        let value = if let Some(bitfield) = node.bitfield {
            let extracted = extract(&raw, bitfield).map_err(|err| map_bitops_error(name, err))?;
            interpret_bitfield_value(
                name,
                extracted,
                bitfield.bit_length,
                integer_sign(node).is_signed(),
            )?
        } else {
            bytes_to_i64(name, &raw, integer_sign(node), node.byte_order)?
        };
        debug!(node = %name, raw = value, "read integer feature");
        node.cache.replace(Some(value));
        node.raw_cache.replace(Some(raw));
        Ok(value)
    }

    /// Write an integer feature and update dependent caches.
    pub fn set_integer(
        &mut self,
        name: &str,
        value: i64,
        io: &dyn RegisterIo,
    ) -> Result<(), GenApiError> {
        if let Some(Node::IntConverter(_)) = self.nodes.get(name) {
            return self.set_int_converter(name, value, io);
        }
        let node = self.get_integer_node(name)?;
        self.ensure_writable_now(name, &node.access, io)?;
        self.ensure_selectors(name, &node.selected_if, io)?;
        if let Some(ref pv) = node.pvalue {
            let pv = pv.clone();
            return self.set_integer(&pv, value, io);
        }
        let addressing = node
            .addressing
            .as_ref()
            .ok_or_else(|| GenApiError::NodeNotFound(format!("{name}: no addressing or pValue")))?;
        let (address, len) = self.resolve_address(name, addressing, io)?;
        if value < node.min || value > node.max {
            return Err(GenApiError::Range(name.to_string()));
        }
        if let Some(inc) = node.inc
            && inc != 0
            && (value - node.min) % inc != 0
        {
            return Err(GenApiError::Range(name.to_string()));
        }
        if let Some(bitfield) = node.bitfield {
            let encoded = encode_bitfield_value(
                name,
                value,
                bitfield.bit_length,
                integer_sign(node).is_signed(),
            )?;
            let readable = !matches!(node.access, AccessMode::WO);
            let mut raw = get_raw_or_read(name, readable, &node.raw_cache, io, address, len)?;
            insert(&mut raw, bitfield, encoded).map_err(|err| map_bitops_error(name, err))?;
            debug!(node = %name, raw = value, "write integer feature");
            io.write(address, &raw).map_err(|err| match err {
                GenApiError::Io(_) => err,
                other => other,
            })?;
            node.cache.replace(Some(value));
            node.raw_cache.replace(Some(raw));
        } else {
            let bytes = i64_to_bytes(name, value, len, integer_sign(node), node.byte_order)?;
            debug!(node = %name, raw = value, "write integer feature");
            io.write(address, &bytes).map_err(|err| match err {
                GenApiError::Io(_) => err,
                other => other,
            })?;
            node.cache.replace(Some(value));
            node.raw_cache.replace(Some(bytes));
        }
        self.invalidate_dependents(name);
        Ok(())
    }

    /// Read a floating point feature.
    pub fn get_float(&self, name: &str, io: &dyn RegisterIo) -> Result<f64, GenApiError> {
        match self.nodes.get(name) {
            Some(Node::Converter(_)) => return self.get_converter(name, io),
            Some(Node::IntConverter(_)) => {
                return self.get_int_converter(name, io).map(|v| v as f64);
            }
            _ => {}
        }
        if let Some(output) = self.nodes.get(name).and_then(|node| match node {
            Node::SwissKnife(sk) => Some(sk.output),
            _ => None,
        }) {
            return match output {
                SkOutput::Float => {
                    let node = match self.nodes.get(name) {
                        Some(Node::SwissKnife(node)) => node,
                        _ => unreachable!("node vanished during lookup"),
                    };
                    let mut stack = HashSet::new();
                    let value = self.evaluate_swissknife(node, io, &mut stack)?;
                    Ok(value.as_f64())
                }
                SkOutput::Integer => self.get_integer(name, io).map(|v| v as f64),
            };
        }
        let node = self.get_float_node(name)?;
        ensure_readable(&node.access, name)?;
        self.ensure_selectors(name, &node.selected_if, io)?;
        if let Some(ref pv) = node.pvalue {
            let pv = pv.clone();
            return self.get_float(&pv, io);
        }
        let addressing = node
            .addressing
            .as_ref()
            .ok_or_else(|| GenApiError::NodeNotFound(format!("{name}: no addressing or pValue")))?;
        let (address, len) = self.resolve_address(name, addressing, io)?;
        if let Some(value) = *node.cache.borrow() {
            return Ok(value);
        }
        let raw = io.read(address, len as usize).map_err(|err| match err {
            GenApiError::Io(_) => err,
            other => other,
        })?;
        let value = match node.encoding {
            FloatEncoding::Ieee754 => {
                let v = decode_ieee754(name, &raw, node.byte_order)?;
                debug!(node = %name, value = v, "read float feature (ieee754)");
                v
            }
            FloatEncoding::ScaledInteger => {
                // `<Float>`/`<FloatReg>` declare no `<Sign>`; a scaled raw
                // value is conventionally signed so an offset can go either way.
                let raw_value = bytes_to_i64(name, &raw, Sign::Signed, node.byte_order)?;
                let v = apply_scale(node, raw_value as f64);
                debug!(node = %name, raw = raw_value, value = v, "read float feature (scaled)");
                v
            }
        };
        node.cache.replace(Some(value));
        Ok(value)
    }

    /// Write a floating point feature using the scale/offset conversion.
    pub fn set_float(
        &mut self,
        name: &str,
        value: f64,
        io: &dyn RegisterIo,
    ) -> Result<(), GenApiError> {
        match self.nodes.get(name) {
            Some(Node::Converter(_)) => return self.set_converter(name, value, io),
            Some(Node::IntConverter(_)) => {
                return self.set_int_converter(name, round_to_i64(name, value)?, io);
            }
            _ => {}
        }
        let node = self.get_float_node(name)?;
        self.ensure_writable_now(name, &node.access, io)?;
        self.ensure_selectors(name, &node.selected_if, io)?;
        if let Some(ref pv) = node.pvalue {
            let pv = pv.clone();
            return self.set_float(&pv, value, io);
        }
        let addressing = node
            .addressing
            .as_ref()
            .ok_or_else(|| GenApiError::NodeNotFound(format!("{name}: no addressing or pValue")))?;
        let (address, len) = self.resolve_address(name, addressing, io)?;
        if value < node.min || value > node.max {
            return Err(GenApiError::Range(name.to_string()));
        }
        let bytes = match node.encoding {
            FloatEncoding::Ieee754 => {
                let bytes = encode_ieee754(name, value, len, node.byte_order)?;
                debug!(node = %name, value, "write float feature (ieee754)");
                bytes
            }
            FloatEncoding::ScaledInteger => {
                let raw = encode_float(node, value)?;
                let bytes = i64_to_bytes(name, raw, len, Sign::Signed, node.byte_order)?;
                debug!(node = %name, raw, value, "write float feature (scaled)");
                bytes
            }
        };
        io.write(address, &bytes).map_err(|err| match err {
            GenApiError::Io(_) => err,
            other => other,
        })?;
        node.cache.replace(Some(value));
        self.invalidate_dependents(name);
        Ok(())
    }

    /// Read an enumeration feature returning the symbolic entry name.
    pub fn get_enum(&self, name: &str, io: &dyn RegisterIo) -> Result<String, GenApiError> {
        let node = self.get_enum_node(name)?;
        ensure_readable(&node.access, name)?;
        self.ensure_selectors(name, &node.selected_if, io)?;
        // When pValue is set, read the integer from the delegate node.
        if let Some(ref pv) = node.pvalue {
            let pv = pv.clone();
            if let Some(value) = node.value_cache.borrow().clone() {
                return Ok(value);
            }
            let raw_value = self.get_integer(&pv, io)?;
            let entry = self.lookup_enum_entry(node, raw_value, io)?;
            node.value_cache.replace(Some(entry.clone()));
            return Ok(entry);
        }
        let addressing = node
            .addressing
            .as_ref()
            .ok_or_else(|| GenApiError::NodeNotFound(format!("{name}: no addressing")))?;
        let (address, len) = self.resolve_address(name, addressing, io)?;
        if let Some(value) = node.value_cache.borrow().clone() {
            return Ok(value);
        }
        let raw = io.read(address, len as usize).map_err(|err| match err {
            GenApiError::Io(_) => err,
            other => other,
        })?;
        // `<Enumeration>` declares no `<Sign>` and no `<Endianess>` — no
        // document in the vendor corpus declares one — so the GenICam
        // defaults apply: signed entry values, big-endian payload.
        let raw_value = bytes_to_i64(name, &raw, Sign::Signed, ByteOrder::Big)?;
        let entry = self.lookup_enum_entry(node, raw_value, io)?;
        debug!(node = %name, raw = raw_value, entry = %entry, "read enum feature");
        node.value_cache.replace(Some(entry.clone()));
        Ok(entry)
    }

    /// Write an enumeration entry.
    pub fn set_enum(
        &mut self,
        name: &str,
        entry: &str,
        io: &dyn RegisterIo,
    ) -> Result<(), GenApiError> {
        let node = self.get_enum_node(name)?;
        self.ensure_writable_now(name, &node.access, io)?;
        self.ensure_selectors(name, &node.selected_if, io)?;
        if let Some(ref pv) = node.pvalue {
            let pv = pv.clone();
            let entry_decl = node
                .entries
                .iter()
                .find(|candidate| candidate.name == entry)
                .ok_or_else(|| GenApiError::EnumNoSuchEntry {
                    node: name.to_string(),
                    entry: entry.to_string(),
                })?;
            let raw_value = self.resolve_enum_entry_value(node, entry_decl, io)?;
            let entry_str = entry.to_string();
            // Re-borrow node after mutable self call.
            self.set_integer(&pv, raw_value, io)?;
            let node = self.get_enum_node(name)?;
            node.value_cache.replace(Some(entry_str));
            node.invalidate();
            self.invalidate_dependents(name);
            return Ok(());
        }
        let addressing = node
            .addressing
            .as_ref()
            .ok_or_else(|| GenApiError::NodeNotFound(format!("{name}: no addressing")))?;
        let (address, len) = self.resolve_address(name, addressing, io)?;
        let entry_decl = node
            .entries
            .iter()
            .find(|candidate| candidate.name == entry)
            .ok_or_else(|| GenApiError::EnumNoSuchEntry {
                node: name.to_string(),
                entry: entry.to_string(),
            })?;
        let raw = self.resolve_enum_entry_value(node, entry_decl, io)?;
        let bytes = i64_to_bytes(name, raw, len, Sign::Signed, ByteOrder::Big)?;
        debug!(node = %name, raw, entry, "write enum feature");
        io.write(address, &bytes).map_err(|err| match err {
            GenApiError::Io(_) => err,
            other => other,
        })?;
        node.value_cache.replace(None);
        self.invalidate_dependents(name);
        Ok(())
    }

    /// List the available entry names for an enumeration feature.
    pub fn enum_entries(&self, name: &str) -> Result<Vec<String>, GenApiError> {
        let node = self.get_enum_node(name)?;
        if let Some(mapping) = node.mapping_cache.borrow().as_ref() {
            let mut names: Vec<_> = mapping.by_name.keys().cloned().collect();
            names.sort();
            names.dedup();
            return Ok(names);
        }
        let mut names: Vec<_> = node
            .entries
            .iter()
            .map(|entry| entry.name.clone())
            .collect();
        names.sort();
        names.dedup();
        Ok(names)
    }

    /// Evaluate `pIsImplemented` for `name`, returning `true` when the feature
    /// is implemented by the device.
    ///
    /// Absent `pIsImplemented` defaults to `true` (matching the GenICam spec:
    /// an undeclared predicate means "always implemented"). Evaluation errors
    /// propagate to the caller so bad XML is visible rather than silently
    /// reported as implemented.
    pub fn is_implemented(&self, name: &str, io: &dyn RegisterIo) -> Result<bool, GenApiError> {
        let prefs = self.predicate_refs(name)?;
        match &prefs.p_is_implemented {
            None => Ok(true),
            Some(provider) => self.eval_predicate_ref(name, provider, io),
        }
    }

    /// Evaluate `pIsAvailable` plus selector gating for `name`.
    ///
    /// Returns `false` when the feature is not implemented, when
    /// `pIsAvailable` evaluates to zero, or when any `selected_if` rule is
    /// violated by the current selector value. Callers that want pure XML
    /// gating without selector checks should use [`NodeMap::is_implemented`]
    /// instead.
    pub fn is_available(&self, name: &str, io: &dyn RegisterIo) -> Result<bool, GenApiError> {
        if !self.is_implemented(name, io)? {
            return Ok(false);
        }
        let prefs = self.predicate_refs(name)?;
        if let Some(provider) = &prefs.p_is_available
            && !self.eval_predicate_ref(name, provider, io)?
        {
            return Ok(false);
        }
        let selected_if = self
            .nodes
            .get(name)
            .and_then(Self::selected_if_slice)
            .unwrap_or(&[]);
        self.selectors_allow(selected_if, io)
    }

    /// Refuse a write the device's current state does not permit.
    ///
    /// The static `<AccessMode>` is only half the picture, and for a great
    /// many real nodes it is the less informative half: FLIR's `ExposureTime`
    /// declares no `<AccessMode>` at all — so it defaults to `RW` — and puts
    /// the entire restriction in `<pIsLocked>ExposureTime_Lck</pIsLocked>`, a
    /// device register. Checking only the static mode meant we sent writes the
    /// camera's own description said were not allowed, and the device answered
    /// `ACCESS_DENIED` (issue #45).
    ///
    /// Deliberately *not* routed through [`NodeMap::effective_access_mode`]:
    /// that function collapses "unavailable" into `RO` because it serves a UI
    /// that has a separate availability flag. Here the distinction is the
    /// whole value of the error, so the two conditions are checked separately.
    ///
    /// Reads keep the static [`AccessMode::WO`] check only. Evaluating
    /// predicates on every `get` would add device round-trips to the hottest
    /// path in the library for a check the subsequent read reports anyway; a
    /// refused write, by contrast, is worth one predicate evaluation to turn a
    /// wire error into a named local one. See backlog GA-06.
    fn ensure_writable_now(
        &self,
        name: &str,
        access: &AccessMode,
        io: &dyn RegisterIo,
    ) -> Result<(), GenApiError> {
        ensure_writable(access, name)?;
        if !self.is_available(name, io)? {
            return Err(GenApiError::Unavailable(name.to_string()));
        }
        let prefs = self.predicate_refs(name)?;
        if let Some(provider) = &prefs.p_is_locked
            && self.eval_predicate_ref(name, provider, io)?
        {
            return Err(GenApiError::Locked {
                name: name.to_string(),
                locked_by: provider.to_string(),
            });
        }
        Ok(())
    }

    /// Return the effective [`AccessMode`] for `name` given the current
    /// device state.
    ///
    /// - If the feature is unavailable (see [`NodeMap::is_available`]), the
    ///   function returns `AccessMode::RO` — we cannot report "NA" without
    ///   introducing a new variant, and Studio's wire protocol carries the
    ///   availability flag separately.
    /// - If `pIsLocked` evaluates truthy, `RW` downgrades to `RO`; `RO` and
    ///   `WO` are unaffected.
    /// - Otherwise the statically declared access mode applies.
    pub fn effective_access_mode(
        &self,
        name: &str,
        io: &dyn RegisterIo,
    ) -> Result<AccessMode, GenApiError> {
        let node = self
            .nodes
            .get(name)
            .ok_or_else(|| GenApiError::NodeNotFound(name.to_string()))?;
        let base = node.access_mode().unwrap_or(AccessMode::RO);
        if !self.is_available(name, io)? {
            return Ok(AccessMode::RO);
        }
        let prefs = self.predicate_refs(name)?;
        if let Some(provider) = &prefs.p_is_locked
            && self.eval_predicate_ref(name, provider, io)?
        {
            return Ok(match base {
                AccessMode::RW => AccessMode::RO,
                other => other,
            });
        }
        Ok(base)
    }

    /// Return the subset of enum entries currently reported as available by
    /// the device, or the full static list when no entry declares an
    /// `pIsImplemented`/`pIsAvailable`.
    ///
    /// Falling back to the full list preserves current behaviour for XMLs
    /// that don't gate individual entries, so callers stop seeing the stale
    /// static list when the new predicates are added and otherwise behave as
    /// before.
    pub fn available_enum_entries(
        &self,
        name: &str,
        io: &dyn RegisterIo,
    ) -> Result<Vec<String>, GenApiError> {
        let node = self.get_enum_node(name)?;
        let any_entry_predicate = node.entries.iter().any(|e| !e.predicates.is_empty());
        if !any_entry_predicate {
            return self.enum_entries(name);
        }
        let mut out = Vec::new();
        for entry in &node.entries {
            if let Some(provider) = &entry.predicates.p_is_implemented
                && !self.eval_predicate_ref(name, provider, io)?
            {
                continue;
            }
            if let Some(provider) = &entry.predicates.p_is_available
                && !self.eval_predicate_ref(name, provider, io)?
            {
                continue;
            }
            out.push(entry.name.clone());
        }
        out.sort();
        out.dedup();
        Ok(out)
    }

    fn predicate_refs(&self, name: &str) -> Result<&PredicateRefs, GenApiError> {
        self.nodes
            .get(name)
            .map(Node::predicates)
            .ok_or_else(|| GenApiError::NodeNotFound(name.to_string()))
    }

    /// Evaluate a `pIs*` reference by reading the target node as an integer
    /// truthy value.
    ///
    /// `ctx` is the node that owns the predicate; it is used for diagnostics
    /// and cycle detection so a predicate that accidentally resolves back to
    /// its own owner fails fast rather than recursing. Providers can be any
    /// numeric-resolvable node: Integer, Boolean, Enum (integer form),
    /// SwissKnife, or a Converter.
    fn eval_predicate_ref(
        &self,
        ctx: &str,
        provider: &str,
        io: &dyn RegisterIo,
    ) -> Result<bool, GenApiError> {
        if provider == ctx {
            return Err(GenApiError::ExprEval {
                name: ctx.to_string(),
                msg: "predicate references the node it gates".into(),
            });
        }
        let mut stack = HashSet::new();
        stack.insert(ctx.to_string());
        let value = self.resolve_numeric(provider, io, &mut stack)?;
        trace!(node = %ctx, provider, value, "predicate eval");
        Ok(value != 0.0)
    }

    /// Non-erroring cousin of [`NodeMap::ensure_selectors`] — returns `Ok(false)`
    /// when a selector gating rule rejects the current state, rather than
    /// converting that into a [`GenApiError::Unavailable`].
    fn selectors_allow(
        &self,
        rules: &[(String, Vec<String>)],
        io: &dyn RegisterIo,
    ) -> Result<bool, GenApiError> {
        for (selector, allowed) in rules {
            if allowed.is_empty() {
                continue;
            }
            let current = self.get_selector_value(selector, io)?;
            if !allowed.iter().any(|v| v == &current) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn selected_if_slice(node: &Node) -> Option<&[(String, Vec<String>)]> {
        match node {
            Node::Integer(n) => Some(&n.selected_if),
            Node::Float(n) => Some(&n.selected_if),
            Node::Enum(n) => Some(&n.selected_if),
            Node::Boolean(n) => Some(&n.selected_if),
            _ => None,
        }
    }

    /// Read a boolean feature.
    pub fn get_bool(&self, name: &str, io: &dyn RegisterIo) -> Result<bool, GenApiError> {
        let node = self.get_bool_node(name)?;
        ensure_readable(&node.access, name)?;
        self.ensure_selectors(name, &node.selected_if, io)?;
        if let Some(ref pv) = node.pvalue {
            let pv = pv.clone();
            let raw = self.get_integer(&pv, io)?;
            let on = node.on_value.unwrap_or(1);
            return Ok(raw == on);
        }
        let addressing = node
            .addressing
            .as_ref()
            .ok_or_else(|| GenApiError::NodeNotFound(format!("{name}: no addressing or pValue")))?;
        let bitfield = node
            .bitfield
            .ok_or_else(|| GenApiError::Parse(format!("{name}: boolean without bitfield")))?;
        let (address, len) = self.resolve_address(name, addressing, io)?;
        if let Some(value) = *node.cache.borrow() {
            return Ok(value);
        }
        let raw = io.read(address, len as usize).map_err(|err| match err {
            GenApiError::Io(_) => err,
            other => other,
        })?;
        let raw_value = extract(&raw, bitfield).map_err(|err| map_bitops_error(name, err))?;
        let value = raw_value != 0;
        debug!(node = %name, raw = raw_value, value, "read boolean feature");
        node.cache.replace(Some(value));
        node.raw_cache.replace(Some(raw));
        Ok(value)
    }

    /// Write a boolean feature.
    pub fn set_bool(
        &mut self,
        name: &str,
        value: bool,
        io: &dyn RegisterIo,
    ) -> Result<(), GenApiError> {
        let node = self.get_bool_node(name)?;
        self.ensure_writable_now(name, &node.access, io)?;
        self.ensure_selectors(name, &node.selected_if, io)?;
        if let Some(ref pv) = node.pvalue {
            let pv = pv.clone();
            let on = node.on_value.unwrap_or(1);
            let off = node.off_value.unwrap_or(0);
            let raw = if value { on } else { off };
            return self.set_integer(&pv, raw, io);
        }
        let addressing = node
            .addressing
            .as_ref()
            .ok_or_else(|| GenApiError::NodeNotFound(format!("{name}: no addressing or pValue")))?;
        let bitfield = node
            .bitfield
            .ok_or_else(|| GenApiError::Parse(format!("{name}: boolean without bitfield")))?;
        let (address, len) = self.resolve_address(name, addressing, io)?;
        let encoded = if value { 1 } else { 0 };
        let readable = !matches!(node.access, AccessMode::WO);
        let mut raw = get_raw_or_read(name, readable, &node.raw_cache, io, address, len)?;
        insert(&mut raw, bitfield, encoded).map_err(|err| map_bitops_error(name, err))?;
        debug!(node = %name, raw = encoded, value, "write boolean feature");
        io.write(address, &raw).map_err(|err| match err {
            GenApiError::Io(_) => err,
            other => other,
        })?;
        node.cache.replace(Some(value));
        node.raw_cache.replace(Some(raw));
        self.invalidate_dependents(name);
        Ok(())
    }

    /// Execute a command feature by writing a value to the command register.
    pub fn exec_command(&mut self, name: &str, io: &dyn RegisterIo) -> Result<(), GenApiError> {
        let node = self.get_command_node(name)?;
        // Determine the value to write and the target.
        let cmd_value = node.command_value.unwrap_or(1);

        if let Some(ref pv) = node.pvalue {
            // Delegate to the pValue node.
            let pv = pv.clone();
            debug!(node = %name, "execute command via pValue");
            return self.set_integer(&pv, cmd_value, io);
        }

        let address = node
            .address
            .ok_or_else(|| GenApiError::NodeNotFound(format!("{name}: no address or pValue")))?;
        if node.len == 0 {
            return Err(GenApiError::Parse(format!(
                "command node {name} has zero length"
            )));
        }
        let data = i64_to_bytes(name, cmd_value, node.len, Sign::Signed, ByteOrder::Big)?;
        debug!(node = %name, "execute command");
        io.write(address, &data).map_err(|err| match err {
            GenApiError::Io(_) => err,
            other => other,
        })?;
        self.invalidate_dependents(name);
        Ok(())
    }

    fn get_integer_node(&self, name: &str) -> Result<&IntegerNode, GenApiError> {
        match self.nodes.get(name) {
            Some(Node::Integer(node)) => Ok(node),
            Some(_) => Err(GenApiError::Type(name.to_string())),
            None => Err(GenApiError::NodeNotFound(name.to_string())),
        }
    }

    fn get_float_node(&self, name: &str) -> Result<&FloatNode, GenApiError> {
        match self.nodes.get(name) {
            Some(Node::Float(node)) => Ok(node),
            Some(_) => Err(GenApiError::Type(name.to_string())),
            None => Err(GenApiError::NodeNotFound(name.to_string())),
        }
    }

    fn get_enum_node(&self, name: &str) -> Result<&EnumNode, GenApiError> {
        match self.nodes.get(name) {
            Some(Node::Enum(node)) => Ok(node),
            Some(_) => Err(GenApiError::Type(name.to_string())),
            None => Err(GenApiError::NodeNotFound(name.to_string())),
        }
    }

    fn get_bool_node(&self, name: &str) -> Result<&BooleanNode, GenApiError> {
        match self.nodes.get(name) {
            Some(Node::Boolean(node)) => Ok(node),
            Some(_) => Err(GenApiError::Type(name.to_string())),
            None => Err(GenApiError::NodeNotFound(name.to_string())),
        }
    }

    fn get_command_node(&self, name: &str) -> Result<&CommandNode, GenApiError> {
        match self.nodes.get(name) {
            Some(Node::Command(node)) => Ok(node),
            Some(_) => Err(GenApiError::Type(name.to_string())),
            None => Err(GenApiError::NodeNotFound(name.to_string())),
        }
    }

    fn ensure_selectors(
        &self,
        node_name: &str,
        rules: &[(String, Vec<String>)],
        io: &dyn RegisterIo,
    ) -> Result<(), GenApiError> {
        for (selector, allowed) in rules {
            if allowed.is_empty() {
                continue;
            }
            let current = self.get_selector_value(selector, io)?;
            if !allowed.iter().any(|value| value == &current) {
                return Err(GenApiError::Unavailable(format!(
                    "node '{node_name}' unavailable for selector '{selector}={current}'"
                )));
            }
        }
        Ok(())
    }

    fn lookup_enum_entry(
        &self,
        node: &EnumNode,
        raw_value: i64,
        io: &dyn RegisterIo,
    ) -> Result<String, GenApiError> {
        {
            let mut cache = node.mapping_cache.borrow_mut();
            if cache.is_none() {
                *cache = Some(self.build_enum_mapping(node, io)?);
            }
            if let Some(mapping) = cache.as_ref()
                && let Some(entry) = mapping.by_value.get(&raw_value)
            {
                return Ok(entry.clone());
            }
            *cache = Some(self.build_enum_mapping(node, io)?);
            if let Some(mapping) = cache.as_ref()
                && let Some(entry) = mapping.by_value.get(&raw_value)
            {
                return Ok(entry.clone());
            }
        }
        Err(GenApiError::EnumValueUnknown {
            node: node.name.clone(),
            value: raw_value,
        })
    }

    fn build_enum_mapping(
        &self,
        node: &EnumNode,
        io: &dyn RegisterIo,
    ) -> Result<EnumMapping, GenApiError> {
        let mut by_value = HashMap::new();
        let mut by_name = HashMap::new();

        for entry in &node.entries {
            let value = self.resolve_enum_entry_value(node, entry, io)?;
            match by_value.entry(value) {
                HashMapEntry::Vacant(slot) => {
                    slot.insert(entry.name.clone());
                }
                HashMapEntry::Occupied(existing) => {
                    warn!(
                        enum_node = %node.name,
                        value,
                        kept = %existing.get(),
                        dropped = %entry.name,
                        "duplicate enum value"
                    );
                }
            }
            by_name.insert(entry.name.clone(), value);
        }

        let mut summary: Vec<_> = by_value
            .iter()
            .map(|(value, name)| (*value, name.clone()))
            .collect();
        summary.sort_by_key(|(value, _)| *value);
        debug!(node = %node.name, entries = ?summary, "build enum mapping");

        Ok(EnumMapping { by_value, by_name })
    }

    fn resolve_enum_entry_value(
        &self,
        node: &EnumNode,
        entry: &EnumEntryDecl,
        io: &dyn RegisterIo,
    ) -> Result<i64, GenApiError> {
        match &entry.value {
            EnumValueSrc::Literal(value) => Ok(*value),
            EnumValueSrc::FromNode(provider) => {
                let value = self.get_integer(provider, io)?;
                trace!(
                    enum_node = %node.name,
                    entry = %entry.name,
                    provider = %provider,
                    value,
                    "resolved enum entry from provider"
                );
                Ok(value)
            }
        }
    }

    /// Resolve the device register address and length backing a feature.
    ///
    /// This is the addressing half of typed feature access, exposed for
    /// callers that need raw register I/O through [`RegisterIo`] — a
    /// file-transfer buffer, for instance, whose address the XML supplies
    /// through `<pAddress>` and which no typed accessor covers. Address
    /// terms, `<pIndex>` scaling and selector blocks resolve exactly as they
    /// do for `get_integer` and friends, so a caller does not have to
    /// reimplement GenICam addressing.
    ///
    /// Returns [`GenApiError::Unavailable`] for a node that has no addressing
    /// of its own because it delegates through `<pValue>`, and for a
    /// selector-mapped node whose current selector value has no block.
    pub fn register_address(
        &self,
        name: &str,
        io: &dyn RegisterIo,
    ) -> Result<(u64, u32), GenApiError> {
        let node = self
            .node(name)
            .ok_or_else(|| GenApiError::NodeNotFound(name.to_string()))?;
        let addressing = match node {
            Node::Integer(node) => node.addressing.as_ref(),
            Node::Float(node) => node.addressing.as_ref(),
            Node::Enum(node) => node.addressing.as_ref(),
            Node::Boolean(node) => node.addressing.as_ref(),
            Node::String(node) => Some(&node.addressing),
            Node::Register(node) => Some(&node.addressing),
            _ => None,
        }
        .ok_or_else(|| {
            GenApiError::Unavailable(format!("node '{name}' has no register addressing"))
        })?;
        self.resolve_address(name, addressing, io)
    }

    fn resolve_address(
        &self,
        node_name: &str,
        addressing: &Addressing,
        io: &dyn RegisterIo,
    ) -> Result<(u64, u32), GenApiError> {
        match addressing {
            Addressing::Sum { terms, len } => {
                let mut address: u64 = 0;
                for term in terms {
                    address =
                        address.wrapping_add(self.resolve_address_term(node_name, term, *len, io)?);
                }
                if terms.len() > 1 {
                    debug!(
                        node = %node_name,
                        terms = terms.len(),
                        address = format_args!("0x{address:X}"),
                        len = *len,
                        "resolve summed address"
                    );
                }
                Ok((address, *len))
            }
            Addressing::BySelector { selector, map } => {
                let value = self.get_selector_value(selector, io)?;
                if let Some((_, (address, len))) = map.iter().find(|(name, _)| name == &value) {
                    let addr = *address;
                    let len = *len;
                    debug!(
                        node = %node_name,
                        selector = %selector,
                        value = %value,
                        address = format_args!("0x{addr:X}"),
                        len,
                        "resolve address via selector"
                    );
                    Ok((addr, len))
                } else {
                    Err(GenApiError::Unavailable(format!(
                        "node '{node_name}' unavailable for selector '{selector}={value}'"
                    )))
                }
            }
        }
    }

    /// Resolve one address term to the offset it contributes.
    fn resolve_address_term(
        &self,
        node_name: &str,
        term: &AddressTerm,
        len: u32,
        io: &dyn RegisterIo,
    ) -> Result<u64, GenApiError> {
        let bad = |addr: i64| GenApiError::BadIndirectAddress {
            name: node_name.to_string(),
            addr,
        };
        match term {
            AddressTerm::Fixed(offset) => Ok(*offset),
            AddressTerm::Node(provider) => {
                let value = self.get_integer(provider, io)?;
                u64::try_from(value).map_err(|_| bad(value))
            }
            AddressTerm::Index { node, offset } => {
                let index = self.get_integer(node, io)?;
                let index = u64::try_from(index).map_err(|_| bad(index))?;
                let stride = match offset {
                    IndexOffset::Fixed(stride) => *stride,
                    IndexOffset::Node(provider) => {
                        let value = self.get_integer(provider, io)?;
                        u64::try_from(value).map_err(|_| bad(value))?
                    }
                    // A bare `<pIndex>` strides by the register length.
                    IndexOffset::Length => u64::from(len),
                };
                Ok(index.wrapping_mul(stride))
            }
        }
    }

    fn get_selector_value(
        &self,
        selector: &str,
        io: &dyn RegisterIo,
    ) -> Result<String, GenApiError> {
        match self.nodes.get(selector) {
            Some(Node::Enum(_)) => self.get_enum(selector, io),
            Some(Node::Boolean(_)) => Ok(self.get_bool(selector, io)?.to_string()),
            Some(Node::Integer(_)) => Ok(self.get_integer(selector, io)?.to_string()),
            Some(_) => Err(GenApiError::Parse(format!(
                "selector {selector} has unsupported type"
            ))),
            None => Err(GenApiError::NodeNotFound(selector.to_string())),
        }
    }

    /// Bind a formula's declared variables and evaluate it.
    ///
    /// Shared by SwissKnife, Converter and IntConverter: they differ only in
    /// which AST and variable list they hand over, in the arithmetic mode, and
    /// in any variables the caller binds directly (`FROM` and `OLD` on a
    /// write, which have no provider node to read).
    #[allow(clippy::too_many_arguments)]
    fn eval_formula(
        &self,
        name: &str,
        ast: &SkAst,
        vars: &[FormulaVar],
        overrides: &[(&str, SkValue)],
        mode: EvalMode,
        io: &dyn RegisterIo,
        stack: &mut HashSet<String>,
    ) -> Result<SkValue, GenApiError> {
        let mut values: HashMap<String, SkValue> = HashMap::new();
        for var in vars {
            if overrides.iter().any(|(ident, _)| *ident == var.name) {
                continue;
            }
            let value = self.resolve_formula_var(name, var, io, stack)?;
            values.insert(var.name.clone(), value);
        }
        for (ident, value) in overrides {
            values.insert((*ident).to_string(), *value);
        }
        let mut resolver = |ident: &str| -> Result<SkValue, SkEvalError> {
            values
                .get(ident)
                .copied()
                .ok_or_else(|| SkEvalError::UnknownVariable(ident.to_string()))
        };
        eval_ast(ast, &mut resolver, mode).map_err(|err| expr_error(name, err))
    }

    fn evaluate_swissknife(
        &self,
        node: &SkNode,
        io: &dyn RegisterIo,
        stack: &mut HashSet<String>,
    ) -> Result<SkValue, GenApiError> {
        if let Some((value, generation)) = *node.cache.borrow()
            && generation == self.generation.get()
        {
            return Ok(value);
        }
        if !stack.insert(node.name.clone()) {
            stack.remove(&node.name);
            return Err(GenApiError::ExprEval {
                name: node.name.clone(),
                msg: "cyclic dependency".into(),
            });
        }
        let current_gen = self.generation.get();
        let result = self.eval_formula(
            &node.name,
            &node.ast,
            &node.vars,
            &[],
            eval_mode(node.output),
            io,
            stack,
        );
        stack.remove(&node.name);
        let value = result?;
        debug!(node = %node.name, value = %value, "evaluate SwissKnife");
        node.cache.replace(Some((value, current_gen)));
        Ok(value)
    }

    /// Resolve a node reference to the value a formula should see.
    ///
    /// Integer-typed providers stay integral: routing a 64-bit register value
    /// through `f64` on the way into a formula would round away its low bits.
    fn resolve_value(
        &self,
        provider: &str,
        io: &dyn RegisterIo,
        stack: &mut HashSet<String>,
    ) -> Result<SkValue, GenApiError> {
        match self.nodes.get(provider) {
            Some(Node::Integer(_)) => self.get_integer(provider, io).map(SkValue::Int),
            Some(Node::Float(_)) => self.get_float(provider, io).map(SkValue::Float),
            Some(Node::Boolean(_)) => self
                .get_bool(provider, io)
                .map(|flag| SkValue::Int(i64::from(flag))),
            Some(Node::Enum(_)) => self.get_enum_numeric(provider, io).map(SkValue::Int),
            Some(Node::SwissKnife(node)) => self.evaluate_swissknife(node, io, stack),
            Some(Node::Converter(node)) => self.evaluate_converter(node, io, stack),
            Some(Node::IntConverter(node)) => self
                .evaluate_int_converter(node, io, stack)
                .map(SkValue::Int),
            Some(_) => Err(GenApiError::Type(provider.to_string())),
            None => Err(GenApiError::NodeNotFound(provider.to_string())),
        }
    }

    /// Resolve one formula variable: the provider's value, or the property of
    /// it that a `.Min` / `.Max` / `.Inc` / `.Entry.<Name>` extension selects.
    fn resolve_formula_var(
        &self,
        formula: &str,
        var: &FormulaVar,
        io: &dyn RegisterIo,
        stack: &mut HashSet<String>,
    ) -> Result<SkValue, GenApiError> {
        match &var.attr {
            VarAttr::Value => self.resolve_value(&var.provider, io, stack),
            VarAttr::Entry(entry) => {
                let node = match self.nodes.get(&var.provider) {
                    Some(Node::Enum(node)) => node,
                    Some(_) => return Err(GenApiError::Type(var.provider.clone())),
                    None => return Err(GenApiError::NodeNotFound(var.provider.clone())),
                };
                let decl = node
                    .entries
                    .iter()
                    .find(|decl| decl.name == *entry)
                    .ok_or_else(|| GenApiError::EnumNoSuchEntry {
                        node: var.provider.clone(),
                        entry: entry.clone(),
                    })?;
                self.resolve_enum_entry_value(node, decl, io)
                    .map(SkValue::Int)
            }
            attr => self.resolve_limit(formula, var, attr, io, stack),
        }
    }

    /// Resolve `.Min`, `.Max` or `.Inc` of a formula variable's node.
    ///
    /// `<pMin>` / `<pMax>` win over a static bound. A node that declares no
    /// bound of its own but delegates through `<pValue>` takes the bound of
    /// the node it delegates to: an `<Integer>` fronting an `<IntReg>` would
    /// otherwise report the `i64::MIN` / `i64::MAX` placeholders that stand
    /// in for "not declared". That delegation rule is our reading; the
    /// standard defines the extensions but not this case. An undeclared
    /// integer increment is 1. A `<Float>` has no increment in this model,
    /// so `.Inc` on one is an evaluation error rather than a guess.
    fn resolve_limit(
        &self,
        formula: &str,
        var: &FormulaVar,
        attr: &VarAttr,
        io: &dyn RegisterIo,
        stack: &mut HashSet<String>,
    ) -> Result<SkValue, GenApiError> {
        let mut current = var.provider.as_str();
        let mut visited = HashSet::new();
        loop {
            if !visited.insert(current) {
                return Err(GenApiError::ExprEval {
                    name: formula.to_string(),
                    msg: format!("{}: cyclic <pValue> chain at {current}", var.name),
                });
            }
            match self.nodes.get(current) {
                Some(Node::Integer(node)) => {
                    let (pointer, declared) = match attr {
                        VarAttr::Min => (node.p_min.as_deref(), Some(node.min)),
                        VarAttr::Max => (node.p_max.as_deref(), Some(node.max)),
                        _ => (None, node.inc),
                    };
                    if let Some(pointer) = pointer {
                        return self.resolve_value(pointer, io, stack);
                    }
                    let undeclared = match attr {
                        VarAttr::Min => node.min == i64::MIN,
                        VarAttr::Max => node.max == i64::MAX,
                        _ => node.inc.is_none(),
                    };
                    if undeclared && let Some(pvalue) = node.pvalue.as_deref() {
                        current = pvalue;
                        continue;
                    }
                    return Ok(SkValue::Int(declared.unwrap_or(1)));
                }
                Some(Node::Float(node)) => {
                    let (declared, undeclared) = match attr {
                        VarAttr::Min => (Some(node.min), node.min == f64::MIN),
                        VarAttr::Max => (Some(node.max), node.max == f64::MAX),
                        _ => (None, true),
                    };
                    if undeclared && let Some(pvalue) = node.pvalue.as_deref() {
                        current = pvalue;
                        continue;
                    }
                    return declared
                        .map(SkValue::Float)
                        .ok_or_else(|| GenApiError::ExprEval {
                            name: formula.to_string(),
                            msg: format!("{}: Float {current} declares no increment", var.name),
                        });
                }
                Some(other) => {
                    return Err(GenApiError::ExprEval {
                        name: formula.to_string(),
                        msg: format!(
                            "{}: {} {current} has no minimum, maximum or increment",
                            var.name,
                            other.kind_name()
                        ),
                    });
                }
                None => return Err(GenApiError::NodeNotFound(current.to_string())),
            }
        }
    }

    fn resolve_numeric(
        &self,
        provider: &str,
        io: &dyn RegisterIo,
        stack: &mut HashSet<String>,
    ) -> Result<f64, GenApiError> {
        self.resolve_value(provider, io, stack)
            .map(|value| value.as_f64())
    }

    fn get_enum_numeric(&self, name: &str, io: &dyn RegisterIo) -> Result<i64, GenApiError> {
        let entry = self.get_enum(name, io)?;
        let node = self.get_enum_node(name)?;
        {
            let mut mapping = node.mapping_cache.borrow_mut();
            if mapping.is_none() {
                *mapping = Some(self.build_enum_mapping(node, io)?);
            }
            if let Some(map) = mapping.as_ref()
                && let Some(value) = map.by_name.get(&entry)
            {
                return Ok(*value);
            }
        }
        Err(GenApiError::EnumNoSuchEntry {
            node: name.to_string(),
            entry,
        })
    }

    fn invalidate_dependents(&self, name: &str) {
        self.bump_generation();
        if let Some(children) = self.dependents.get(name) {
            let mut visited = HashSet::new();
            for child in children {
                self.invalidate_recursive(child, &mut visited);
            }
        }
    }

    fn invalidate_recursive(&self, name: &str, visited: &mut HashSet<String>) {
        if !visited.insert(name.to_string()) {
            return;
        }
        if let Some(node) = self.nodes.get(name) {
            node.invalidate_cache();
        }
        if let Some(children) = self.dependents.get(name) {
            for child in children {
                self.invalidate_recursive(child, visited);
            }
        }
    }

    fn bump_generation(&self) {
        let current = self.generation.get();
        self.generation.set(current.wrapping_add(1));
    }

    // ========================================================================
    // Converter/IntConverter/String support
    // ========================================================================

    fn get_converter_node(&self, name: &str) -> Result<&ConverterNode, GenApiError> {
        match self.nodes.get(name) {
            Some(Node::Converter(node)) => Ok(node),
            Some(_) => Err(GenApiError::Type(name.to_string())),
            None => Err(GenApiError::NodeNotFound(name.to_string())),
        }
    }

    fn get_int_converter_node(&self, name: &str) -> Result<&IntConverterNode, GenApiError> {
        match self.nodes.get(name) {
            Some(Node::IntConverter(node)) => Ok(node),
            Some(_) => Err(GenApiError::Type(name.to_string())),
            None => Err(GenApiError::NodeNotFound(name.to_string())),
        }
    }

    fn get_string_node(&self, name: &str) -> Result<&StringNode, GenApiError> {
        match self.nodes.get(name) {
            Some(Node::String(node)) => Ok(node),
            Some(_) => Err(GenApiError::Type(name.to_string())),
            None => Err(GenApiError::NodeNotFound(name.to_string())),
        }
    }

    fn get_register_node(&self, name: &str) -> Result<&RegisterNode, GenApiError> {
        match self.nodes.get(name) {
            Some(Node::Register(node)) => Ok(node),
            Some(_) => Err(GenApiError::Type(name.to_string())),
            None => Err(GenApiError::NodeNotFound(name.to_string())),
        }
    }

    /// Read a Converter feature value (float) using the provided transport.
    pub fn get_converter(&self, name: &str, io: &dyn RegisterIo) -> Result<f64, GenApiError> {
        let node = self.get_converter_node(name)?;
        if let Some((value, generation)) = *node.cache.borrow()
            && generation == self.generation.get()
        {
            return Ok(value.as_f64());
        }
        let mut stack = HashSet::new();
        let value = self.evaluate_converter(node, io, &mut stack)?;
        node.cache.replace(Some((value, self.generation.get())));
        Ok(value.as_f64())
    }

    /// Read an IntConverter feature value (integer) using the provided transport.
    pub fn get_int_converter(&self, name: &str, io: &dyn RegisterIo) -> Result<i64, GenApiError> {
        let node = self.get_int_converter_node(name)?;
        if let Some((value, generation)) = *node.cache.borrow()
            && generation == self.generation.get()
        {
            return Ok(value);
        }
        let mut stack = HashSet::new();
        let value = self.evaluate_int_converter(node, io, &mut stack)?;
        node.cache.replace(Some((value, self.generation.get())));
        Ok(value)
    }

    /// Write a Converter feature value (float) through its `<FormulaTo>`.
    pub fn set_converter(
        &mut self,
        name: &str,
        value: f64,
        io: &dyn RegisterIo,
    ) -> Result<(), GenApiError> {
        let node = self.get_converter_node(name)?;
        let (p_value, raw) = self.converter_raw_write(
            &node.name,
            &node.ast_to,
            &node.vars_to,
            &node.p_value,
            SkValue::Float(value),
            eval_mode(node.output),
            io,
        )?;
        self.write_converter_raw(&p_value, raw, io)?;
        self.invalidate_dependents(name);
        Ok(())
    }

    /// Write an IntConverter feature value through its `<FormulaTo>`.
    pub fn set_int_converter(
        &mut self,
        name: &str,
        value: i64,
        io: &dyn RegisterIo,
    ) -> Result<(), GenApiError> {
        let node = self.get_int_converter_node(name)?;
        let (p_value, raw) = self.converter_raw_write(
            &node.name,
            &node.ast_to,
            &node.vars_to,
            &node.p_value,
            SkValue::Int(value),
            EvalMode::Integer,
            io,
        )?;
        self.write_converter_raw(&p_value, raw, io)?;
        self.invalidate_dependents(name);
        Ok(())
    }

    /// Evaluate a converter's `<FormulaTo>` to the raw value to write.
    ///
    /// `FROM` is the value the caller is setting, and `OLD` — where the
    /// formula declares it — is the register's current contents, which
    /// read-modify-write formulas such as `(FROM & 0x7FFFFFFF) | (OLD &
    /// 0x80000000)` depend on.
    #[allow(clippy::too_many_arguments)]
    fn converter_raw_write(
        &self,
        name: &str,
        ast_to: &SkAst,
        vars_to: &[FormulaVar],
        p_value: &str,
        value: SkValue,
        mode: EvalMode,
        io: &dyn RegisterIo,
    ) -> Result<(String, SkValue), GenApiError> {
        let mut stack = HashSet::new();
        let mut overrides = vec![("FROM", value)];
        if vars_to.iter().any(|var| var.name == "OLD") {
            let old = self.resolve_value(p_value, io, &mut stack)?;
            overrides.push(("OLD", old));
        }
        let raw = self.eval_formula(name, ast_to, vars_to, &overrides, mode, io, &mut stack)?;
        Ok((p_value.to_string(), raw))
    }

    fn write_converter_raw(
        &mut self,
        p_value: &str,
        raw: SkValue,
        io: &dyn RegisterIo,
    ) -> Result<(), GenApiError> {
        match self.nodes.get(p_value) {
            Some(Node::Float(_)) => self.set_float(p_value, raw.as_f64(), io),
            Some(Node::Boolean(_)) => self.set_bool(p_value, raw.is_truthy(), io),
            Some(_) => self.set_integer(p_value, sk_to_i64(p_value, raw)?, io),
            None => Err(GenApiError::NodeNotFound(p_value.to_string())),
        }
    }

    /// Read a String feature value using the provided transport.
    pub fn get_string(&self, name: &str, io: &dyn RegisterIo) -> Result<String, GenApiError> {
        let node = self.get_string_node(name)?;
        ensure_readable(&node.access, name)?;
        if let Some((ref value, generation)) = *node.cache.borrow()
            && generation == self.generation.get()
        {
            return Ok(value.clone());
        }
        let (address, len) = self.resolve_address(name, &node.addressing, io)?;
        let raw = io.read(address, len as usize)?;
        // Convert bytes to string, stopping at first null byte
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        let value = String::from_utf8_lossy(&raw[..end]).to_string();
        node.cache
            .replace(Some((value.clone(), self.generation.get())));
        debug!(node = %name, value = %value, "get_string");
        Ok(value)
    }

    /// Write a String feature value using the provided transport.
    pub fn set_string(
        &self,
        name: &str,
        value: &str,
        io: &dyn RegisterIo,
    ) -> Result<(), GenApiError> {
        let node = self.get_string_node(name)?;
        self.ensure_writable_now(name, &node.access, io)?;
        let (address, len) = self.resolve_address(name, &node.addressing, io)?;
        // Build byte buffer with null termination
        let mut buf = vec![0u8; len as usize];
        let bytes = value.as_bytes();
        let copy_len = bytes.len().min(len as usize);
        buf[..copy_len].copy_from_slice(&bytes[..copy_len]);
        io.write(address, &buf)?;
        node.cache
            .replace(Some((value.to_string(), self.generation.get())));
        self.invalidate_dependents(name);
        debug!(node = %name, value = %value, "set_string");
        Ok(())
    }

    /// Read a `<Register>` node's bytes using the provided transport.
    ///
    /// Returns the full declared length. For a large block — the Micro-Epsilon
    /// scanCONTROL declares `FileAccessBuffer` as 100 000 bytes — that is
    /// hundreds of chunked reads; use [`NodeMap::register_address`] and the
    /// transport directly when a partial read is what you want.
    pub fn get_register(&self, name: &str, io: &dyn RegisterIo) -> Result<Vec<u8>, GenApiError> {
        let node = self.get_register_node(name)?;
        ensure_readable(&node.access, name)?;
        ensure_device_port(name, node.port.as_deref())?;
        if let Some((ref value, generation)) = *node.cache.borrow()
            && generation == self.generation.get()
        {
            return Ok(value.clone());
        }
        let (address, len) = self.resolve_address(name, &node.addressing, io)?;
        let raw = io.read(address, len as usize)?;
        node.cache
            .replace(Some((raw.clone(), self.generation.get())));
        debug!(node = %name, len = raw.len(), "get_register");
        Ok(raw)
    }

    /// Write a `<Register>` node's bytes using the provided transport.
    ///
    /// `data` must be exactly the declared length. Unlike [`NodeMap::set_string`],
    /// which pads with NULs, a short slice is refused: zero-padding a
    /// file-transfer buffer to 100 000 bytes because the caller supplied 12 is
    /// data loss, not a convenience.
    pub fn set_register(
        &self,
        name: &str,
        data: &[u8],
        io: &dyn RegisterIo,
    ) -> Result<(), GenApiError> {
        let node = self.get_register_node(name)?;
        self.ensure_writable_now(name, &node.access, io)?;
        ensure_device_port(name, node.port.as_deref())?;
        let (address, len) = self.resolve_address(name, &node.addressing, io)?;
        if data.len() != len as usize {
            return Err(GenApiError::Range(format!(
                "register '{name}' is {len} bytes; got {}",
                data.len()
            )));
        }
        io.write(address, data)?;
        node.cache
            .replace(Some((data.to_vec(), self.generation.get())));
        self.invalidate_dependents(name);
        debug!(node = %name, len = data.len(), "set_register");
        Ok(())
    }

    /// Evaluate a Converter in the read direction (`<FormulaFrom>`).
    fn evaluate_converter(
        &self,
        node: &ConverterNode,
        io: &dyn RegisterIo,
        stack: &mut HashSet<String>,
    ) -> Result<SkValue, GenApiError> {
        if !stack.insert(node.name.clone()) {
            stack.remove(&node.name);
            return Err(GenApiError::ExprEval {
                name: node.name.clone(),
                msg: "cyclic dependency".into(),
            });
        }
        let result = self.eval_formula(
            &node.name,
            &node.ast_from,
            &node.vars_from,
            &[],
            eval_mode(node.output),
            io,
            stack,
        );
        stack.remove(&node.name);
        let value = result?;
        debug!(node = %node.name, value = %value, "evaluate Converter");
        Ok(value)
    }

    /// Evaluate an IntConverter in the read direction (`<FormulaFrom>`).
    fn evaluate_int_converter(
        &self,
        node: &IntConverterNode,
        io: &dyn RegisterIo,
        stack: &mut HashSet<String>,
    ) -> Result<i64, GenApiError> {
        if !stack.insert(node.name.clone()) {
            stack.remove(&node.name);
            return Err(GenApiError::ExprEval {
                name: node.name.clone(),
                msg: "cyclic dependency".into(),
            });
        }
        let result = self.eval_formula(
            &node.name,
            &node.ast_from,
            &node.vars_from,
            &[],
            EvalMode::Integer,
            io,
            stack,
        );
        stack.remove(&node.name);
        let int_value = result?.as_i64();
        debug!(node = %node.name, int_value, "evaluate IntConverter");
        Ok(int_value)
    }
}

/// Build one runtime node from its declaration, recording the nodes it depends
/// on in `dependents`.
fn build_node(
    decl: NodeDecl,
    dependents: &mut HashMap<String, Vec<String>>,
) -> Result<(String, Node), GenApiError> {
    match decl {
        NodeDecl::Integer {
            name,
            meta,
            addressing,
            len,
            access,
            min,
            max,
            inc,
            unit,
            bitfield,
            sign,
            byte_order,
            selectors,
            selected_if,
            pvalue,
            p_max,
            p_min,
            value,
            predicates,
        } => {
            if let Some(ref addr) = addressing {
                register_addressing_dependency(dependents, &name, addr);
            }
            if let Some(ref pv) = pvalue {
                dependents.entry(pv.clone()).or_default().push(name.clone());
            }
            if let Some(ref pm) = p_max {
                dependents.entry(pm.clone()).or_default().push(name.clone());
            }
            if let Some(ref pm) = p_min {
                dependents.entry(pm.clone()).or_default().push(name.clone());
            }
            for (selector, _) in &selected_if {
                dependents
                    .entry(selector.clone())
                    .or_default()
                    .push(name.clone());
            }
            register_predicate_dependencies(dependents, &name, &predicates);
            let node = IntegerNode {
                name: name.clone(),
                meta,
                addressing,
                len,
                access,
                min,
                max,
                inc,
                unit,
                bitfield,
                sign,
                byte_order,
                selectors,
                selected_if,
                pvalue,
                p_max,
                p_min,
                value,
                predicates,
                cache: std::cell::RefCell::new(None),
                raw_cache: std::cell::RefCell::new(None),
            };
            Ok((name, Node::Integer(node)))
        }
        NodeDecl::Float {
            name,
            meta,
            addressing,
            access,
            min,
            max,
            unit,
            scale,
            offset,
            selectors,
            selected_if,
            pvalue,
            encoding,
            byte_order,
            predicates,
        } => {
            if let Some(ref addr) = addressing {
                register_addressing_dependency(dependents, &name, addr);
            }
            if let Some(ref pv) = pvalue {
                dependents.entry(pv.clone()).or_default().push(name.clone());
            }
            for (selector, _) in &selected_if {
                dependents
                    .entry(selector.clone())
                    .or_default()
                    .push(name.clone());
            }
            register_predicate_dependencies(dependents, &name, &predicates);
            let node = FloatNode {
                name: name.clone(),
                meta,
                addressing,
                access,
                min,
                max,
                unit,
                scale,
                offset,
                selectors,
                selected_if,
                pvalue,
                encoding,
                byte_order,
                predicates,
                cache: std::cell::RefCell::new(None),
            };
            Ok((name, Node::Float(node)))
        }
        NodeDecl::Enum {
            name,
            meta,
            addressing,
            access,
            entries,
            default,
            selectors,
            selected_if,
            pvalue,
            predicates,
        } => {
            if let Some(ref addr) = addressing {
                register_addressing_dependency(dependents, &name, addr);
            }
            if let Some(ref pv) = pvalue {
                dependents.entry(pv.clone()).or_default().push(name.clone());
            }
            for (selector, _) in &selected_if {
                dependents
                    .entry(selector.clone())
                    .or_default()
                    .push(name.clone());
            }
            register_predicate_dependencies(dependents, &name, &predicates);
            let mut providers = Vec::new();
            let mut provider_set = HashSet::new();
            for entry in &entries {
                if let EnumValueSrc::FromNode(node_name) = &entry.value {
                    dependents
                        .entry(node_name.clone())
                        .or_default()
                        .push(name.clone());
                    if provider_set.insert(node_name.clone()) {
                        providers.push(node_name.clone());
                    }
                }
                register_predicate_dependencies(dependents, &name, &entry.predicates);
            }
            providers.sort();
            let node = EnumNode {
                name: name.clone(),
                meta,
                addressing,
                access,
                pvalue,
                entries,
                default,
                selectors,
                selected_if,
                providers,
                predicates,
                value_cache: std::cell::RefCell::new(None),
                mapping_cache: std::cell::RefCell::new(None),
            };
            Ok((name, Node::Enum(node)))
        }
        NodeDecl::Boolean {
            name,
            meta,
            addressing,
            len,
            access,
            bitfield,
            selectors,
            selected_if,
            pvalue,
            on_value,
            off_value,
            predicates,
        } => {
            if let Some(ref addr) = addressing {
                register_addressing_dependency(dependents, &name, addr);
            }
            if let Some(ref pv) = pvalue {
                dependents.entry(pv.clone()).or_default().push(name.clone());
            }
            for (selector, _) in &selected_if {
                dependents
                    .entry(selector.clone())
                    .or_default()
                    .push(name.clone());
            }
            register_predicate_dependencies(dependents, &name, &predicates);
            let node = BooleanNode {
                name: name.clone(),
                meta,
                addressing,
                len,
                access,
                bitfield,
                selectors,
                selected_if,
                pvalue,
                on_value,
                off_value,
                predicates,
                cache: std::cell::RefCell::new(None),
                raw_cache: std::cell::RefCell::new(None),
            };
            Ok((name, Node::Boolean(node)))
        }
        NodeDecl::Command {
            name,
            meta,
            address,
            len,
            pvalue,
            command_value,
            predicates,
        } => {
            if let Some(ref pv) = pvalue {
                dependents.entry(pv.clone()).or_default().push(name.clone());
            }
            register_predicate_dependencies(dependents, &name, &predicates);
            let node = CommandNode {
                name: name.clone(),
                meta,
                address,
                len,
                pvalue,
                command_value,
                predicates,
            };
            Ok((name, Node::Command(node)))
        }
        NodeDecl::Category {
            name,
            meta,
            children,
            predicates,
        } => {
            register_predicate_dependencies(dependents, &name, &predicates);
            let node = CategoryNode {
                name: name.clone(),
                meta,
                children,
                predicates,
            };
            Ok((name, Node::Category(node)))
        }
        NodeDecl::SwissKnife(decl) => {
            let name = decl.name;
            let meta = decl.meta;
            let expr = decl.expr;
            let variables = decl.variables;
            let output = decl.output;
            let predicates = decl.predicates;
            let mut ast = parse_expression(&expr).map_err(|err| GenApiError::ExprParse {
                name: name.clone(),
                msg: err.to_string(),
            })?;
            substitute(&mut ast, &formula_bindings(&name, &decl.bindings)?);
            let vars = formula_vars(&name, &variables, &[&ast])?;
            let mut used = HashSet::new();
            collect_identifiers(&ast, &mut used);
            for ident in &used {
                // `E` and `PI` are language constants, not variables, so
                // they legitimately appear without a `<pVariable>`.
                if !vars.iter().any(|var| var.name == *ident) && !is_builtin_constant(ident) {
                    return Err(GenApiError::UnknownVariable {
                        name: name.clone(),
                        var: ident.clone(),
                    });
                }
            }
            register_formula_dependencies(
                dependents,
                &name,
                vars.iter().map(|var| var.provider.as_str()),
            );
            register_predicate_dependencies(dependents, &name, &predicates);
            let node = SkNode {
                name: name.clone(),
                meta,
                output,
                ast,
                vars,
                predicates,
                cache: std::cell::RefCell::new(None),
            };
            Ok((name, Node::SwissKnife(node)))
        }
        NodeDecl::Converter(decl) => {
            let name = decl.name;
            let bindings = formula_bindings(&name, &decl.bindings)?;
            let mut ast_to =
                parse_expression(&decl.formula_to).map_err(|err| GenApiError::ExprParse {
                    name: name.clone(),
                    msg: format!("FormulaTo: {err}"),
                })?;
            substitute(&mut ast_to, &bindings);
            let mut ast_from =
                parse_expression(&decl.formula_from).map_err(|err| GenApiError::ExprParse {
                    name: name.clone(),
                    msg: format!("FormulaFrom: {err}"),
                })?;
            substitute(&mut ast_from, &bindings);
            let vars_to = formula_vars(&name, &decl.variables_to, &[&ast_to])?;
            let vars_from = formula_vars(&name, &decl.variables_from, &[&ast_from])?;
            register_formula_dependencies(
                dependents,
                &name,
                vars_to
                    .iter()
                    .chain(&vars_from)
                    .map(|var| var.provider.as_str())
                    .chain([decl.p_value.as_str()]),
            );
            register_predicate_dependencies(dependents, &name, &decl.predicates);
            let node = ConverterNode {
                name: name.clone(),
                meta: decl.meta,
                p_value: decl.p_value,
                ast_to,
                ast_from,
                vars_to,
                vars_from,
                unit: decl.unit,
                output: decl.output,
                predicates: decl.predicates,
                cache: std::cell::RefCell::new(None),
            };
            Ok((name, Node::Converter(node)))
        }
        NodeDecl::IntConverter(decl) => {
            let name = decl.name;
            let bindings = formula_bindings(&name, &decl.bindings)?;
            let mut ast_to =
                parse_expression(&decl.formula_to).map_err(|err| GenApiError::ExprParse {
                    name: name.clone(),
                    msg: format!("FormulaTo: {err}"),
                })?;
            substitute(&mut ast_to, &bindings);
            let mut ast_from =
                parse_expression(&decl.formula_from).map_err(|err| GenApiError::ExprParse {
                    name: name.clone(),
                    msg: format!("FormulaFrom: {err}"),
                })?;
            substitute(&mut ast_from, &bindings);
            let vars_to = formula_vars(&name, &decl.variables_to, &[&ast_to])?;
            let vars_from = formula_vars(&name, &decl.variables_from, &[&ast_from])?;
            register_formula_dependencies(
                dependents,
                &name,
                vars_to
                    .iter()
                    .chain(&vars_from)
                    .map(|var| var.provider.as_str())
                    .chain([decl.p_value.as_str()]),
            );
            register_predicate_dependencies(dependents, &name, &decl.predicates);
            let node = IntConverterNode {
                name: name.clone(),
                meta: decl.meta,
                p_value: decl.p_value,
                ast_to,
                ast_from,
                vars_to,
                vars_from,
                unit: decl.unit,
                predicates: decl.predicates,
                cache: std::cell::RefCell::new(None),
            };
            Ok((name, Node::IntConverter(node)))
        }
        NodeDecl::String(decl) => {
            let name = decl.name;
            register_addressing_dependency(dependents, &name, &decl.addressing);
            register_predicate_dependencies(dependents, &name, &decl.predicates);
            let node = StringNode {
                name: name.clone(),
                meta: decl.meta,
                addressing: decl.addressing,
                access: decl.access,
                predicates: decl.predicates,
                cache: std::cell::RefCell::new(None),
            };
            Ok((name, Node::String(node)))
        }
        NodeDecl::Register(decl) => {
            let name = decl.name;
            register_addressing_dependency(dependents, &name, &decl.addressing);
            register_predicate_dependencies(dependents, &name, &decl.predicates);
            let node = RegisterNode {
                name: name.clone(),
                meta: decl.meta,
                addressing: decl.addressing,
                access: decl.access,
                port: decl.port,
                predicates: decl.predicates,
                cache: std::cell::RefCell::new(None),
            };
            Ok((name, Node::Register(node)))
        }
        // `NodeDecl` is `#[non_exhaustive]`, so this crate can no longer match
        // it exhaustively and the compiler will not point at this function when
        // a variant is added. Fail loudly rather than defaulting: a node type
        // the XML layer understands but this one silently drops is precisely
        // the class of defect GA-02 existed to end.
        other => Err(GenApiError::Unsupported(format!(
            "node '{}' has declaration kind '{}', which this nodemap cannot build; \
             viva-genapi-xml understands it but viva-genapi has no arm for it",
            other.name(),
            other.kind()
        ))),
    }
}

/// Effective signedness of an integer node's payload.
///
/// `<Sign>` is the only signal. `<Min>` says nothing about it: it constrains
/// the *feature* value, while `<Sign>` describes the *register payload*
/// encoding, and the two live on different nodes when a feature delegates
/// through `<pValue>`.
///
/// Inferring "signed" from a negative `<Min>` looks reasonable and is
/// unsalvageable in practice. `<Min>` is optional and defaults to `i64::MIN`,
/// which is negative — and across the whole vendor corpus **not one** of the
/// 4 300 `<IntReg>` or 5 479 `<MaskedIntReg>` nodes declares it. The
/// inference therefore fires on every register-backed integer on every real
/// camera, which is precisely the case `<Sign>` exists to decide.
fn integer_sign(node: &IntegerNode) -> Sign {
    node.sign
}

/// Resolve the variables a formula reads from its `<pVariable>` declarations.
///
/// Every declaration becomes a [`FormulaVar`]; one whose `Name` carries an
/// extension — the standard's own example is
/// `<pVariable Name="Gain.Max">Gain</pVariable>` — reads that property of its
/// node rather than its value. An identifier in `asts` that is not declared as
/// written but qualifies a declared plain variable (`V1.Min` with `V1`
/// declared) is added too. Identifiers that match neither are left out; the
/// caller decides whether that is an error.
///
/// An extension the standard does not define fails the build with
/// [`GenApiError::UnknownVariableExtension`], which sends the node to
/// [`NodeMap::skipped`] rather than evaluating it to a wrong number.
fn formula_vars(
    node: &str,
    declared: &[(String, String)],
    asts: &[&SkAst],
) -> Result<Vec<FormulaVar>, GenApiError> {
    let qualified = |var: &str, extension: &str, provider: &str| {
        VarAttr::parse(extension)
            .map(|attr| FormulaVar {
                name: var.to_string(),
                provider: provider.to_string(),
                attr,
            })
            .ok_or_else(|| GenApiError::UnknownVariableExtension {
                name: node.to_string(),
                var: var.to_string(),
                extension: extension.to_string(),
            })
    };
    let mut vars = Vec::with_capacity(declared.len());
    for (var, provider) in declared {
        vars.push(match split_qualified(var) {
            Some((_, extension)) => qualified(var, extension, provider)?,
            None => FormulaVar {
                name: var.clone(),
                provider: provider.clone(),
                attr: VarAttr::Value,
            },
        });
    }
    let mut used = HashSet::new();
    for ast in asts {
        collect_identifiers(ast, &mut used);
    }
    let mut used: Vec<String> = used.into_iter().collect();
    used.sort();
    for ident in used {
        if declared.iter().any(|(var, _)| *var == ident) {
            continue;
        }
        let Some((base, extension)) = split_qualified(&ident) else {
            continue;
        };
        if let Some((_, provider)) = declared.iter().find(|(var, _)| var == base) {
            vars.push(qualified(&ident, extension, provider)?);
        }
    }
    Ok(vars)
}

/// Record `node` as a dependent of every distinct provider its formula reads.
fn register_formula_dependencies<'a>(
    dependents: &mut HashMap<String, Vec<String>>,
    node: &str,
    providers: impl IntoIterator<Item = &'a str>,
) {
    let mut seen = HashSet::new();
    for provider in providers {
        if seen.insert(provider) {
            dependents
                .entry(provider.to_string())
                .or_default()
                .push(node.to_string());
        }
    }
}

/// Resolve a formula's `<Constant>` and named `<Expression>` declarations into
/// sub-ASTs ready for substitution.
///
/// Declaration order is significant: an `<Expression>` may reference constants
/// and expressions declared before it, so each is substituted against what has
/// been bound so far. That also makes a self- or forward-reference resolve to
/// nothing rather than looping.
fn formula_bindings(
    node: &str,
    declared: &FormulaBindings,
) -> Result<HashMap<String, SkAst>, GenApiError> {
    let mut bindings: HashMap<String, SkAst> = HashMap::new();
    for (name, literal) in &declared.constants {
        let value = parse_expression(literal).map_err(|err| GenApiError::ExprParse {
            name: node.to_string(),
            msg: format!("Constant {name}: {err}"),
        })?;
        bindings.insert(name.clone(), value);
    }
    for (name, formula) in &declared.expressions {
        let mut ast = parse_expression(formula).map_err(|err| GenApiError::ExprParse {
            name: node.to_string(),
            msg: format!("Expression {name}: {err}"),
        })?;
        substitute(&mut ast, &bindings);
        bindings.insert(name.clone(), ast);
    }
    Ok(bindings)
}

/// Narrow a formula result to `i64` for an integer-typed feature.
///
/// An integer formula that stayed integral is exact; one that picked up a
/// float along the way (a fractional literal, a transcendental function)
/// rounds, as the GenApi integer output rule requires.
fn sk_to_i64(name: &str, value: SkValue) -> Result<i64, GenApiError> {
    match value {
        SkValue::Int(value) => Ok(value),
        SkValue::Float(value) => round_to_i64(name, value),
    }
}

/// Arithmetic mode implied by a formula's declared output type.
fn eval_mode(output: SkOutput) -> EvalMode {
    match output {
        SkOutput::Integer => EvalMode::Integer,
        SkOutput::Float => EvalMode::Float,
    }
}

/// Map a formula evaluation failure onto the public error type.
fn expr_error(name: &str, err: SkEvalError) -> GenApiError {
    match err {
        SkEvalError::UnknownVariable(var) => GenApiError::UnknownVariable {
            name: name.to_string(),
            var,
        },
        SkEvalError::DivisionByZero => GenApiError::ExprEval {
            name: name.to_string(),
            msg: "division by zero".into(),
        },
        SkEvalError::UnknownFunction(func) => GenApiError::ExprEval {
            name: name.to_string(),
            msg: format!("unknown function: {func}"),
        },
        SkEvalError::ArityMismatch {
            name: func,
            expected,
            got,
        } => GenApiError::ExprEval {
            name: name.to_string(),
            msg: format!("function {func} expects {expected} args, got {got}"),
        },
    }
}

impl TryFrom<XmlModel> for NodeMap {
    type Error = GenApiError;

    fn try_from(model: XmlModel) -> Result<Self, Self::Error> {
        NodeMap::try_from_xml(model)
    }
}

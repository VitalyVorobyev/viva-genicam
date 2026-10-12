//! Saving a camera's configuration and restoring it: GenICam's `<Streamable>`
//! features (ADR-0028).
//!
//! GenICam Standard v2.1.1 §2.8.1 marks a feature `<Streamable>` when it "is
//! prepared to be stored to and loaded from a file via the GenApi node tree.
//! The idea is to persist the state of a camera by storing the features marked
//! as Streamable and restore the state by writing those features back to the
//! node tree." This module is that idea, in three parts:
//!
//! - [`NodeMap::persistence_plan`] decides from the XML alone which features a
//!   configuration holds, which selectors each is saved under, and the order a
//!   restore writes them in;
//! - [`NodeMap::save_configuration`] reads them from the device into a
//!   [`Configuration`], and [`NodeMap::load_configuration`] writes one back,
//!   reporting every value it could not write;
//! - [`Configuration::to_text`] and [`Configuration::parse`] are the file
//!   format: the text format of GenApi's reference implementation
//!   (`CFeatureBag`), chosen in ADR-0028.
//!
//! Nothing here touches a file or a socket, so it builds for wasm.

use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap, HashMap, HashSet};
use std::fmt;

use tracing::warn;
use viva_genapi_xml::{AccessMode, DocumentIdentity};

use crate::nodemap::Limit;
use crate::nodes::Node;
use crate::text::{bytes_to_hex, hex_to_bytes};
use crate::{GenApiError, NodeMap, RegisterIo};

/// The first line of every file: the magic GUID that GenApi's reference
/// implementation (`Persistence.h`, `GENAPI_PERSISTENCE_MAGIC`) writes and
/// requires.
pub const FILE_MAGIC: &str = "{05D8C294-F295-4dfb-9D01-096BD04049F4}";

/// The magic of the reference implementation's *other* format,
/// `CFeatureBagger`, which this module does not read.
const BAGGER_MAGIC: &str = "{4709CB3C-7322-4460-84C3-DA11DDA09939}";

/// How many values one feature may be saved under, counting every
/// combination of its selectors' entries. A bound, so that a save always ends:
/// an integer selector may declare no useful maximum.
pub const MAX_SELECTOR_COMBINATIONS: usize = 65_536;

/// One feature as a configuration holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedFeature {
    /// The feature.
    pub name: String,
    /// The selectors whose entries the feature is saved under, outermost
    /// first: a selector that selects another comes before it. Empty for a
    /// feature no selector selects.
    pub selectors: Vec<String>,
    /// The features in the plan that a restore writes before this one,
    /// because writing them can change this one: its selector, the provider
    /// of its `pIsAvailable`/`pIsLocked`, a feature its bounds or address are
    /// computed from. Features that depend on each other in a cycle — `Width`
    /// and `OffsetX` bound each other — keep document order between them.
    pub after: Vec<String>,
}

/// What a configuration holds and the order a restore writes it in, decided
/// from the XML alone. See [`NodeMap::persistence_plan`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PersistencePlan {
    features: Vec<PlannedFeature>,
}

impl PersistencePlan {
    /// The streamable features, in restore order.
    pub fn features(&self) -> &[PlannedFeature] {
        &self.features
    }

    /// Where `name` comes in the restore order, if the plan holds it.
    pub fn position(&self, name: &str) -> Option<usize> {
        self.features.iter().position(|f| f.name == name)
    }
}

/// One value of a configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedValue {
    /// The feature.
    pub feature: String,
    /// The selector entries the value belongs to, outermost first, as
    /// `(selector, entry)`; empty for a feature no selector selects.
    pub selectors: Vec<(String, String)>,
    /// The value as text: an integer in decimal, a float in the shortest form
    /// that reads back to the same value, an enumeration entry by name, a
    /// boolean as `1`/`0`, a register as `0x` and hexadecimal bytes.
    pub value: String,
}

/// A feature a save left out, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Omission {
    /// The feature.
    pub feature: String,
    /// The selector entries, when only some of the feature's values were
    /// left out.
    pub selectors: Vec<(String, String)>,
    /// Why: "not available", "locked by ExposureAuto", ...
    pub reason: String,
}

/// A camera configuration: the streamable features' values, and the camera
/// they were read from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Configuration {
    /// The camera the configuration was saved from, as its XML names it.
    /// Empty fields when a file does not say.
    pub identity: DocumentIdentity,
    /// The values, in the order a restore writes them.
    pub values: Vec<SavedValue>,
    /// What a save left out. Written to a file as comments, so a reader sees
    /// why a feature is missing; not read back.
    pub omitted: Vec<Omission>,
}

/// A configuration file that could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("line {line}: {message}")]
pub struct ParseConfigurationError {
    /// 1-based line number.
    pub line: usize,
    /// What was wrong with it.
    pub message: String,
}

/// A value [`NodeMap::load_configuration`] could not write.
#[derive(Debug)]
pub struct LoadFailure {
    /// The value, as the configuration holds it.
    pub value: SavedValue,
    /// Why the write failed, after the second pass.
    pub error: GenApiError,
}

/// What [`NodeMap::load_configuration`] did.
#[derive(Debug, Default)]
pub struct LoadReport {
    /// Values written.
    pub written: usize,
    /// Of [`LoadReport::written`], those written only on the second pass,
    /// once the values they depend on were in place.
    pub written_on_retry: usize,
    /// Values that could not be written, each with its reason.
    pub failures: Vec<LoadFailure>,
    /// Doubts about the configuration as a whole: saved from another camera
    /// model, or from another version of its XML.
    pub warnings: Vec<String>,
}

impl LoadReport {
    /// Whether every value was written.
    pub fn is_complete(&self) -> bool {
        self.failures.is_empty()
    }
}

impl fmt::Display for SavedValue {
    /// `Gain[GainSelector=DigitalAll] = 9`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.feature)?;
        write_selectors(f, &self.selectors)?;
        write!(f, " = {}", self.value)
    }
}

impl fmt::Display for LoadFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.value, self.error)
    }
}

/// `[Selector=Entry, ...]`, or nothing for no selectors.
fn write_selectors(f: &mut impl fmt::Write, selectors: &[(String, String)]) -> fmt::Result {
    if selectors.is_empty() {
        return Ok(());
    }
    f.write_char('[')?;
    for (i, (selector, entry)) in selectors.iter().enumerate() {
        if i > 0 {
            f.write_str(", ")?;
        }
        write!(f, "{selector}={entry}")?;
    }
    f.write_char(']')
}

/// The comment keys that carry [`Configuration::identity`].
const KEY_VENDOR: &str = "VendorName";
const KEY_MODEL: &str = "ModelName";
const KEY_VERSION: &str = "DeviceXmlVersion";

impl Configuration {
    /// The configuration as a file, in the layout GenApi's reference
    /// implementation writes (`CFeatureBag`, ADR-0028):
    ///
    /// ```text
    /// # {05D8C294-F295-4dfb-9D01-096BD04049F4}
    /// # viva-genicam configuration
    /// # VendorName = Basler
    /// # ModelName = acA1600-20gm
    /// # DeviceXmlVersion = 3.2.1
    /// # not saved: ExposureTime: locked by ExposureAuto
    /// GainSelector<TAB>AnalogAll
    /// Gain<TAB>{GainSelector=DigitalAll}<TAB>9
    /// ```
    ///
    /// One value per line, the feature name and the value separated by a tab
    /// (`<TAB>` above);
    /// a value read under selectors puts them between the two, in braces.
    /// Lines starting with `#` are comments: the camera's identity, and what
    /// the save left out and why.
    pub fn to_text(&self) -> String {
        use fmt::Write as _;
        let mut out = String::new();
        let _ = writeln!(out, "# {FILE_MAGIC}");
        let _ = writeln!(out, "# viva-genicam configuration");
        let _ = writeln!(out, "# {KEY_VENDOR} = {}", self.identity.vendor_name);
        let _ = writeln!(out, "# {KEY_MODEL} = {}", self.identity.model_name);
        let _ = writeln!(out, "# {KEY_VERSION} = {}", self.identity.version);
        for omission in &self.omitted {
            let _ = write!(out, "# not saved: {}", omission.feature);
            let _ = write_selectors(&mut out, &omission.selectors);
            let _ = writeln!(out, ": {}", omission.reason);
        }
        for value in &self.values {
            out.push_str(&value.feature);
            out.push('\t');
            if !value.selectors.is_empty() {
                out.push('{');
                for (i, (selector, entry)) in value.selectors.iter().enumerate() {
                    if i > 0 {
                        out.push('\t');
                    }
                    let _ = write!(out, "{selector}={entry}");
                }
                out.push_str("}\t");
            }
            out.push_str(&value.value);
            out.push('\n');
        }
        out
    }

    /// Read a configuration file: ours, or one in the same layout from
    /// another GenApi-based tool.
    ///
    /// Blank lines and `#` comments are skipped, and line endings may be
    /// `\n` or `\r\n`. The camera's identity is read from the comments this
    /// crate writes; a file without them has an empty
    /// [`Configuration::identity`]. A file of GenApi's other format
    /// (`CFeatureBagger`) is refused, as is a line with no value.
    pub fn parse(text: &str) -> Result<Self, ParseConfigurationError> {
        let mut configuration = Configuration::default();
        for (index, line) in text.lines().enumerate() {
            let error = |message: &str| ParseConfigurationError {
                line: index + 1,
                message: message.to_string(),
            };
            if line.trim().is_empty() {
                continue;
            }
            if let Some(comment) = line.strip_prefix('#') {
                let comment = comment.trim();
                if comment.contains(BAGGER_MAGIC) {
                    return Err(error(
                        "written by GenApi's CFeatureBagger, a format this reader does not read",
                    ));
                }
                if let Some((key, value)) = comment.split_once('=') {
                    let value = value.trim().to_string();
                    match key.trim() {
                        KEY_VENDOR => configuration.identity.vendor_name = value,
                        KEY_MODEL => configuration.identity.model_name = value,
                        KEY_VERSION => configuration.identity.version = value,
                        _ => {}
                    }
                }
                continue;
            }
            let (name, rest) = line
                .split_once('\t')
                .ok_or_else(|| error("expected a feature name, a tab and a value"))?;
            let name = name.trim();
            if name.is_empty() {
                return Err(error("no feature name before the tab"));
            }
            let (selectors, value) = match rest.strip_prefix('{') {
                Some(group) => {
                    let (inner, after) = group
                        .split_once('}')
                        .ok_or_else(|| error("a selector group without its closing brace"))?;
                    let value = after
                        .strip_prefix('\t')
                        .ok_or_else(|| error("expected a tab after the selector group"))?;
                    let selectors = inner
                        .split('\t')
                        .map(|pair| {
                            pair.split_once('=')
                                .map(|(s, e)| (s.trim().to_string(), e.trim().to_string()))
                                .filter(|(s, e)| !s.is_empty() && !e.is_empty())
                                .ok_or_else(|| error("expected Selector=Entry in the group"))
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    (selectors, value)
                }
                None => (Vec::new(), rest),
            };
            if value.is_empty() {
                return Err(error("no value after the tab"));
            }
            configuration.values.push(SavedValue {
                feature: name.to_string(),
                selectors,
                value: value.to_string(),
            });
        }
        Ok(configuration)
    }
}

impl NodeMap {
    /// Which features a configuration holds, under which selectors, and the
    /// order a restore writes them in — decided from the XML alone.
    ///
    /// The features are those the XML marks `<Streamable>Yes</Streamable>`,
    /// except categories and commands, which have no value. A feature is
    /// written after every feature in the plan whose write can change it —
    /// whatever the dependency graph links from the nodes the first is
    /// written through to the second: a `<pSelected>`, a `pIsAvailable` or
    /// `pIsLocked` provider, a `<pMin>`/`<pMax>`, an address, a
    /// `<pInvalidator>`. Features that depend on one another in a cycle keep
    /// document order between them, and otherwise the plan follows document
    /// order too.
    pub fn persistence_plan(&self) -> PersistencePlan {
        let names: Vec<&str> = self
            .node_names()
            .filter(|name| {
                self.node(name).is_some_and(|node| {
                    node.meta().streamable && !matches!(node, Node::Category(_) | Node::Command(_))
                })
            })
            .collect();
        let index: HashMap<&str, usize> = names.iter().enumerate().map(|(i, n)| (*n, i)).collect();

        // successors[p] holds every f that p's write can change.
        let mut successors: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); names.len()];
        for (p, name) in names.iter().enumerate() {
            let mut seen: HashSet<&str> = HashSet::new();
            let mut pending: Vec<&str> = self.value_chain(name);
            while let Some(current) = pending.pop() {
                if !seen.insert(current) {
                    continue;
                }
                for dependent in self.dependents(current) {
                    match index.get(dependent.as_str()) {
                        // Stop at a planned feature: its own edges carry on.
                        Some(&f) => {
                            if f != p {
                                successors[p].insert(f);
                            }
                        }
                        None => pending.push(dependent),
                    }
                }
            }
        }

        let order = restore_order(&successors);
        let mut predecessors: Vec<Vec<String>> = vec![Vec::new(); names.len()];
        for (p, targets) in successors.iter().enumerate() {
            for &f in targets {
                predecessors[f].push(names[p].to_string());
            }
        }
        let features = order
            .into_iter()
            .map(|i| PlannedFeature {
                name: names[i].to_string(),
                selectors: self.selectors_outermost_first(names[i]),
                after: std::mem::take(&mut predecessors[i]),
            })
            .collect();
        PersistencePlan { features }
    }

    /// The selectors of `name` — those that select it, and the selectors of
    /// those in turn — a selector that selects another (directly or through
    /// others) before it, document order otherwise.
    ///
    /// A selector of a selector matters as much as a direct one: when
    /// `LUTSelector` selects `LUTIndex` and `LUTIndex` selects `LUTValue`,
    /// `LUTValue` read under each `LUTIndex` differs for each `LUTSelector`
    /// entry too, though `LUTSelector` does not name it (Codex review of
    /// #200).
    fn selectors_outermost_first(&self, name: &str) -> Vec<String> {
        let mut selectors: Vec<&str> = Vec::new();
        let mut pending: Vec<&str> = self.selectors_of(name);
        while let Some(selector) = pending.pop() {
            if selector == name || selectors.contains(&selector) {
                continue;
            }
            selectors.push(selector);
            pending.extend(self.selectors_of(selector));
        }
        let position: HashMap<&str, usize> = self
            .node_names()
            .enumerate()
            .map(|(i, node)| (node, i))
            .collect();
        selectors.sort_by_key(|selector| position.get(selector).copied());
        let selects = |outer: &str, inner: &str| -> bool {
            let mut seen: HashSet<&str> = HashSet::new();
            let mut pending: Vec<&str> = vec![outer];
            while let Some(current) = pending.pop() {
                if !seen.insert(current) {
                    continue;
                }
                for selected in self.node(current).map(Node::selected).unwrap_or(&[]) {
                    if selected == inner {
                        return true;
                    }
                    pending.push(selected);
                }
            }
            false
        };
        let mut ranked: Vec<(usize, &str)> = selectors
            .iter()
            .map(|s| {
                let depth = selectors
                    .iter()
                    .filter(|t| *t != s && selects(t, s))
                    .count();
                (depth, *s)
            })
            .collect();
        ranked.sort_by_key(|(depth, _)| *depth);
        ranked.into_iter().map(|(_, s)| s.to_string()).collect()
    }

    /// Read every streamable feature from the device: each selected one
    /// under every entry of its selectors, which are put back afterwards.
    ///
    /// A feature is left out, with the reason in [`Configuration::omitted`],
    /// when it cannot be both read and written — declared read-only or
    /// write-only — or when, now, it is not implemented, not available,
    /// locked, or its read fails. A locked feature would not be written on
    /// restore either: what locks it (`ExposureAuto` for `ExposureTime`) is
    /// saved and restored first. One that `TLParamsLocked` locks is left out
    /// while the camera acquires, so save while it does not.
    pub fn save_configuration(&mut self, io: &dyn RegisterIo) -> Configuration {
        let plan = self.persistence_plan();
        let mut saved = Configuration {
            identity: self.identity().clone(),
            ..Configuration::default()
        };
        for feature in plan.features() {
            if let Some(reason) = self.not_persistable(&feature.name) {
                saved.omitted.push(Omission {
                    feature: feature.name.clone(),
                    selectors: Vec::new(),
                    reason: reason.to_string(),
                });
                continue;
            }
            let mut walk = SelectorWalk {
                feature: &feature.name,
                selectors: &feature.selectors,
                context: Vec::new(),
                values: Vec::new(),
                omitted: Vec::new(),
                reads: 0,
                abandoned: None,
            };
            self.walk_selectors(&mut walk, io);
            match walk.abandoned {
                Some(reason) => saved.omitted.push(Omission {
                    feature: feature.name.clone(),
                    selectors: Vec::new(),
                    reason,
                }),
                None => {
                    saved.values.append(&mut walk.values);
                    saved.omitted.append(&mut walk.omitted);
                }
            }
        }
        saved
    }

    /// Why `name` cannot be saved whatever the device's state: a value that
    /// cannot be written back, or not read.
    fn not_persistable(&self, name: &str) -> Option<&'static str> {
        match self.declared_access_mode(name) {
            Some(AccessMode::RW) => None,
            Some(AccessMode::RO) => Some("read-only"),
            Some(AccessMode::WO) => Some("write-only"),
            None => Some("neither readable nor writable"),
        }
    }

    /// Set each of `walk.selectors` from `walk.context.len()` on to each of
    /// its entries in turn, read the feature under every combination, and
    /// put each selector back as it was.
    fn walk_selectors(&mut self, walk: &mut SelectorWalk<'_>, io: &dyn RegisterIo) {
        let depth = walk.context.len();
        let Some(selector) = walk.selectors.get(depth) else {
            walk.reads += 1;
            if walk.reads > MAX_SELECTOR_COMBINATIONS {
                walk.abandoned = Some(format!(
                    "more than {MAX_SELECTOR_COMBINATIONS} combinations of selector entries"
                ));
                return;
            }
            match self.read_for_saving(walk.feature, io) {
                Ok(value) => walk.values.push(SavedValue {
                    feature: walk.feature.to_string(),
                    selectors: walk.context.clone(),
                    value,
                }),
                Err(reason) => walk.omitted.push(Omission {
                    feature: walk.feature.to_string(),
                    selectors: walk.context.clone(),
                    reason,
                }),
            }
            return;
        };
        let original = match self.get_text(selector, io) {
            Ok(value) => value,
            Err(err) => {
                walk.omitted.push(Omission {
                    feature: walk.feature.to_string(),
                    selectors: walk.context.clone(),
                    reason: format!("its selector {selector} cannot be read: {err}"),
                });
                return;
            }
        };
        let entries = match self.selector_entries(selector, io) {
            Ok(entries) => entries,
            Err(reason) => {
                walk.abandoned = Some(reason);
                return;
            }
        };
        for entry in entries {
            if walk.abandoned.is_some() {
                break;
            }
            if let Err(err) = self.set_text(selector, &entry, io) {
                let mut selectors = walk.context.clone();
                selectors.push((selector.clone(), entry.clone()));
                walk.omitted.push(Omission {
                    feature: walk.feature.to_string(),
                    selectors,
                    reason: format!("its selector cannot be set to {entry}: {err}"),
                });
                continue;
            }
            walk.context.push((selector.clone(), entry));
            self.walk_selectors(walk, io);
            walk.context.pop();
        }
        if let Err(err) = self.set_text(selector, &original, io) {
            warn!(%selector, %original, error = %err, "could not put the selector back");
        }
    }

    /// The entries a save iterates a selector over: an enumeration's
    /// available entries, an integer's values from its minimum to its maximum
    /// by its increment.
    fn selector_entries(&self, selector: &str, io: &dyn RegisterIo) -> Result<Vec<String>, String> {
        match self.node(selector) {
            Some(Node::Enum(_)) => self
                .available_enum_entries(selector, io)
                .map_err(|err| format!("the entries of its selector {selector}: {err}")),
            Some(Node::Integer(_)) => {
                let bound = |which: Limit| {
                    self.limit(selector, which, io, &mut HashSet::new())
                        .map(|value| value.map(|v| v.as_i64()))
                        .map_err(|err| format!("the bounds of its selector {selector}: {err}"))
                };
                let (Some(min), Some(max)) = (bound(Limit::Min)?, bound(Limit::Max)?) else {
                    return Err(format!("its selector {selector} declares no bounds"));
                };
                let inc = bound(Limit::Inc)?.filter(|inc| *inc > 0).unwrap_or(1);
                let count = if max < min {
                    0
                } else {
                    (max as i128 - min as i128) / inc as i128 + 1
                };
                if count > MAX_SELECTOR_COMBINATIONS as i128 {
                    return Err(format!(
                        "its selector {selector} has {count} values, more than the \
                         {MAX_SELECTOR_COMBINATIONS} a configuration saves"
                    ));
                }
                Ok((0..count as i64)
                    .map(|k| (min + k * inc).to_string())
                    .collect())
            }
            _ => Err(format!(
                "its selector {selector} is neither an enumeration nor an integer"
            )),
        }
    }

    /// Read `name` as a configuration holds it, or say why not.
    fn read_for_saving(&self, name: &str, io: &dyn RegisterIo) -> Result<String, String> {
        match self.is_implemented(name, io) {
            Ok(true) => {}
            Ok(false) => return Err("not implemented".into()),
            Err(err) => return Err(format!("pIsImplemented: {err}")),
        }
        match self.is_available(name, io) {
            Ok(true) => {}
            Ok(false) => return Err("not available".into()),
            Err(err) => return Err(format!("pIsAvailable: {err}")),
        }
        match self.locked_by(name, io) {
            Ok(None) => {}
            Ok(Some(provider)) => return Err(format!("locked by {provider}")),
            Err(err) => return Err(format!("pIsLocked: {err}")),
        }
        let value = match self.node(name) {
            Some(Node::Register(_)) => self.get_register(name, io).map(|b| bytes_to_hex(&b)),
            Some(Node::Boolean(_)) => self
                .get_bool(name, io)
                .map(|on| if on { "1" } else { "0" }.to_string()),
            _ => self.get_text(name, io),
        }
        .map_err(|err| format!("read failed: {err}"))?;
        if value.contains(['\n', '\r']) {
            return Err("its value has a line break, which the file cannot hold".into());
        }
        // GenApi's reader refuses a line with no value, so a file holding one
        // could not be loaded there.
        if value.is_empty() {
            return Err("empty, and the file has no way to hold an empty value".into());
        }
        Ok(value)
    }

    /// Write a configuration back to the device.
    ///
    /// Values are written in the plan's order, whatever their order in
    /// `configuration`: a feature after the selectors, availability and lock
    /// providers and bounds it depends on. A value saved under selectors is
    /// written with each selector set to its entry, and the selectors are put
    /// back afterwards. A file of plain lines that sets a selector on one line
    /// and a selected feature on the next — a replay of writes, as some tools
    /// save — binds each such value to the selector line before it.
    ///
    /// A write that fails is tried once more after all the others, for the
    /// failures that are only a matter of order: `Width` and `OffsetX` bound
    /// each other, so one of the two may have to wait. Whatever still fails
    /// is in [`LoadReport::failures`] with its reason, in the configuration's
    /// order; nothing stops the load early. A configuration saved from
    /// another camera model, or another major version of its XML, is loaded
    /// all the same, with a warning in [`LoadReport::warnings`].
    ///
    /// `TLParamsLocked` is not touched: the standard does not ask for it, and
    /// a camera that acquires reports the features it locks as failures.
    pub fn load_configuration(
        &mut self,
        configuration: &Configuration,
        io: &dyn RegisterIo,
    ) -> LoadReport {
        let mut report = LoadReport {
            warnings: identity_warnings(&configuration.identity, self.identity()),
            ..LoadReport::default()
        };
        let plan = self.persistence_plan();
        let values = self.bind_replayed_selectors(&configuration.values, &plan);
        let mut ordered: Vec<(usize, SavedValue)> = values.into_iter().enumerate().collect();
        ordered.sort_by_key(|(index, value)| {
            (plan.position(&value.feature).unwrap_or(usize::MAX), *index)
        });

        let mut retry = Vec::new();
        let mut failed = Vec::new();
        for (index, value) in ordered {
            match self.write_saved(&value, io) {
                Ok(()) => report.written += 1,
                Err(err) if is_permanent(&err) => failed.push((index, value, err)),
                Err(_) => retry.push((index, value)),
            }
        }
        for (index, value) in retry {
            match self.write_saved(&value, io) {
                Ok(()) => {
                    report.written += 1;
                    report.written_on_retry += 1;
                }
                Err(err) => failed.push((index, value, err)),
            }
        }
        failed.sort_by_key(|(index, _, _)| *index);
        report.failures = failed
            .into_iter()
            .map(|(_, value, error)| LoadFailure { value, error })
            .collect();
        report
    }

    /// Give each plain value of a selected feature the selector entries the
    /// lines before it set, for files that replay writes in order.
    fn bind_replayed_selectors(
        &self,
        values: &[SavedValue],
        plan: &PersistencePlan,
    ) -> Vec<SavedValue> {
        let mut current: HashMap<&str, &str> = HashMap::new();
        let mut bound = Vec::with_capacity(values.len());
        for value in values {
            let mut out = value.clone();
            if value.selectors.is_empty() {
                let selectors = match plan.position(&value.feature) {
                    Some(i) => plan.features()[i].selectors.clone(),
                    None => self.selectors_outermost_first(&value.feature),
                };
                out.selectors = selectors
                    .into_iter()
                    .filter_map(|s| {
                        let entry = current.get(s.as_str())?.to_string();
                        Some((s, entry))
                    })
                    .collect();
                if self
                    .node(&value.feature)
                    .is_some_and(|node| !node.selected().is_empty())
                {
                    current.insert(&value.feature, &value.value);
                }
            }
            bound.push(out);
        }
        bound
    }

    /// Write one value, under its selector entries, and put the selectors
    /// back.
    fn write_saved(&mut self, value: &SavedValue, io: &dyn RegisterIo) -> Result<(), GenApiError> {
        let mut moved: Vec<(&str, String)> = Vec::new();
        let result = self.write_under_selectors(value, io, &mut moved);
        for (selector, original) in moved.into_iter().rev() {
            if let Err(err) = self.set_text(selector, &original, io) {
                warn!(%selector, %original, error = %err, "could not put the selector back");
            }
        }
        result
    }

    fn write_under_selectors<'v>(
        &mut self,
        value: &'v SavedValue,
        io: &dyn RegisterIo,
        moved: &mut Vec<(&'v str, String)>,
    ) -> Result<(), GenApiError> {
        for (selector, entry) in &value.selectors {
            let original = self.get_text(selector, io)?;
            self.set_text(selector, entry, io)?;
            moved.push((selector, original));
        }
        match self.node(&value.feature) {
            Some(Node::Register(_)) => {
                let bytes = hex_to_bytes(&value.value).ok_or_else(|| GenApiError::InvalidText {
                    name: value.feature.clone(),
                    expected: "hexadecimal bytes",
                    text: value.value.clone(),
                })?;
                self.set_register(&value.feature, &bytes, io)
            }
            _ => self.set_text(&value.feature, &value.value, io),
        }
    }
}

/// The state of one feature's walk over its selectors' entries.
struct SelectorWalk<'a> {
    feature: &'a str,
    selectors: &'a [String],
    /// The selector entries set so far, outermost first.
    context: Vec<(String, String)>,
    values: Vec<SavedValue>,
    omitted: Vec<Omission>,
    reads: usize,
    /// Set when the feature cannot be saved at all.
    abandoned: Option<String>,
}

/// Errors no second attempt can change: the feature, its type or its access,
/// or the text itself.
fn is_permanent(err: &GenApiError) -> bool {
    matches!(
        err,
        GenApiError::NodeNotFound(_)
            | GenApiError::Type(_)
            | GenApiError::Access(_)
            | GenApiError::InvalidText { .. }
            | GenApiError::Unsupported(_)
    )
}

/// Warn when a configuration was saved from another camera model, or from
/// another major version of its XML — "files with a higher Major version
/// number are not backward compatible" (GenICam Standard v2.1.1 §2.7).
fn identity_warnings(saved: &DocumentIdentity, camera: &DocumentIdentity) -> Vec<String> {
    let differs = |a: &str, b: &str| !a.is_empty() && a != b;
    if differs(&saved.vendor_name, &camera.vendor_name)
        || differs(&saved.model_name, &camera.model_name)
    {
        return vec![format!(
            "saved from a {} {}, loaded onto a {} {}",
            saved.vendor_name, saved.model_name, camera.vendor_name, camera.model_name
        )];
    }
    let major = |version: &str| version.split('.').next().unwrap_or("").to_string();
    if !camera.version.is_empty() && differs(&major(&saved.version), &major(&camera.version)) {
        return vec![format!(
            "saved with version {} of the camera's XML, loaded with version {}: \
             a different major version is not compatible",
            saved.version, camera.version
        )];
    }
    Vec::new()
}

/// The order a restore writes the plan's features in, given `successors[p]`,
/// every feature whose value `p`'s write can change.
///
/// Strongly connected components (features in a cycle) are written as a
/// unit, their members in document order; the components follow the
/// dependencies between them, and document order where none applies.
fn restore_order(successors: &[BTreeSet<usize>]) -> Vec<usize> {
    let components = strongly_connected(successors);
    let mut component_of = vec![0usize; successors.len()];
    for (c, members) in components.iter().enumerate() {
        for &v in members {
            component_of[v] = c;
        }
    }
    let mut indegree = vec![0usize; components.len()];
    let mut component_successors: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); components.len()];
    for (p, targets) in successors.iter().enumerate() {
        for &f in targets {
            let (from, to) = (component_of[p], component_of[f]);
            if from != to && component_successors[from].insert(to) {
                indegree[to] += 1;
            }
        }
    }
    // Members are sorted, so a component's first member is its earliest.
    let mut ready: BinaryHeap<Reverse<(usize, usize)>> = components
        .iter()
        .enumerate()
        .filter(|(c, _)| indegree[*c] == 0)
        .map(|(c, members)| Reverse((members[0], c)))
        .collect();
    let mut order = Vec::with_capacity(successors.len());
    while let Some(Reverse((_, c))) = ready.pop() {
        order.extend(components[c].iter().copied());
        for &next in &component_successors[c] {
            indegree[next] -= 1;
            if indegree[next] == 0 {
                ready.push(Reverse((components[next][0], next)));
            }
        }
    }
    order
}

/// Tarjan's strongly connected components, iteratively; each component's
/// members sorted ascending.
fn strongly_connected(successors: &[BTreeSet<usize>]) -> Vec<Vec<usize>> {
    const UNSEEN: usize = usize::MAX;
    let n = successors.len();
    let adjacency: Vec<Vec<usize>> = successors
        .iter()
        .map(|s| s.iter().copied().collect())
        .collect();
    let mut index = vec![UNSEEN; n];
    let mut low = vec![0usize; n];
    let mut on_stack = vec![false; n];
    let mut stack = Vec::new();
    let mut next_index = 0;
    let mut components = Vec::new();
    for root in 0..n {
        if index[root] != UNSEEN {
            continue;
        }
        // (vertex, position in its adjacency list)
        let mut call: Vec<(usize, usize)> = vec![(root, 0)];
        index[root] = next_index;
        low[root] = next_index;
        next_index += 1;
        stack.push(root);
        on_stack[root] = true;
        while let Some(&mut (v, ref mut edge)) = call.last_mut() {
            if let Some(&w) = adjacency[v].get(*edge) {
                *edge += 1;
                if index[w] == UNSEEN {
                    index[w] = next_index;
                    low[w] = next_index;
                    next_index += 1;
                    stack.push(w);
                    on_stack[w] = true;
                    call.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
                continue;
            }
            call.pop();
            if let Some(&(parent, _)) = call.last() {
                low[parent] = low[parent].min(low[v]);
            }
            if low[v] == index[v] {
                let mut members = Vec::new();
                while let Some(w) = stack.pop() {
                    on_stack[w] = false;
                    members.push(w);
                    if w == v {
                        break;
                    }
                }
                members.sort_unstable();
                components.push(members);
            }
        }
    }
    components
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(n: usize, edges: &[(usize, usize)]) -> Vec<BTreeSet<usize>> {
        let mut successors = vec![BTreeSet::new(); n];
        for &(p, f) in edges {
            successors[p].insert(f);
        }
        successors
    }

    #[test]
    fn a_dependency_moves_a_feature_ahead_of_document_order() {
        assert_eq!(restore_order(&graph(3, &[(2, 0)])), [1, 2, 0]);
    }

    #[test]
    fn a_cycle_keeps_document_order_and_its_dependencies() {
        // 1 <-> 2 is a cycle that 3 must precede; 0 is free.
        assert_eq!(
            restore_order(&graph(4, &[(1, 2), (2, 1), (3, 1)])),
            [0, 3, 1, 2]
        );
    }

    #[test]
    fn components_are_found_in_a_chain_of_cycles() {
        let mut found = strongly_connected(&graph(5, &[(0, 1), (1, 0), (1, 2), (2, 3), (3, 2)]));
        found.sort();
        assert_eq!(found, [vec![0, 1], vec![2, 3], vec![4]]);
    }
}

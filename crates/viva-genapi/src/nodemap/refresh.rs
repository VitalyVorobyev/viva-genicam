//! `<Cachable>`, `<PollingTime>` and what changed when (GA-05, GA-40,
//! [ADR-0029]).
//!
//! GenICam Standard v2.1.1 §2.8.3 gives a register one of three caching
//! modes: "WriteThrough means that a value written to the camera is written to
//! the cache as well. WriteAround means that only read values are written to
//! the cache", and `NoCache` keeps nothing. Absent, a `<FloatReg>`'s mode is
//! `WriteAround` and every other register's `WriteThrough` (§3.2).
//! `<PollingTime>` is "a recommended time interval [in ms] after which a node
//! should be invalidated".
//!
//! The standard says nothing about the node in front of a register — the
//! `<Float>` whose `<pValue>` is a `NoCache` `<IntReg>`, the `<Converter>`
//! reading it. Here such a node is cached only as far as every register it is
//! read through allows, the combination GenApi's reference implementation
//! computes with `Combine`, and it keeps no cache of its own when one of them
//! polls: the register's own cache then decides when the value is read again.
//! That is the reading that never answers with a stale value.
//!
//! A `NodeMap` is not `Sync` — its caches are `RefCell`s — so there are no
//! callbacks. Every write and every invalidation advances a generation and
//! records it against the nodes it touched; [`NodeMap::changed_since`] answers
//! what changed after a generation a viewer saw, and [`NodeMap::poll`] turns
//! elapsed polling times into such changes, as `INodeMap::Poll` does.
//!
//! [ADR-0029]: https://github.com/VitalyVorobyev/viva-genicam/blob/main/docs/adrs/adr0029-caching-polling-and-refresh.md

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use viva_genapi_xml::CachingMode;

use super::NodeMap;
use crate::clock::{Clock, default_clock};
use crate::nodes::{Node, node_addressing};

/// What a `<PollingTime>` polls, by the kind of node declaring it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PollKind {
    /// A value: its reading is the device's for the polling time, then read
    /// again (§2.8.3).
    Value,
    /// A command: polled from its execution until `is_done` finds it
    /// finished; "If the command is write only no polling takes place"
    /// (§2.8.8).
    Command,
    /// An enumeration: polled — and read past every cache — while its value
    /// is an entry with `<IsSelfClearing>Yes` (§2.8.10).
    SelfClearing,
}

/// A node with a `<PollingTime>` of its own.
#[derive(Debug)]
pub(crate) struct Polled {
    kind: PollKind,
    period: Duration,
    /// When polling last started: the value read from the device (or written
    /// through to the cache), the command executed, the self-clearing entry
    /// seen. `None` while there is nothing to poll.
    since: Cell<Option<Duration>>,
}

/// The caching and polling each node is subject to, resolved once from the
/// XML, and the change history.
#[derive(Debug)]
pub(crate) struct Refresh {
    /// The mode of every node whose mode is not `WriteThrough`: its own, and
    /// that of every node its value is read through.
    modes: HashMap<String, CachingMode>,
    /// Nodes whose value is read through a polled node. They keep no cache
    /// of their own, so the polled node's expiry is what decides.
    via_polled: HashSet<String>,
    /// The shortest polling time on each node's value path, its own
    /// included.
    polling: HashMap<String, Duration>,
    /// Nodes with a polling time of their own.
    polled: HashMap<String, Polled>,
    clock: Arc<dyn Clock>,
    /// The generation each node last changed in.
    changed: RefCell<HashMap<String, u64>>,
    /// The generation every node last changed in at once.
    all_changed: Cell<u64>,
}

impl Default for Refresh {
    fn default() -> Self {
        Self {
            modes: HashMap::new(),
            via_polled: HashSet::new(),
            polling: HashMap::new(),
            polled: HashMap::new(),
            clock: default_clock(),
            changed: RefCell::new(HashMap::new()),
            all_changed: Cell::new(0),
        }
    }
}

/// A node's own caching mode: its `<Cachable>`, or a register's default.
fn own_mode(node: &Node) -> Option<CachingMode> {
    let declared = node.meta().cachable;
    if node_addressing(node).is_none() {
        return declared;
    }
    Some(declared.unwrap_or(match node {
        // Only a `<FloatReg>` is a float with a register of its own in a
        // standard document (a `<Float>` has none, §2.8.9).
        Node::Float(_) => CachingMode::WriteAround,
        _ => CachingMode::WriteThrough,
    }))
}

/// The nodes `node`'s value is read from: its `<pValue>`, a formula's
/// variables, a converter's `<pValue>` and the variables of its
/// `<FormulaFrom>`.
fn value_sources(node: &Node) -> Vec<&str> {
    match node {
        Node::Integer(n) => n.pvalue.iter().map(String::as_str).collect(),
        Node::Float(n) => n.pvalue.iter().map(String::as_str).collect(),
        Node::Enum(n) => n.pvalue.iter().map(String::as_str).collect(),
        Node::Boolean(n) => n.pvalue.iter().map(String::as_str).collect(),
        Node::Command(n) => n.pvalue.iter().map(String::as_str).collect(),
        Node::Converter(n) => std::iter::once(n.p_value.as_str())
            .chain(n.vars_from.iter().map(|var| var.provider.as_str()))
            .collect(),
        Node::IntConverter(n) => std::iter::once(n.p_value.as_str())
            .chain(n.vars_from.iter().map(|var| var.provider.as_str()))
            .collect(),
        Node::SwissKnife(n) => n.vars.iter().map(|var| var.provider.as_str()).collect(),
        _ => Vec::new(),
    }
}

/// What a node is subject to along its value path.
#[derive(Debug, Clone, Copy)]
struct Resolved {
    mode: CachingMode,
    polling: Option<Duration>,
    via_polled: bool,
}

const NEUTRAL: Resolved = Resolved {
    mode: CachingMode::WriteThrough,
    polling: None,
    via_polled: false,
};

impl NodeMap {
    /// Resolve every node's caching mode and polling time, once the map is
    /// built.
    pub(crate) fn build_refresh(&mut self) {
        let polled: HashMap<String, Polled> = self
            .nodes
            .iter()
            .filter_map(|(name, node)| {
                let ms = node.meta().polling_time_ms?;
                let kind = match node {
                    Node::Command(_) => PollKind::Command,
                    Node::Enum(_) => PollKind::SelfClearing,
                    _ => PollKind::Value,
                };
                Some((
                    name.clone(),
                    Polled {
                        kind,
                        period: Duration::from_millis(ms),
                        since: Cell::new(None),
                    },
                ))
            })
            .collect();
        let mut memo: HashMap<&str, Resolved> = HashMap::new();
        let mut stack: Vec<&str> = Vec::new();
        for name in &self.order {
            self.resolve(name, &polled, &mut memo, &mut stack);
        }
        let mut modes = HashMap::new();
        let mut polling = HashMap::new();
        let mut via_polled = HashSet::new();
        for (name, resolved) in memo {
            if resolved.mode != CachingMode::WriteThrough {
                modes.insert(name.to_string(), resolved.mode);
            }
            if let Some(period) = resolved.polling {
                polling.insert(name.to_string(), period);
            }
            if resolved.via_polled {
                via_polled.insert(name.to_string());
            }
        }
        self.refresh.modes = modes;
        self.refresh.polling = polling;
        self.refresh.via_polled = via_polled;
        self.refresh.polled = polled;
    }

    fn resolve<'a>(
        &'a self,
        name: &'a str,
        polled: &HashMap<String, Polled>,
        memo: &mut HashMap<&'a str, Resolved>,
        stack: &mut Vec<&'a str>,
    ) -> Resolved {
        if let Some(resolved) = memo.get(name) {
            return *resolved;
        }
        let Some(node) = self.nodes.get(name) else {
            return NEUTRAL;
        };
        // A `<pValue>` cycle is reported elsewhere; here it ends the path.
        if stack.contains(&name) {
            return NEUTRAL;
        }
        stack.push(name);
        let mut resolved = Resolved {
            mode: own_mode(node).unwrap_or(CachingMode::WriteThrough),
            polling: node.meta().polling_time_ms.map(Duration::from_millis),
            via_polled: false,
        };
        for source in value_sources(node) {
            let from = self.resolve(source, polled, memo, stack);
            resolved.mode = resolved.mode.combine(from.mode);
            resolved.polling = match (resolved.polling, from.polling) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            resolved.via_polled |= from.via_polled || polled.contains_key(source);
        }
        stack.pop();
        memo.insert(name, resolved);
        resolved
    }

    /// How `name`'s value may be cached: its own `<Cachable>` — or, for a
    /// register that declares none, the standard's default — combined with
    /// that of every node its value is read through. `None` for an unknown
    /// name.
    ///
    /// FLIR's `DeviceTemperature` is a `<Float>` over a `<Converter>` over an
    /// `<IntReg>` declared `NoCache`, so all three are `NoCache`.
    pub fn caching_mode(&self, name: &str) -> Option<CachingMode> {
        self.nodes.contains_key(name).then(|| {
            self.refresh
                .modes
                .get(name)
                .copied()
                .unwrap_or(CachingMode::WriteThrough)
        })
    }

    /// How long `name`'s value may be shown before it is read again: the
    /// shortest `<PollingTime>` among the node and those its value is read
    /// through. `None` when nothing on the path polls, and for an unknown
    /// name.
    ///
    /// A viewer re-reads the features it shows at this interval. A command's
    /// is how often to ask whether it has finished.
    pub fn polling_time(&self, name: &str) -> Option<Duration> {
        self.refresh.polling.get(name).copied()
    }

    /// Measure polling times against `clock` rather than the system clock —
    /// a [`ManualClock`](crate::ManualClock) in a test, or the host's clock
    /// where there is no `std::time::Instant` (`wasm32-unknown-unknown`).
    /// What was read before is treated as read at an unknown time, so it is
    /// read again.
    pub fn set_clock(&mut self, clock: Arc<dyn Clock>) {
        self.refresh.clock = clock;
        for polled in self.refresh.polled.values() {
            polled.since.set(None);
        }
    }

    /// The current generation: advanced by every write and every
    /// invalidation, never by a read.
    pub fn generation(&self) -> u64 {
        self.generation.get()
    }

    /// The nodes that changed after generation `since` — written, or
    /// invalidated because something they depend on was — in document order,
    /// and the generation now, to pass next time.
    ///
    /// A viewer keeps the generation of what it shows and re-reads the
    /// features this names. A value the camera changes on its own appears
    /// here once [`NodeMap::poll`] finds its polling time over, or once
    /// something invalidates it; nothing else can know.
    pub fn changed_since(&self, since: u64) -> (Vec<String>, u64) {
        let now = self.generation.get();
        if self.refresh.all_changed.get() > since {
            return (self.order.clone(), now);
        }
        let changed = self.refresh.changed.borrow();
        let names = self
            .order
            .iter()
            .filter(|name| {
                changed
                    .get(name.as_str())
                    .is_some_and(|generation| *generation > since)
            })
            .cloned()
            .collect();
        (names, now)
    }

    /// Invalidate every polled node whose polling time has passed, and
    /// return their names with those of every node read through them, in
    /// document order: what a viewer should read again. GenApi's
    /// `INodeMap::Poll`, measured against the node map's clock.
    ///
    /// A value is polled from when it was last read from the device and
    /// reported once until it is read again; one never read is not reported.
    /// A command is polled from its execution until [`NodeMap::is_done`]
    /// finds it finished, an enumeration while its value is a self-clearing
    /// entry; both are reported each time their polling time passes. The
    /// invalidations count as changes for [`NodeMap::changed_since`].
    pub fn poll(&self) -> Vec<String> {
        let refresh = &self.refresh;
        if refresh.polled.is_empty() {
            return Vec::new();
        }
        let now = refresh.clock.now();
        let mut due: Vec<&str> = Vec::new();
        for (name, polled) in &refresh.polled {
            let Some(since) = polled.since.get() else {
                continue;
            };
            if now.saturating_sub(since) < polled.period {
                continue;
            }
            polled.since.set(match polled.kind {
                PollKind::Value => None,
                PollKind::Command | PollKind::SelfClearing => Some(now),
            });
            due.push(name);
        }
        if due.is_empty() {
            return Vec::new();
        }
        self.bump_generation();
        let mut touched: HashSet<String> = HashSet::new();
        for name in due {
            self.invalidate_recursive(name, &mut touched);
        }
        self.order
            .iter()
            .filter(|name| touched.contains(name.as_str()))
            .cloned()
            .collect()
    }

    /// Whether a read of `name` may be answered from its own cache now.
    pub(crate) fn may_serve(&self, name: &str) -> bool {
        let refresh = &self.refresh;
        if refresh.modes.get(name) == Some(&CachingMode::NoCache)
            || refresh.via_polled.contains(name)
        {
            return false;
        }
        let Some(polled) = refresh.polled.get(name) else {
            return true;
        };
        match polled.kind {
            PollKind::Value => polled
                .since
                .get()
                .is_some_and(|since| refresh.clock.now().saturating_sub(since) < polled.period),
            PollKind::SelfClearing => polled.since.get().is_none(),
            PollKind::Command => true,
        }
    }

    /// Whether a value of `name` just read from the device may be kept.
    pub(crate) fn may_cache(&self, name: &str) -> bool {
        self.refresh.modes.get(name) != Some(&CachingMode::NoCache)
            && !self.refresh.via_polled.contains(name)
    }

    /// Whether a write to `name` may leave the written value in its cache:
    /// `WriteThrough` only.
    pub(crate) fn keeps_written(&self, name: &str) -> bool {
        !self.refresh.modes.contains_key(name) && !self.refresh.via_polled.contains(name)
    }

    /// `name`'s value was just read from the device, or written through to
    /// its cache: a polled value's polling time starts now.
    pub(crate) fn note_read(&self, name: &str) {
        let refresh = &self.refresh;
        if refresh.polled.is_empty() {
            return;
        }
        if let Some(polled) = refresh.polled.get(name)
            && polled.kind == PollKind::Value
        {
            polled.since.set(Some(refresh.clock.now()));
        }
    }

    /// `name`, an enumeration, now holds an entry that is, or is not,
    /// self-clearing: polling starts or stops.
    pub(crate) fn note_entry(&self, name: &str, self_clearing: bool) {
        let refresh = &self.refresh;
        if let Some(polled) = refresh.polled.get(name)
            && polled.kind == PollKind::SelfClearing
        {
            polled.since.set(self_clearing.then(|| refresh.clock.now()));
        }
    }

    /// Whether `name` is an enumeration polled while self-clearing that is
    /// active now: read past every cache.
    pub(crate) fn self_clearing_active(&self, name: &str) -> bool {
        self.refresh.polled.get(name).is_some_and(|polled| {
            polled.kind == PollKind::SelfClearing && polled.since.get().is_some()
        })
    }

    /// `name`, a command, was executed (`true`) or found finished: polling
    /// starts or stops.
    pub(crate) fn note_command(&self, name: &str, running: bool) {
        let refresh = &self.refresh;
        if let Some(polled) = refresh.polled.get(name)
            && polled.kind == PollKind::Command
        {
            polled.since.set(running.then(|| refresh.clock.now()));
        }
    }

    /// Record that `name` changed in the current generation.
    pub(crate) fn note_changed(&self, name: &str) {
        let generation = self.generation.get();
        let mut changed = self.refresh.changed.borrow_mut();
        match changed.get_mut(name) {
            Some(at) => *at = generation,
            None => {
                changed.insert(name.to_string(), generation);
            }
        }
    }

    /// Record that every node changed in the current generation.
    pub(crate) fn note_all_changed(&self) {
        self.refresh.all_changed.set(self.generation.get());
    }
}

# Handoff: expose `IsImplemented` / `IsAvailable` predicates on `NodeMap`

**To:** genicam-rs maintainers
**From:** GenICam Studio — Feature Browser review
**Date:** 2026-04-12
**Severity:** user-visible (feature dropdowns offer options the device does not support)
**Repro:** aravis `arv-fake-gv-camera-0.8`, select `PixelFormat` in the Feature Browser

## Summary

Studio's Feature Browser shows **all XML-declared enum entries**, not just those the device currently reports as implemented or available. E.g. `PixelFormat` shows four entries while the fake camera supports two. Writing the unsupported entries either errors silently or silently writes whatever the device rejects to.

Studio has a new `FeatureState.enum_available: Option<Vec<String>>` contract (`viva-zenoh-api` API v2) precisely for this case, and the service is wired to fill it — but the service currently just passes through `Camera::enum_entries`, which returns the full static list from the XML. The filtering work has to happen in `viva-genapi`'s `NodeMap`.

## Ask

Add four predicates to `NodeMap`, all evaluating at the current selector/pValue context:

```rust
impl NodeMap {
    /// Does this node exist as a feature on the device at all?
    /// (`pIsImplemented` expression, or a <NodeSwissKnife> referenced by it.)
    pub fn is_implemented(&self, name: &str, io: &dyn RegisterIo) -> Result<bool, GenApiError>;

    /// Is the node currently accessible? (`pIsAvailable` expression,
    /// selector gating rules, and effective access mode all collapse into one
    /// bool for the UI's purposes.)
    pub fn is_available(&self, name: &str, io: &dyn RegisterIo) -> Result<bool, GenApiError>;

    /// The access mode that actually applies right now. Static XML `AccessMode`
    /// is the default; `pIsLocked` / `pIsAvailable` expressions can escalate a
    /// node to `NA` or downgrade `RW` to `RO`.
    pub fn effective_access_mode(&self, name: &str, io: &dyn RegisterIo)
        -> Result<AccessMode, GenApiError>;

    /// For Enumeration nodes: the subset of `entries` that pass
    /// `is_implemented && is_available` given the current device state. When
    /// no predicate is declared, return the full static list so callers do
    /// not regress behaviour.
    pub fn available_enum_entries(&self, name: &str, io: &dyn RegisterIo)
        -> Result<Vec<String>, GenApiError>;
}
```

`viva-genapi-xml`'s `NodeDecl` already carries `pIsImplemented` / `pIsAvailable` refs (check the Enum and Integer variants); the predicates resolve them the same way the swissknife evaluator does for `pMin` / `pMax`.

## How Studio plans to consume this

`viva-service::DeviceHandle::get_feature_state` (in `crates/viva-service/src/device.rs`) currently does:

```rust
let enum_available = if matches!(node, Node::Enum(_)) {
    camera.enum_entries(name).ok()
} else {
    None
};
```

Once the predicates land, it becomes:

```rust
let enum_available = if matches!(node, Node::Enum(_)) {
    camera.nodemap().available_enum_entries(name, camera.transport()).ok()
} else {
    None
};
// Override the declared access mode with the runtime-effective one.
access_mode = access_mode_string_from(
    camera.nodemap().effective_access_mode(name, camera.transport()).ok(),
);
is_implemented = camera.nodemap().is_implemented(name, camera.transport()).unwrap_or(true);
is_available = camera.nodemap().is_available(name, camera.transport()).unwrap_or(true);
```

— a one-line change per predicate. All four fields on `FeatureState` already exist on the wire (`viva-zenoh-api`), the UI already reads them, and the UI fallback when these are absent is to **show everything**. So landing this work is pure upside.

## Priority hint

Without `available_enum_entries` the PixelFormat / AcquisitionMode / TriggerSelector etc. dropdowns will keep offering entries the device rejects, which is the #1 user-reported annoyance. Without `is_available` selector-gated nodes remain editable even when they will never succeed. Without `effective_access_mode` Apply buttons stay lit on RO nodes. If you can only land one of the four, `available_enum_entries` delivers the most perceived fix.

## References

- GenICam GenApi Standard — `IsImplemented`, `IsAvailable`, `pIsImplemented`, `pIsAvailable` semantics.
- ADR-010 (GenICam Studio repo) — the `FeatureState` contract this plugs into.
- Companion handoff: `2026-04-12-genapi-numeric-type-dispatch.md`.

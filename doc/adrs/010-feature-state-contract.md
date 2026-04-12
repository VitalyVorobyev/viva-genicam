# ADR-010: FeatureState as the authoritative live-state contract

**Status:** Accepted
**Date:** 2026-04-12

## Context

The Feature Browser UI was showing static XML metadata as if it were live device state. Five distinct user-visible bugs (enum resets to "(unset)" after Apply, Command buttons do nothing, integer/float values decoded as garbage, extra enum entries, `Width` range showing `i64::MIN..=i64::MAX`) all traced back to one architectural gap: every layer between the device and the UI either hard-coded introspection fields (`access_mode: "RW"`, `min/max/inc: None`) or discarded them on the wire.

The Zenoh `NodeValueUpdate` contract (API v1) declared `min`/`max`/`inc` as *optional hints* (ZA-06) but nothing ever populated them. The service's `publish_node_value` literally did `access_mode: "RW".to_string(), min: None, max: None, inc: None`. The Studio embedded backend did the same in `get_feature`. The UI, given no live introspection, fell back to `node.constraints` from the static XML — which itself defaulted to `i64::MIN/MAX` when the XML had no explicit bounds.

## Decision

Introduce `FeatureState` as the **authoritative live-state tuple** for any GenICam feature, alongside (not replacing) the legacy `NodeValueUpdate`.

```rust
pub struct FeatureState {
    pub value:          serde_json::Value,
    pub access_mode:    String,             // "RO" | "RW" | "WO" | "NA"
    pub kind:           String,             // "Integer" | "Float" | "Enumeration" | ...
    pub is_implemented: bool,
    pub is_available:   bool,
    pub numeric:        Option<NumericRange>,   // Integer/Float
    pub enum_available: Option<Vec<String>>,    // Enumeration
    pub unit:           Option<String>,
}
```

Every layer in the stack is updated to produce / consume this type:

1. **Wire contract** (`viva-zenoh-api`): new types, new keyspace `nodes/{name}/state` + `nodes/bulk/state`. `NodeValueUpdate` stays as a projection of `FeatureState::to_node_value_update`.
2. **Service** (`viva-service`): `DeviceOps::get_feature_state` trait method. GigE (`DeviceHandle`) overrides with typed NodeMap reads; other transports use a default projection from `get_feature`.
3. **Studio Tauri backend**: `DeviceBackend::get_feature_state` trait method; `EmbeddedBackend` implements it directly, `RemoteBackend` projects from the Zenoh `node_cache`. New Tauri commands `query_feature_state` / `query_feature_states_bulk`. `write_node` now returns `FeatureState`; `execute_command` returns `CommandResult` with `affected_states`.
4. **UI** (Step 5 of the migration): the Feature Browser treats `FeatureState` as the single source of truth for enum options, slider ranges, access-mode gating, and post-apply draft reconciliation.

### What `FeatureState` is NOT

- It is **not** a replacement for `UiGraph` / `UiNode`. Static XML metadata (display name, tooltip, categorisation, SwissKnife expressions) stays in the existing descriptor — `FeatureState` carries only the runtime-variable parts.
- It is **not** populated by the XML parser. A `FeatureState` only exists once a device is connected and a read happens against it.
- It does **not** carry selectors' transitive state. When a selector changes, the service republishes affected nodes' `FeatureState`; there is no selector-tuple field on the state itself.

## Consequences

### Positive
- Every user-visible symptom of the "static-as-live" bug is addressable by fixing one path rather than five.
- UI can disable Apply/Execute based on real access mode, show real enum options, show real ranges — or honestly say "range unknown".
- Subsequent work to add `IsAvailable`/`IsImplemented` predicates in `viva-genapi` plugs into the existing `is_implemented` / `is_available` fields without another contract change.
- Bumps API version to `2`. Services and clients that only speak v1 keep working via the `NodeValueUpdate` projection.

### Negative
- Two parallel keys on the wire (`nodes/{name}/value` + `nodes/{name}/state`) during the v1→v2 transition. Once the UI migration completes, the legacy key could be retired, but there is no forced timeline.
- Every transport that implements `DeviceOps` gets a default `get_feature_state` that returns partial information; only GigE currently has the typed-read override. U3V is a follow-up.
- The `viva-genapi` nodemap has known bugs where float-backed registers are decoded through the integer path (see `docs/handoffs/2026-04-12-genapi-numeric-type-dispatch.md`). Fixing `FeatureState` dispatch surfaces those bugs rather than hiding them behind string sniffing — intentional trade-off.

## References
- Implementation plan: `~/.claude/plans/distributed-churning-mist.md`
- ADR-008 (the original Zenoh API contract this extends)
- Handoffs for the `viva-genapi` follow-ups: `docs/handoffs/2026-04-12-*.md`

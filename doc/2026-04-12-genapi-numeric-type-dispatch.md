# Handoff: `viva-genapi` numeric type dispatch

**To:** genicam-rs maintainers
**From:** GenICam Studio — Feature Browser review
**Date:** 2026-04-12
**Severity:** user-visible on every connected GigE camera
**Repro:** aravis `arv-fake-gv-camera-0.8` + Studio embedded mode (`cargo tauri dev`)

## Summary

Numeric features read through `Camera::get` return values whose string representation is **the bit pattern of the float, decoded as an integer (or vice versa)**. The Studio UI surfaces this as "garbage numbers" for everyday fields:

| Feature | Expected value | Observed value (as `Camera::get` returns) |
|---|---|---|
| `AcquisitionFrameRate` | `30.0` fps | `1106247680` |
| `ExposureTime` | `~6000.0` µs | `4662219572839973000` |

## The numbers are not random

- `1106247680 = 0x41F00000` — this is the **IEEE-754 single-precision** representation of `30.0`. So `AcquisitionFrameRate`'s 4-byte register, which actually contains `30.0` as an `f32`, is being read back as an `i64`.
- `4662219572839973000 = 0x40B77EB85EB85EB8` — an **IEEE-754 double-precision** value very close to `6000.0`. So `ExposureTime`'s 8-byte register contains `6000.0` as an `f64` but is being read back as an `i64`.

In both cases the bits reach the application intact; what is wrong is the **type** we choose at read time.

## Where this lives

The Studio embedded backend now dispatches through `Camera::nodemap().get_integer() / get_float() / ...` based on `Node::kind_name()` — see [`viva-studio-tauri/src-tauri/src/backend/embedded.rs::build_feature_state`](../../apps/viva-studio-tauri/src-tauri/src/backend/embedded.rs). That dispatch is *correct*; the garbage still appears, so the bug is upstream in how `viva-genapi` reads float-backed registers, or in the XML/`NodeDecl` classifying these features as `Integer` when they are actually `Float`.

Candidates, in likely order:

1. **`Camera::get`** (`genicam-rs/crates/viva-genicam/src/lib.rs:195`): when the matched node is `Some(Node::Integer(_))` but the underlying register is actually a float, `nodemap.get_integer` reads the raw bytes as signed integer with no awareness of the float encoding. If the XML really says `Integer`, it's an XML / parser bug; if the XML says `Float` but the NodeMap resolves it as Integer, it's a dispatch bug.
2. **`IntConverter` / `Converter` paths**: these wrap a raw node and apply formulas. If `AcquisitionFrameRate` is declared as an IntConverter over a float-register providerand the formula evaluator takes the raw i64 bit pattern rather than the converted float, the output would be exactly what we see.
3. **Endianness**: less likely (the decoded pattern matches float-in-little-endian → i64-in-little-endian), but worth a targeted check in `viva-genapi/src/io.rs` or the `Addressing` code path.

## Ask

1. In `viva-genapi`, verify the read-time type dispatch matches the declared node kind: `Node::Float(_)` → `get_float`, `Node::Integer(_)` → `get_integer`, never the reverse. Add a debug assertion in `get_integer` that rejects registers whose `Addressing.length` is 4 or 8 AND whose declared register is known to back a `Float` elsewhere in the model.
2. Confirm the aravis fake camera's XML declares `AcquisitionFrameRate` and `ExposureTime` as `<Float>`, not `<Integer>`. If they are declared `<Integer>` with a format-style conversion, the XML-level type dispatch in `viva-genapi-xml` needs to resolve that to a Float node.
3. Add an integration test against `arv-fake-gv-camera-0.8`: set `ExposureTime` to `5000.0`, read it back, assert the returned value is within `|5000 - v| < 1.0` (allowing for device rounding). Today that assertion would fail with an astronomical number.

## Why this matters to Studio

The Studio UI now prefers live `FeatureState.numeric.min/max` over static XML (see [ADR-010](../adrs/010-feature-state-contract.md)). But the live *value* rendered in the Feature Browser's live badge and used to seed the editor still comes from `Camera::get`. When that value is nonsense, the UI honestly displays nonsense — it can reject NaN/Inf but cannot detect "this integer is secretly a float bit pattern". So this bug is **blocking** for the "ExposureTime shows sane numbers" line item in our migration verification.

## Not in scope for this handoff

- Expanding `IsImplemented` / `IsAvailable` predicate support (see the companion handoff `2026-04-12-genapi-introspection-predicates.md`).
- Changing the wire protocol. The Zenoh `FeatureState` contract is already in place (`viva-zenoh-api` API v2); the service just needs to deliver correct values into it.

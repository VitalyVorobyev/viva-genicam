// The backend's IPC types, generated from the Rust definitions
// (`src/generated/ipc.ts`, backlog ST-28). Import them from here.
//
// Notes the generated declarations do not carry:
//
// - `FeatureState` is the authoritative live state of a feature (ADR-0010).
//   `value` is `null` when the feature has nothing to read — a Category, a
//   Command, a write-only node, the register a command writes through, or a
//   feature not implemented or not available now. Show no value for those, not
//   the text "null". A `Register` reports `{ kind: "register", length }`.
// - `numeric` is present only for Integer/Float nodes with a resolvable range.
//   When absent, render "range unknown" — never invent `i64::MIN` / `i64::MAX`.
// - `enum_available` replaces the static XML entries as the dropdown options.
// - Read live state through `FeatureStoreContext` (`useFeature`), which keeps
//   the one cache the UI has (ST-29).

export type {
  AcquisitionStatus,
  CommandResult,
  ConnectionState,
  DeviceInfo,
  DisconnectReason,
  FeatureState,
  FeatureStateChanged,
  FeaturesInvalidated,
  ImageMeta,
  JsonValue,
  NetworkConfig,
  NumericRange,
  SfncFeature,
  SfncGroup,
  StreamerInfo,
  StudioEvents,
} from "../generated/ipc";

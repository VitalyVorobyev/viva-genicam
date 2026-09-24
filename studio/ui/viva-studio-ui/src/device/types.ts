// Types mirroring the Rust device_state.rs enums and structs.
// Kept in sync with the Tauri backend serialization.

export interface DeviceInfo {
  /** Stable device id; for GigE `cam-<mac hex>` in both embedded and remote mode. */
  id: string;
  /** User-defined name, else the model, else the address. */
  name: string;
  model: string;
  /** Serial as the device reports it; empty when it reports none. */
  serial: string;
  /** Transport type: "gige", "usb3", or "zenoh" (remote service). */
  transport?: string;
  /** Current IPv4 address; absent for USB3 and from services before API v3. */
  ip?: string;
  /** MAC address as `AA:BB:CC:DD:EE:FF`. */
  mac?: string;
  /** User-defined device name, when one is set. */
  user_name?: string;
  manufacturer?: string;
}

export type ConnectionState =
  | { kind: "disconnected" }
  | { kind: "connecting"; device_id: string }
  | { kind: "connected"; device_id: string; device_name: string; model: string }
  | { kind: "reconnecting"; device_id: string; attempt: number; max_attempts: number; reason: string }
  | { kind: "error"; message: string };

export interface AcquisitionStatus {
  active: boolean;
  fps: number | null;
  dropped: number;
}

export interface StreamerInfo {
  ws_url: string;
  width: number;
  height: number;
}

/**
 * Legacy node value payload. New code should use {@link FeatureState} instead —
 * it carries the same value + access mode plus live introspection (kind,
 * numeric range, enum availability, implemented/available flags).
 *
 * Kept for backward compatibility with services/providers that still emit the
 * older shape; the Tauri backend projects `FeatureState` to this type until
 * the UI migration is complete.
 */
export interface NodeValueEntry {
  /** `null` when the feature has nothing to read; see {@link FeatureState}. */
  value: number | string | boolean | null;
  access_mode: string;
  /** Optional runtime minimum constraint reported by the camera service. */
  min?: number;
  /** Optional runtime maximum constraint reported by the camera service. */
  max?: number;
  /** Optional runtime increment (step) reported by the camera service. */
  inc?: number;
}

/** Numeric range for an Integer or Float feature at the current camera state. */
export interface NumericRange {
  min: number;
  max: number;
  /** Optional increment (step); absent when the feature has no grid. */
  inc?: number;
}

/**
 * Live state of a GenICam feature at a single point in time.
 *
 * Authoritative source of truth for the Feature Browser UI: the current value,
 * access mode, kind, numeric range, and available enum entries all reflect
 * what the device reports now (not the static XML descriptor).
 *
 * - `kind` matches `viva_genapi::Node::kind_name` ("Integer", "Float",
 *   "Enumeration", "Boolean", "Command", "Category", "SwissKnife", "Converter",
 *   "IntConverter", "StringReg"). Tolerate unknown kinds.
 * - `access_mode` uses GenICam spelling: `"RO"`, `"RW"`, `"WO"`, `"NA"`.
 * - `value` is `null` when the feature has nothing to read: a Category, a
 *   Command, a node declared `WO`, or the register a command writes through.
 *   The backend never reads those, but still reports them so the UI can offer
 *   the write or the execute. Show no value for them, not the text "null".
 * - `is_implemented` / `is_available` default to `true` when the service does
 *   not yet evaluate them.
 * - `numeric` is present only for Integer/Float nodes with a resolvable range.
 *   When absent, the UI must render "range unknown" — never invent `i64::MIN` /
 *   `i64::MAX` fallbacks.
 * - `enum_available` is present only for Enumeration nodes and replaces the
 *   static XML enum list as the source of dropdown options.
 */
export interface FeatureState {
  value: number | string | boolean | null;
  access_mode: string;
  kind: string;
  is_implemented: boolean;
  is_available: boolean;
  numeric?: NumericRange;
  enum_available?: string[];
  unit?: string;
}

/**
 * Response from executing a Command node. `ok=false` means the backend rejected
 * the request; `error` carries the message. `affected_states` is a map of
 * nodes whose post-execution value the backend already re-read on our behalf
 * (e.g. `AcquisitionStatus` after `AcquisitionStart`), so the UI can update
 * badges and form inputs without a second round-trip.
 */
export interface CommandResult {
  ok: boolean;
  error?: string;
  affected_states: Record<string, FeatureState>;
}

export interface ImageMeta {
  pixel_format: string;
  width: number;
  height: number;
  payload_size: number;
}

export interface SfncFeature {
  node: string;
  widget: "float_slider" | "int_slider" | "int_select" | "enum_select" | "bool_toggle" | "command_button" | string;
}

export interface SfncGroup {
  id: string;
  title: string;
  icon: string;
  default_open: boolean;
  features: SfncFeature[];
}

export interface DisconnectReason {
  message: string;
  device_id: string;
}

export interface StreamerStatus {
  running: boolean;
  error: string | null;
  restart_count: number;
}

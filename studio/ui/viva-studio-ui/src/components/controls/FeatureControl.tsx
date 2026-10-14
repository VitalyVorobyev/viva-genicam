import { useState, type KeyboardEvent, type ReactNode } from "react";
import type { UiNode } from "../../xml_model/uigraph";
import type { FeatureState, NumericRange } from "../../device/types";
import {
  useCommandRunner,
  useFeature,
  useFeatureWriter,
} from "../../device/FeatureStoreContext";
import { nodeDisplayName } from "../../xml_model/helpers";
import { InspectorRow } from "./InspectorRow";

/**
 * How a feature is drawn; `sfnc-groups.json` names one per feature. A Rust
 * test (`commands/sfnc_groups.rs`) keeps the bundled file to these names.
 */
const WIDGETS = [
  "float_input",
  "int_input",
  "int_select",
  "enum_select",
  "bool_toggle",
  "command_button",
  "text",
] as const;
export type FeatureWidget = (typeof WIDGETS)[number];

function isFeatureWidget(name: string | undefined): name is FeatureWidget {
  return (WIDGETS as readonly string[]).includes(name ?? "");
}

interface FeatureControlProps {
  /** The feature's declaration; nothing is drawn for a feature the camera lacks. */
  node: UiNode | undefined;
  /** Overrides the widget the node's kind implies. */
  widget?: string;
  /** Disables the control for a reason of the caller's, such as streaming. */
  disabledReason?: string;
  /** Drawn before the control in its column, such as the switch that enables it. */
  leading?: ReactNode;
}

/**
 * One feature as a row of the inspector, driven by its live state (backlog
 * ST-29).
 *
 * The range, the enumeration entries and whether the control is enabled all
 * come from the camera's `FeatureState`, not from guesses. A feature that is
 * unavailable, locked or read-only is shown greyed out with the reason
 * (ST-35); one the device does not implement is not shown.
 *
 * What the user types is kept only once they actually change it, and only
 * until they commit it (Enter, blur, or an arrow key's step) or press
 * Escape. A field that merely has focus keeps following the camera, and
 * leaving it without an edit writes nothing (Codex review of #167).
 */
export function FeatureControl({ node, widget, disabledReason, leading }: FeatureControlProps) {
  const state = useFeature(node?.name);
  if (!node) return null;
  if (state && !state.is_implemented) return null;

  const kind = isFeatureWidget(widget) ? widget : widgetForKind(node);
  if (!kind) return null;
  const label = nodeDisplayName(node);
  const blocked = disabledReason ?? blockedReason(state, kind);
  const props = { node, state, label, blocked, leading };

  switch (kind) {
    case "float_input":
    case "int_input":
      return <NumericControl {...props} integer={kind === "int_input"} />;
    case "int_select":
      return <IntegerSelect {...props} />;
    case "enum_select":
      return <EnumSelect {...props} />;
    case "bool_toggle":
      return <BooleanToggle {...props} />;
    case "command_button":
      return <CommandButton {...props} />;
    case "text":
      return <TextControl {...props} />;
  }
}

interface ControlProps {
  node: UiNode;
  state: FeatureState | undefined;
  label: string;
  /** Why the control cannot be used now, or `null` if it can. */
  blocked: string | null;
  leading?: ReactNode;
}

function widgetForKind(node: UiNode): FeatureWidget | null {
  switch (node.kind) {
    case "Float":
      return "float_input";
    case "Integer":
      return "int_input";
    case "Enumeration":
      return "enum_select";
    case "Boolean":
      return "bool_toggle";
    case "Command":
      return "command_button";
    case "String":
      return "text";
    default:
      return null;
  }
}

/** Why a feature cannot be changed now, from its live state. */
export function blockedReason(
  state: FeatureState | undefined,
  kind: FeatureWidget,
): string | null {
  if (!state) return "Reading…";
  if (!state.is_available) return "Not available in the camera's current state";
  const writable = state.access_mode === "RW" || state.access_mode === "WO";
  if (!writable) {
    return kind === "command_button" ? "Cannot be executed now" : "Read-only now";
  }
  return null;
}

/** The live range, or the declared one when the camera does not report it. */
function rangeOf(node: UiNode, state: FeatureState | undefined): NumericRange | null {
  if (state?.numeric) return state.numeric;
  const c = node.constraints;
  if (c?.min !== undefined && c.max !== undefined && Number.isFinite(c.min) && Number.isFinite(c.max)) {
    return { min: c.min, max: c.max, inc: c.inc };
  }
  return null;
}

/** The unit as shown; GenICam spells a microsecond `us`. */
export function displayUnit(unit: string): string {
  return unit === "us" ? "µs" : unit;
}

function unitOf(node: UiNode, state: FeatureState | undefined): string {
  return displayUnit(state?.unit ?? node.unit ?? "");
}

/** A bound as shown in the range tooltip: integers whole, floats to six digits. */
function formatBound(n: number): string {
  return Number.isInteger(n) ? String(n) : String(Number(n.toPrecision(6)));
}

/**
 * A value the camera accepts: within the range and, for an integer, on its
 * grid `min + k·inc`. An integer that was only rounded could be written off
 * the grid (Width, OffsetX), which the camera refuses.
 */
export function snapToRange(n: number, range: NumericRange | null, integer: boolean): number {
  let v = integer ? Math.round(n) : n;
  if (!range) return v;
  v = Math.min(range.max, Math.max(range.min, v));
  if (integer) {
    const inc = Math.max(1, range.inc ?? 1);
    v = range.min + Math.round((v - range.min) / inc) * inc;
    if (v > range.max) v -= inc;
  }
  return v;
}

/**
 * The value an arrow key asks for: one increment up or down, snapped. An
 * integer steps by 1 unless the camera declares more; a float steps only by
 * an increment the camera declares, and `null` says it has none.
 */
export function stepValue(
  current: number,
  direction: 1 | -1,
  range: NumericRange | null,
  integer: boolean,
): number | null {
  const inc = integer ? Math.max(1, range?.inc ?? 1) : range?.inc;
  if (!inc) return null;
  // Twelve significant digits drop the float noise of the addition.
  return snapToRange(Number((current + direction * inc).toPrecision(12)), range, integer);
}

function NumericControl({
  node,
  state,
  label,
  blocked,
  leading,
  integer,
}: ControlProps & { integer: boolean }) {
  const write = useFeatureWriter();
  const [draft, setDraft] = useState<string | null>(null);
  const live = typeof state?.value === "number" ? state.value : null;
  const range = rangeOf(node, state);
  const unit = unitOf(node, state);
  const shown = draft ?? (live === null ? "" : String(live));
  const rangeText = range
    ? `${formatBound(range.min)} – ${formatBound(range.max)}${unit ? ` ${unit}` : ""}`
    : undefined;

  const commit = (text: string | null) => {
    setDraft(null);
    if (text === null) return;
    const n = Number(text);
    if (text.trim() === "" || !Number.isFinite(n)) return;
    const v = snapToRange(n, range, integer);
    if (v !== live) void write(node.name, v);
  };
  const step = (direction: 1 | -1) => {
    const typed = draft !== null && draft.trim() !== "" ? Number(draft) : NaN;
    const from = Number.isFinite(typed) ? typed : live;
    if (from === null) return;
    const next = stepValue(from, direction, range, integer);
    if (next === null) return;
    setDraft(null);
    if (next !== live) void write(node.name, next);
  };
  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") commit(draft);
    if (e.key === "Escape") setDraft(null);
    if (e.key === "ArrowUp" || e.key === "ArrowDown") {
      // The browser's own step would change the text without a write.
      e.preventDefault();
      step(e.key === "ArrowUp" ? 1 : -1);
    }
  };

  return (
    <InspectorRow label={label} blocked={blocked} feature={node.name}>
      {({ id, describedBy }) => (
        <>
          {leading}
          <span className="iv-number">
            <input
              id={id}
              type="number"
              className="iv-number__input"
              aria-label={label}
              aria-describedby={describedBy}
              title={blocked ?? rangeText}
              value={shown}
              min={range?.min}
              max={range?.max}
              step={integer ? Math.max(1, range?.inc ?? 1) : (range?.inc ?? "any")}
              disabled={blocked !== null}
              onChange={(e) => setDraft(e.target.value)}
              onKeyDown={onKeyDown}
              onBlur={() => commit(draft)}
            />
            {unit && (
              <span className="iv-number__unit" aria-hidden="true">
                {unit}
              </span>
            )}
          </span>
        </>
      )}
    </InspectorRow>
  );
}

/**
 * A boolean feature as a bare checkbox, for drawing before the control it
 * governs, such as `AcquisitionFrameRateEnable` before the frame rate.
 */
export function FeatureCheckbox({ node }: { node: UiNode | undefined }) {
  const state = useFeature(node?.name);
  const write = useFeatureWriter();
  if (!node) return null;
  if (state && !state.is_implemented) return null;
  const label = nodeDisplayName(node);
  const blocked = blockedReason(state, "bool_toggle");
  return (
    <input
      type="checkbox"
      className="iv-row__leading-check"
      data-feature={node.name}
      aria-label={label}
      title={blocked ? `${label} — ${blocked}` : label}
      checked={typeof state?.value === "boolean" ? state.value : false}
      disabled={blocked !== null}
      onChange={(e) => void write(node.name, e.target.checked)}
    />
  );
}

/** An integer chosen from its grid, such as a binning factor. */
function IntegerSelect({ node, state, label, blocked, leading }: ControlProps) {
  const write = useFeatureWriter();
  const range = rangeOf(node, state);
  const live = typeof state?.value === "number" ? state.value : null;
  const options: number[] = [];
  if (range) {
    const step = Math.max(1, range.inc ?? 1);
    for (let v = range.min; v <= range.max && options.length < 64; v += step) options.push(v);
  }
  if (live !== null && !options.includes(live)) options.unshift(live);

  return (
    <InspectorRow label={label} blocked={blocked} feature={node.name}>
      {({ id, describedBy }) => (
        <>
          {leading}
          <select
            id={id}
            className="iv-select"
            aria-label={label}
            aria-describedby={describedBy}
            value={live === null ? "" : String(live)}
            disabled={blocked !== null}
            onChange={(e) => void write(node.name, Number(e.target.value))}
          >
            {live === null && <option value="">—</option>}
            {options.map((v) => (
              <option key={v} value={String(v)}>
                {v}
              </option>
            ))}
          </select>
        </>
      )}
    </InspectorRow>
  );
}

function EnumSelect({ node, state, label, blocked, leading }: ControlProps) {
  const write = useFeatureWriter();
  const live = typeof state?.value === "string" ? state.value : null;
  const declared = node.enum_entries ?? [];
  const names = state?.enum_available ?? declared.map((e) => e.name);
  // The camera's current entry stays listed even if it is not selectable now.
  if (live !== null && !names.includes(live)) names.unshift(live);
  const displayName = (name: string) =>
    declared.find((e) => e.name === name)?.display_name ?? name;

  return (
    <InspectorRow label={label} blocked={blocked} feature={node.name}>
      {({ id, describedBy }) => (
        <>
          {leading}
          <select
            id={id}
            className="iv-select"
            aria-label={label}
            aria-describedby={describedBy}
            value={live ?? ""}
            disabled={blocked !== null}
            onChange={(e) => void write(node.name, e.target.value)}
          >
            {live === null && <option value="">—</option>}
            {names.map((name) => (
              <option key={name} value={name}>
                {displayName(name)}
              </option>
            ))}
          </select>
        </>
      )}
    </InspectorRow>
  );
}

function BooleanToggle({ node, state, label, blocked, leading }: ControlProps) {
  const write = useFeatureWriter();
  const live = typeof state?.value === "boolean" ? state.value : false;
  return (
    <InspectorRow label={label} blocked={blocked} feature={node.name}>
      {({ id, describedBy }) => (
        <>
          {leading}
          <span className="iv-check">
            <input
              id={id}
              type="checkbox"
              aria-label={label}
              aria-describedby={describedBy}
              checked={live}
              disabled={blocked !== null}
              onChange={(e) => void write(node.name, e.target.checked)}
            />
          </span>
        </>
      )}
    </InspectorRow>
  );
}

function CommandButton({ node, label, blocked, leading }: ControlProps) {
  const run = useCommandRunner();
  return (
    <InspectorRow label={label} blocked={blocked} feature={node.name}>
      {({ id, describedBy }) => (
        <>
          {leading}
          <button
            id={id}
            type="button"
            className="btn btn--secondary iv-row__button"
            aria-label={`Execute ${label}`}
            aria-describedby={describedBy}
            disabled={blocked !== null}
            onClick={() => void run(node.name)}
          >
            Execute
          </button>
        </>
      )}
    </InspectorRow>
  );
}

function TextControl({ node, state, label, blocked, leading }: ControlProps) {
  const write = useFeatureWriter();
  const [draft, setDraft] = useState<string | null>(null);
  const live = typeof state?.value === "string" ? state.value : "";
  const commit = () => {
    if (draft !== null && draft !== live) void write(node.name, draft);
    setDraft(null);
  };
  return (
    <InspectorRow label={label} blocked={blocked} feature={node.name}>
      {({ id, describedBy }) => (
        <>
          {leading}
          <input
            id={id}
            type="text"
            className="iv-text-input"
            aria-label={label}
            aria-describedby={describedBy}
            value={draft ?? live}
            disabled={blocked !== null}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") commit();
              if (e.key === "Escape") setDraft(null);
            }}
            onBlur={commit}
          />
        </>
      )}
    </InspectorRow>
  );
}

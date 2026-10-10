import { useState, type KeyboardEvent } from "react";
import type { UiNode } from "../../xml_model/uigraph";
import type { FeatureState, NumericRange } from "../../device/types";
import {
  useCommandRunner,
  useFeature,
  useFeatureWriter,
} from "../../device/FeatureStoreContext";
import { nodeDisplayName } from "../../xml_model/helpers";

/**
 * How a feature is drawn; `sfnc-groups.json` names one per feature. A Rust
 * test (`commands/sfnc_groups.rs`) keeps the bundled file to these names.
 */
const WIDGETS = [
  "float_slider",
  "int_slider",
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
}

/**
 * One feature as a control, driven by its live state (backlog ST-29).
 *
 * The range, the enumeration entries and whether the control is enabled all
 * come from the camera's `FeatureState`, not from guesses. A feature that is
 * unavailable, locked or read-only is shown greyed out with the reason
 * (ST-35); one the device does not implement is not shown.
 *
 * What the user types is kept only once they actually change it, and only
 * until they commit it (Enter, blur, or releasing the slider) or press
 * Escape. A field that merely has focus keeps following the camera, and
 * leaving it without an edit writes nothing (Codex review of #167).
 */
export function FeatureControl({ node, widget, disabledReason }: FeatureControlProps) {
  const state = useFeature(node?.name);
  if (!node) return null;
  if (state && !state.is_implemented) return null;

  const kind = isFeatureWidget(widget) ? widget : widgetForKind(node);
  if (!kind) return null;
  const label = nodeDisplayName(node);
  const blocked = disabledReason ?? blockedReason(state, kind);
  const props = { node, state, label, blocked };

  switch (kind) {
    case "float_slider":
    case "int_slider":
      return <NumericControl {...props} integer={kind === "int_slider"} />;
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
}

function widgetForKind(node: UiNode): FeatureWidget | null {
  switch (node.kind) {
    case "Float":
      return "float_slider";
    case "Integer":
      return "int_slider";
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

function unitOf(node: UiNode, state: FeatureState | undefined): string {
  return state?.unit ?? node.unit ?? "";
}

function Header({ label, value, blocked }: { label: string; value?: string; blocked: string | null }) {
  return (
    <div className="slider-row__header">
      <span className="slider-row__name" title={blocked ?? undefined}>
        {label}
      </span>
      {value !== undefined && <span className="slider-row__value">{value}</span>}
    </div>
  );
}

function Note({ blocked }: { blocked: string | null }) {
  return blocked ? <span className="feature-control__note">{blocked}</span> : null;
}

function NumericControl({ node, state, label, blocked, integer }: ControlProps & { integer: boolean }) {
  const write = useFeatureWriter();
  const [draft, setDraft] = useState<string | null>(null);
  const live = typeof state?.value === "number" ? state.value : null;
  const range = rangeOf(node, state);
  const unit = unitOf(node, state);
  const shown = draft ?? (live === null ? "" : String(live));

  const commit = (text: string | null) => {
    setDraft(null);
    if (text === null) return;
    let n = Number(text);
    if (text.trim() === "" || !Number.isFinite(n)) return;
    if (range) n = Math.min(range.max, Math.max(range.min, n));
    if (integer) n = Math.round(n);
    if (n !== live) void write(node.name, n);
  };
  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") commit(draft);
    if (e.key === "Escape") setDraft(null);
  };

  return (
    <div className="slider-row feature-control" data-feature={node.name}>
      <Header
        label={label}
        value={live === null ? undefined : `${live}${unit ? ` ${unit}` : ""}`}
        blocked={blocked}
      />
      <div className="slider-row__input-group">
        {range && (
          <input
            type="range"
            className="iv-slider"
            aria-label={`${label} slider`}
            min={range.min}
            max={range.max}
            step={integer ? (range.inc ?? 1) : "any"}
            value={shown === "" ? range.min : Number(shown)}
            disabled={blocked !== null}
            onChange={(e) => setDraft(e.target.value)}
            onPointerUp={(e) => commit((e.target as HTMLInputElement).value)}
            onKeyUp={(e) => {
              if (e.key.startsWith("Arrow") || e.key === "PageUp" || e.key === "PageDown") {
                commit((e.target as HTMLInputElement).value);
              }
            }}
          />
        )}
        <input
          type="number"
          className="iv-text-input"
          aria-label={label}
          value={shown}
          step={integer ? (range?.inc ?? 1) : "any"}
          disabled={blocked !== null}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={onKeyDown}
          onBlur={() => commit(draft)}
        />
      </div>
      <Note blocked={blocked} />
    </div>
  );
}

/** An integer chosen from its grid, such as a binning factor. */
function IntegerSelect({ node, state, label, blocked }: ControlProps) {
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
    <div className="auto-toggle-row feature-control" data-feature={node.name}>
      <span className="slider-row__name" title={blocked ?? undefined}>
        {label}
      </span>
      <select
        className="iv-auto-select"
        aria-label={label}
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
    </div>
  );
}

function EnumSelect({ node, state, label, blocked }: ControlProps) {
  const write = useFeatureWriter();
  const live = typeof state?.value === "string" ? state.value : null;
  const declared = node.enum_entries ?? [];
  const names = state?.enum_available ?? declared.map((e) => e.name);
  // The camera's current entry stays listed even if it is not selectable now.
  if (live !== null && !names.includes(live)) names.unshift(live);
  const displayName = (name: string) =>
    declared.find((e) => e.name === name)?.display_name ?? name;

  return (
    <div className="auto-toggle-row feature-control" data-feature={node.name}>
      <span className="slider-row__name" title={blocked ?? undefined}>
        {label}
      </span>
      <select
        className="iv-auto-select"
        aria-label={label}
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
    </div>
  );
}

function BooleanToggle({ node, state, label, blocked }: ControlProps) {
  const write = useFeatureWriter();
  const live = typeof state?.value === "boolean" ? state.value : false;
  return (
    <label className="auto-toggle-row feature-control" data-feature={node.name}>
      <span className="slider-row__name" title={blocked ?? undefined}>
        {label}
      </span>
      <input
        type="checkbox"
        aria-label={label}
        checked={live}
        disabled={blocked !== null}
        onChange={(e) => void write(node.name, e.target.checked)}
      />
    </label>
  );
}

function CommandButton({ node, label, blocked }: ControlProps) {
  const run = useCommandRunner();
  return (
    <div className="auto-toggle-row feature-control" data-feature={node.name}>
      <span className="slider-row__name">{label}</span>
      <button
        type="button"
        className="btn btn--sm"
        disabled={blocked !== null}
        title={blocked ?? undefined}
        onClick={() => void run(node.name)}
      >
        Execute
      </button>
    </div>
  );
}

function TextControl({ node, state, label, blocked }: ControlProps) {
  const write = useFeatureWriter();
  const [draft, setDraft] = useState<string | null>(null);
  const live = typeof state?.value === "string" ? state.value : "";
  const commit = () => {
    if (draft !== null && draft !== live) void write(node.name, draft);
    setDraft(null);
  };
  return (
    <div className="slider-row feature-control" data-feature={node.name}>
      <Header label={label} blocked={blocked} />
      <input
        type="text"
        className="iv-text-input"
        aria-label={label}
        value={draft ?? live}
        disabled={blocked !== null}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") commit();
          if (e.key === "Escape") setDraft(null);
        }}
        onBlur={commit}
      />
      <Note blocked={blocked} />
    </div>
  );
}

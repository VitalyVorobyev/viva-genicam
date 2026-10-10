import { useMemo } from "react";
import type { UiNode } from "../../../xml_model/uigraph";
import type { NodeValue, ValueError } from "../../../xml_model/values";
import type { FeatureState } from "../../../device/types";
import { ValidationErrors } from "./ValidationErrors";

interface EnumEditorProps {
  node: UiNode;
  value: NodeValue | undefined;
  errors: ValueError[];
  onChange: (value: NodeValue) => void;
  /**
   * Live device state. When present, its `enum_available` list is the source
   * of truth for dropdown options — the device may report only a subset of
   * the XML-declared entries as currently supported (e.g. PixelFormat on a
   * mono camera). When absent, we fall back to the static XML enumeration.
   */
  liveState?: FeatureState;
}

// Enumeration editor writes enum names into the shared draft store.
export function EnumEditor({ node, value, errors, onChange, liveState }: EnumEditorProps) {
  // Prefer the live-from-device enum entries over the static XML list so
  // the dropdown reflects what the device actually supports right now.
  const options =
    liveState?.enum_available && liveState.enum_available.length > 0
      ? liveState.enum_available.map((name) => {
          const match = node.enum_entries?.find((e) => e.name === name);
          return { value: name, label: match?.display_name ?? name };
        })
      : (node.enum_entries ?? []).map((entry) => ({
          value: entry.name,
          label: entry.display_name ?? entry.name,
        }));

  // Fall back to the live value when the draft is unset, so the form does
  // not render "(unset)" immediately after a successful Apply.
  let selected = "";
  if (isEnumValue(value)) {
    selected = value.enumName;
  } else if (value === undefined && typeof liveState?.value === "string") {
    selected = liveState.value;
  }
  const selectedLabel = useMemo(() => {
    if (!selected) {
      return null;
    }
    const match = options.find((option) => option.value === selected);
    return match?.label ?? selected;
  }, [options, selected]);

  if (options.length === 0) {
    return <div className="editor__hint">No enum entries available.</div>;
  }

  return (
    <div className="editor">
      <label className="editor__label">Value</label>
      <select
        className="editor__input"
        value={selected}
        onChange={(event) => {
          const next = event.target.value;
          if (!next) {
            onChange(null);
            return;
          }
          onChange({ enumName: next });
        }}
      >
        <option value="">(unset)</option>
        {options.map((option) => (
          <option key={option.value} value={option.value}>
            {option.label}
          </option>
        ))}
      </select>
      {selectedLabel && (
        <div className="editor__hint">Selected: {selectedLabel}</div>
      )}
      <ValidationErrors errors={errors} />
    </div>
  );
}

function isEnumValue(value: NodeValue | undefined): value is { enumName: string } {
  return (
    typeof value === "object" &&
    value !== null &&
    "enumName" in value &&
    typeof (value as { enumName?: unknown }).enumName === "string"
  );
}

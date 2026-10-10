import type { NodeValue, ValueError } from "../../../xml_model/values";
import type { FeatureState } from "../../../device/types";
import { ValidationErrors } from "./ValidationErrors";

interface BoolEditorProps {
  value: NodeValue | undefined;
  errors: ValueError[];
  onChange: (value: NodeValue) => void;
  /** Live device state; its value shows until the user edits the draft. */
  liveState?: FeatureState;
}

// Boolean editor writes into the shared draft store (offline mode).
export function BoolEditor({ value, errors, onChange, liveState }: BoolEditorProps) {
  let checked = false;
  if (typeof value === "boolean") {
    checked = value;
  } else if (value === undefined && typeof liveState?.value === "boolean") {
    checked = liveState.value;
  }

  return (
    <div className="editor">
      <label className="editor editor--inline">
        <input
          type="checkbox"
          checked={checked}
          onChange={(event) => onChange(event.target.checked)}
        />
        Enabled
      </label>
      <ValidationErrors errors={errors} />
    </div>
  );
}

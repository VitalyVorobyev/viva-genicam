import type { NodeValue, ValueError } from "../../../xml_model/values";
import type { FeatureState } from "../../../device/types";
import { ValidationErrors } from "./ValidationErrors";

interface StringEditorProps {
  value: NodeValue | undefined;
  errors: ValueError[];
  onChange: (value: NodeValue) => void;
  /** Live device state; its value shows until the user edits the draft. */
  liveState?: FeatureState;
}

// String editor writes into the shared draft store (offline mode).
export function StringEditor({ value, errors, onChange, liveState }: StringEditorProps) {
  let textValue = "";
  if (typeof value === "string") {
    textValue = value;
  } else if (value === undefined && typeof liveState?.value === "string") {
    textValue = liveState.value;
  }

  return (
    <div className="editor">
      <label className="editor__label">Value</label>
      <input
        className="editor__input"
        type="text"
        value={textValue}
        placeholder="unset (offline)"
        onChange={(event) => onChange(event.target.value)}
      />
      <ValidationErrors errors={errors} />
    </div>
  );
}

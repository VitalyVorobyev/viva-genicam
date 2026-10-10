import { useState, useRef, useEffect } from "react";
import type { ParseXmlResponse } from "../../xml_model/uigraph";
import type { NodeValueEntry } from "../../device/types";
import { isTauri } from "../../tauri";
import { getLiveNumber } from "./liveValueUtils";
import {
  extractNumericConstraints,
  clampValue,
  formatTriggerDelayLabel,
  resolveLiveEnumValue,
} from "./triggerUtils";

interface TriggerSectionProps {
  isConnected: boolean;
  externalModel: ParseXmlResponse | null;
  liveValues: Map<string, NodeValueEntry>;
}

const TRIGGER_DELAY_DEFAULTS = { min: 0, max: 1000000, inc: 0 };

async function writeNode(nodeName: string, value: number | string): Promise<void> {
  if (!isTauri()) return;
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("write_node", { nodeName, value });
  } catch (err) {
    console.error(`write_node(${nodeName}) failed:`, err);
  }
}

export function TriggerSection({
  isConnected,
  externalModel,
  liveValues,
}: TriggerSectionProps) {
  const nodes = externalModel?.graph.nodes_by_name;

  const delayConstraints = extractNumericConstraints(
    nodes?.["TriggerDelay"],
    TRIGGER_DELAY_DEFAULTS,
  );

  const [delayDraft, setDelayDraft] = useState<number>(delayConstraints.min);
  const [delayText, setDelayText] = useState<string>(String(delayConstraints.min));
  const delayDragging = useRef(false);

  const triggerModeEntries = nodes?.["TriggerMode"]?.enum_entries ?? [];
  const triggerSourceEntries = nodes?.["TriggerSource"]?.enum_entries ?? [];
  const triggerActivationEntries = nodes?.["TriggerActivation"]?.enum_entries ?? [];

  const triggerModeValue = resolveLiveEnumValue(liveValues, "TriggerMode");
  const triggerSourceValue = resolveLiveEnumValue(liveValues, "TriggerSource");
  const triggerActivationValue = resolveLiveEnumValue(liveValues, "TriggerActivation");

  // Sync delay draft from liveValues when not dragging
  useEffect(() => {
    if (delayDragging.current) return;
    const v = getLiveNumber(liveValues, "TriggerDelay");
    if (v !== null) {
      const clamped = clampValue(v, delayConstraints.min, delayConstraints.max);
      setDelayDraft(clamped);
      setDelayText(String(clamped));
    }
  }, [liveValues, delayConstraints.min, delayConstraints.max]);

  // After every hook, so a connect or disconnect does not change how many
  // hooks this component calls.
  if (!isConnected) {
    return <p className="sidebar-placeholder">No device connected.</p>;
  }

  const handleDelaySliderPointerDown = () => {
    delayDragging.current = true;
  };

  const handleDelaySliderPointerUp = (e: React.PointerEvent<HTMLInputElement>) => {
    delayDragging.current = false;
    const newVal = parseFloat((e.target as HTMLInputElement).value);
    const clamped = clampValue(newVal, delayConstraints.min, delayConstraints.max);
    setDelayDraft(clamped);
    setDelayText(String(clamped));
    void writeNode("TriggerDelay", clamped);
  };

  const handleDelaySliderChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const newVal = parseFloat(e.target.value);
    setDelayDraft(newVal);
    setDelayText(String(newVal));
  };

  const handleDelayTextChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    setDelayText(e.target.value);
  };

  const handleDelayTextBlur = () => {
    const parsed = parseFloat(delayText);
    const clamped = isNaN(parsed)
      ? delayDraft
      : clampValue(parsed, delayConstraints.min, delayConstraints.max);
    setDelayDraft(clamped);
    setDelayText(String(clamped));
    void writeNode("TriggerDelay", clamped);
  };

  const handleTriggerModeChange = async (e: React.ChangeEvent<HTMLSelectElement>) => {
    await writeNode("TriggerMode", e.target.value);
  };

  const handleTriggerSourceChange = async (e: React.ChangeEvent<HTMLSelectElement>) => {
    await writeNode("TriggerSource", e.target.value);
  };

  const handleTriggerActivationChange = async (e: React.ChangeEvent<HTMLSelectElement>) => {
    await writeNode("TriggerActivation", e.target.value);
  };

  return (
    <div className="exp-gain-section">
      {triggerModeEntries.length > 0 && (
        <div className="if-enum-row">
          <span className="slider-row__name">Trigger Mode</span>
          <select
            className="iv-auto-select"
            value={triggerModeValue ?? ""}
            onChange={handleTriggerModeChange}
          >
            {triggerModeEntries.map((entry) => (
              <option key={entry.name} value={entry.name}>
                {entry.display_name ?? entry.name}
              </option>
            ))}
          </select>
        </div>
      )}

      {triggerSourceEntries.length > 0 && (
        <div className="if-enum-row">
          <span className="slider-row__name">Trigger Source</span>
          <select
            className="iv-auto-select"
            value={triggerSourceValue ?? ""}
            onChange={handleTriggerSourceChange}
          >
            {triggerSourceEntries.map((entry) => (
              <option key={entry.name} value={entry.name}>
                {entry.display_name ?? entry.name}
              </option>
            ))}
          </select>
        </div>
      )}

      {triggerActivationEntries.length > 0 && (
        <div className="if-enum-row">
          <span className="slider-row__name">Trigger Activation</span>
          <select
            className="iv-auto-select"
            value={triggerActivationValue ?? ""}
            onChange={handleTriggerActivationChange}
          >
            {triggerActivationEntries.map((entry) => (
              <option key={entry.name} value={entry.name}>
                {entry.display_name ?? entry.name}
              </option>
            ))}
          </select>
        </div>
      )}

      {nodes?.["TriggerDelay"] !== undefined && (
        <div className="slider-row">
          <div className="slider-row__header">
            <span className="slider-row__name">Trigger Delay</span>
            <span className="slider-row__value">{formatTriggerDelayLabel(delayDraft)}</span>
          </div>
          <input
            type="range"
            className="iv-slider"
            min={delayConstraints.min}
            max={delayConstraints.max}
            step="any"
            value={delayDraft}
            onPointerDown={handleDelaySliderPointerDown}
            onPointerUp={handleDelaySliderPointerUp}
            onChange={handleDelaySliderChange}
          />
          <input
            type="number"
            className="iv-text-input"
            value={delayText}
            onChange={handleDelayTextChange}
            onBlur={handleDelayTextBlur}
          />
        </div>
      )}
    </div>
  );
}

import { useState, useRef, useEffect } from "react";
import type { ParseXmlResponse } from "../../xml_model/uigraph";
import type { NodeValueEntry } from "../../device/types";
import {
  extractNumericConstraints,
  clampValue,
  formatExposureLabel,
  formatGainLabel,
  resolveAutoState,
  isAutoContinuous,
} from "./exposureGainUtils";
import { getLiveNumber } from "./liveValueUtils";
import { useNodeWriter } from "../../device/useNodeWriter";

interface ExposureGainSectionProps {
  isConnected: boolean;
  externalModel: ParseXmlResponse | null;
  liveValues: Map<string, NodeValueEntry>;
}

const EXP_DEFAULTS = { min: 100, max: 100_000 };
const GAIN_DEFAULTS = { min: 0.0, max: 24.0 };

export function ExposureGainSection({
  isConnected,
  externalModel,
  liveValues,
}: ExposureGainSectionProps) {
  const writeNode = useNodeWriter();
  const graph = externalModel?.graph;
  const nodes = graph?.nodes_by_name;

  const expConstraints = extractNumericConstraints(nodes?.["ExposureTime"], EXP_DEFAULTS);
  const gainConstraints = extractNumericConstraints(nodes?.["Gain"], GAIN_DEFAULTS);

  const [expDraft, setExpDraft] = useState<number>(expConstraints.min);
  const [expText, setExpText] = useState<string>(String(expConstraints.min));
  const [gainDraft, setGainDraft] = useState<number>(gainConstraints.min);
  const [gainText, setGainText] = useState<string>(String(gainConstraints.min));

  const expDragging = useRef(false);
  const expEditing = useRef(false);
  const gainDragging = useRef(false);
  const gainEditing = useRef(false);

  const expAutoState = resolveAutoState(liveValues, "ExposureAuto");
  const gainAutoState = resolveAutoState(liveValues, "GainAuto");
  const expAutoContinuous = isAutoContinuous(expAutoState);
  const gainAutoContinuous = isAutoContinuous(gainAutoState);

  const expAutoEntries = nodes?.["ExposureAuto"]?.enum_entries ?? [];
  const gainAutoEntries = nodes?.["GainAuto"]?.enum_entries ?? [];

  // Sync draft from liveValues when not dragging
  const liveExp = getLiveNumber(liveValues, "ExposureTime");
  useEffect(() => {
    if (expDragging.current || expEditing.current) return;
    const v = liveExp;
    if (v !== null) {
      const clamped = clampValue(v, expConstraints.min, expConstraints.max);
      setExpDraft(clamped);
      setExpText(String(clamped));
    }
  }, [liveExp, expConstraints.min, expConstraints.max]);

  const liveGain = getLiveNumber(liveValues, "Gain");
  useEffect(() => {
    if (gainDragging.current || gainEditing.current) return;
    const v = liveGain;
    if (v !== null) {
      const clamped = clampValue(v, gainConstraints.min, gainConstraints.max);
      setGainDraft(clamped);
      setGainText(String(clamped));
    }
  }, [liveGain, gainConstraints.min, gainConstraints.max]);

  if (!isConnected) {
    return <p className="sidebar-placeholder">No device connected.</p>;
  }

  const handleExpSliderPointerDown = () => {
    expDragging.current = true;
  };

  const handleExpSliderPointerUp = (e: React.PointerEvent<HTMLInputElement>) => {
    expDragging.current = false;
    const newVal = parseFloat((e.target as HTMLInputElement).value);
    const clamped = clampValue(newVal, expConstraints.min, expConstraints.max);
    setExpDraft(clamped);
    setExpText(String(clamped));
    void writeNode("ExposureTime", clamped);
  };

  const handleExpSliderChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const newVal = parseFloat(e.target.value);
    setExpDraft(newVal);
    setExpText(String(newVal));
  };

  const handleExpTextChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    setExpText(e.target.value);
  };

  const handleExpTextBlur = () => {
    expEditing.current = false;
    const parsed = parseFloat(expText);
    const clamped = isNaN(parsed)
      ? expDraft
      : clampValue(parsed, expConstraints.min, expConstraints.max);
    setExpDraft(clamped);
    setExpText(String(clamped));
    void writeNode("ExposureTime", clamped);
  };

  const handleGainSliderPointerDown = () => {
    gainDragging.current = true;
  };

  const handleGainSliderPointerUp = (e: React.PointerEvent<HTMLInputElement>) => {
    gainDragging.current = false;
    const newVal = parseFloat((e.target as HTMLInputElement).value);
    const clamped = clampValue(newVal, gainConstraints.min, gainConstraints.max);
    setGainDraft(clamped);
    setGainText(String(clamped));
    void writeNode("Gain", clamped);
  };

  const handleGainSliderChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const newVal = parseFloat(e.target.value);
    setGainDraft(newVal);
    setGainText(String(newVal));
  };

  const handleGainTextChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    setGainText(e.target.value);
  };

  const handleGainTextBlur = () => {
    gainEditing.current = false;
    const parsed = parseFloat(gainText);
    const clamped = isNaN(parsed)
      ? gainDraft
      : clampValue(parsed, gainConstraints.min, gainConstraints.max);
    setGainDraft(clamped);
    setGainText(String(clamped));
    void writeNode("Gain", clamped);
  };

  const handleExpAutoChange = async (e: React.ChangeEvent<HTMLSelectElement>) => {
    const newVal = e.target.value;
    await writeNode("ExposureAuto", newVal);
  };

  const handleGainAutoChange = async (e: React.ChangeEvent<HTMLSelectElement>) => {
    const newVal = e.target.value;
    await writeNode("GainAuto", newVal);
  };

  return (
    <div className="exp-gain-section">
      {/* ExposureTime */}
      <div className="slider-row">
        <div className="slider-row__header">
          <span className="slider-row__name">Exposure Time</span>
          <span className="slider-row__value">{formatExposureLabel(expDraft)}</span>
        </div>
        <div className="slider-row__input-group">
          <input
            type="range"
            className="iv-slider"
            min={expConstraints.min}
            max={expConstraints.max}
            step="any"
            value={expDraft}
            disabled={expAutoContinuous}
            onPointerDown={handleExpSliderPointerDown}
            onPointerUp={handleExpSliderPointerUp}
            onChange={handleExpSliderChange}
          />
          <input
            type="number"
            className="iv-text-input"
            value={expText}
            disabled={expAutoContinuous}
            onChange={handleExpTextChange}
            onFocus={() => {
              expEditing.current = true;
            }}
            onBlur={handleExpTextBlur}
          />
        </div>
        {expAutoEntries.length > 0 && (
          <div className="auto-toggle-row">
            <span className="slider-row__name">Exposure Auto</span>
            <select
              className="iv-auto-select"
              value={expAutoState ?? ""}
              onChange={handleExpAutoChange}
            >
              {expAutoEntries.map((entry) => (
                <option key={entry.name} value={entry.name}>
                  {entry.display_name ?? entry.name}
                </option>
              ))}
            </select>
          </div>
        )}
      </div>

      {/* Gain */}
      <div className="slider-row">
        <div className="slider-row__header">
          <span className="slider-row__name">Gain</span>
          <span className="slider-row__value">{formatGainLabel(gainDraft)}</span>
        </div>
        <div className="slider-row__input-group">
          <input
            type="range"
            className="iv-slider"
            min={gainConstraints.min}
            max={gainConstraints.max}
            step="any"
            value={gainDraft}
            disabled={gainAutoContinuous}
            onPointerDown={handleGainSliderPointerDown}
            onPointerUp={handleGainSliderPointerUp}
            onChange={handleGainSliderChange}
          />
          <input
            type="number"
            className="iv-text-input"
            value={gainText}
            disabled={gainAutoContinuous}
            onChange={handleGainTextChange}
            onFocus={() => {
              gainEditing.current = true;
            }}
            onBlur={handleGainTextBlur}
          />
        </div>
        {gainAutoEntries.length > 0 && (
          <div className="auto-toggle-row">
            <span className="slider-row__name">Gain Auto</span>
            <select
              className="iv-auto-select"
              value={gainAutoState ?? ""}
              onChange={handleGainAutoChange}
            >
              {gainAutoEntries.map((entry) => (
                <option key={entry.name} value={entry.name}>
                  {entry.display_name ?? entry.name}
                </option>
              ))}
            </select>
          </div>
        )}
      </div>
    </div>
  );
}

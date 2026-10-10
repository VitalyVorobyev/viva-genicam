import { useState, useRef, useEffect } from "react";
import type { ParseXmlResponse } from "../../xml_model/uigraph";
import type { NodeValueEntry } from "../../device/types";
import { getLiveNumber } from "./liveValueUtils";
import {
  extractNumericConstraints,
  clampValue,
  formatBalanceRatioLabel,
  formatGammaLabel,
  resolveLiveEnumValue,
} from "./colorProcessingUtils";
import { useNodeWriter } from "../../device/useNodeWriter";

interface ColorProcessingSectionProps {
  isConnected: boolean;
  externalModel: ParseXmlResponse | null;
  liveValues: Map<string, NodeValueEntry>;
}

const BALANCE_RATIO_DEFAULTS = { min: 0.0, max: 4.0, inc: 0 };
const GAMMA_DEFAULTS = { min: 0.1, max: 4.0, inc: 0 };

export function ColorProcessingSection({
  isConnected,
  externalModel,
  liveValues,
}: ColorProcessingSectionProps) {
  const writeNode = useNodeWriter();
  const graph = externalModel?.graph;
  const nodes = graph?.nodes_by_name;

  const balanceRatioConstraints = extractNumericConstraints(
    nodes?.["BalanceRatio"],
    BALANCE_RATIO_DEFAULTS,
  );
  const gammaConstraints = extractNumericConstraints(nodes?.["Gamma"], GAMMA_DEFAULTS);

  const [balanceRatioDraft, setBalanceRatioDraft] = useState<number>(
    balanceRatioConstraints.min,
  );
  const [balanceRatioText, setBalanceRatioText] = useState<string>(
    String(balanceRatioConstraints.min),
  );
  const [gammaDraft, setGammaDraft] = useState<number>(gammaConstraints.min);
  const [gammaText, setGammaText] = useState<string>(String(gammaConstraints.min));

  const balanceRatioDragging = useRef(false);
  const balanceRatioEditing = useRef(false);
  const gammaDragging = useRef(false);
  const gammaEditing = useRef(false);

  const liveBalanceRatio = getLiveNumber(liveValues, "BalanceRatio");
  useEffect(() => {
    if (balanceRatioDragging.current || balanceRatioEditing.current) return;
    const v = liveBalanceRatio;
    if (v !== null) {
      const clamped = clampValue(v, balanceRatioConstraints.min, balanceRatioConstraints.max);
      setBalanceRatioDraft(clamped);
      setBalanceRatioText(String(clamped));
    }
  }, [liveBalanceRatio, balanceRatioConstraints.min, balanceRatioConstraints.max]);

  const liveGamma = getLiveNumber(liveValues, "Gamma");
  useEffect(() => {
    if (gammaDragging.current || gammaEditing.current) return;
    const v = liveGamma;
    if (v !== null) {
      const clamped = clampValue(v, gammaConstraints.min, gammaConstraints.max);
      setGammaDraft(clamped);
      setGammaText(String(clamped));
    }
  }, [liveGamma, gammaConstraints.min, gammaConstraints.max]);

  if (!isConnected) {
    return <p className="sidebar-placeholder">No device connected.</p>;
  }

  const balanceRatioSelectorEntries = nodes?.["BalanceRatioSelector"]?.enum_entries ?? [];
  const lutSelectorEntries = nodes?.["LUTSelector"]?.enum_entries ?? [];
  const hasBalanceRatio = nodes?.["BalanceRatio"] !== undefined;
  const hasGamma = nodes?.["Gamma"] !== undefined;
  const hasLutEnable = nodes?.["LUTEnable"] !== undefined;

  const hasAnyNode =
    balanceRatioSelectorEntries.length > 0 ||
    hasBalanceRatio ||
    hasGamma ||
    hasLutEnable ||
    lutSelectorEntries.length > 0;

  if (!hasAnyNode) {
    return <p className="sidebar-placeholder">No color processing nodes available.</p>;
  }

  const balanceRatioSelectorValue = resolveLiveEnumValue(liveValues, "BalanceRatioSelector");
  const lutSelectorValue = resolveLiveEnumValue(liveValues, "LUTSelector");
  const lutEnableValue = liveValues.get("LUTEnable")?.value as boolean | undefined;

  const handleBalanceRatioSelectorChange = async (
    e: React.ChangeEvent<HTMLSelectElement>,
  ) => {
    await writeNode("BalanceRatioSelector", e.target.value);
  };

  const handleBalanceRatioSliderPointerDown = () => {
    balanceRatioDragging.current = true;
  };

  const handleBalanceRatioSliderPointerUp = (e: React.PointerEvent<HTMLInputElement>) => {
    balanceRatioDragging.current = false;
    const newVal = parseFloat((e.target as HTMLInputElement).value);
    const clamped = clampValue(newVal, balanceRatioConstraints.min, balanceRatioConstraints.max);
    setBalanceRatioDraft(clamped);
    setBalanceRatioText(String(clamped));
    void writeNode("BalanceRatio", clamped);
  };

  const handleBalanceRatioSliderChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const newVal = parseFloat(e.target.value);
    setBalanceRatioDraft(newVal);
    setBalanceRatioText(String(newVal));
  };

  const handleBalanceRatioTextChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    setBalanceRatioText(e.target.value);
  };

  const handleBalanceRatioTextBlur = () => {
    balanceRatioEditing.current = false;
    const parsed = parseFloat(balanceRatioText);
    const clamped = isNaN(parsed)
      ? balanceRatioDraft
      : clampValue(parsed, balanceRatioConstraints.min, balanceRatioConstraints.max);
    setBalanceRatioDraft(clamped);
    setBalanceRatioText(String(clamped));
    void writeNode("BalanceRatio", clamped);
  };

  const handleGammaSliderPointerDown = () => {
    gammaDragging.current = true;
  };

  const handleGammaSliderPointerUp = (e: React.PointerEvent<HTMLInputElement>) => {
    gammaDragging.current = false;
    const newVal = parseFloat((e.target as HTMLInputElement).value);
    const clamped = clampValue(newVal, gammaConstraints.min, gammaConstraints.max);
    setGammaDraft(clamped);
    setGammaText(String(clamped));
    void writeNode("Gamma", clamped);
  };

  const handleGammaSliderChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const newVal = parseFloat(e.target.value);
    setGammaDraft(newVal);
    setGammaText(String(newVal));
  };

  const handleGammaTextChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    setGammaText(e.target.value);
  };

  const handleGammaTextBlur = () => {
    gammaEditing.current = false;
    const parsed = parseFloat(gammaText);
    const clamped = isNaN(parsed)
      ? gammaDraft
      : clampValue(parsed, gammaConstraints.min, gammaConstraints.max);
    setGammaDraft(clamped);
    setGammaText(String(clamped));
    void writeNode("Gamma", clamped);
  };

  const handleLutEnableChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    void writeNode("LUTEnable", e.target.checked);
  };

  const handleLutSelectorChange = async (e: React.ChangeEvent<HTMLSelectElement>) => {
    await writeNode("LUTSelector", e.target.value);
  };

  return (
    <div className="exp-gain-section">
      {balanceRatioSelectorEntries.length > 0 && (
        <div className="if-enum-row">
          <span className="slider-row__name">Balance Ratio Selector</span>
          <select
            className="iv-auto-select"
            value={balanceRatioSelectorValue ?? ""}
            onChange={handleBalanceRatioSelectorChange}
          >
            {balanceRatioSelectorEntries.map((entry) => (
              <option key={entry.name} value={entry.name}>
                {entry.display_name ?? entry.name}
              </option>
            ))}
          </select>
        </div>
      )}

      {hasBalanceRatio && (
        <div className="slider-row">
          <div className="slider-row__header">
            <span className="slider-row__name">Balance Ratio</span>
            <span className="slider-row__value">
              {formatBalanceRatioLabel(balanceRatioDraft)}
            </span>
          </div>
          <input
            type="range"
            className="iv-slider"
            min={balanceRatioConstraints.min}
            max={balanceRatioConstraints.max}
            step="any"
            value={balanceRatioDraft}
            onPointerDown={handleBalanceRatioSliderPointerDown}
            onPointerUp={handleBalanceRatioSliderPointerUp}
            onChange={handleBalanceRatioSliderChange}
          />
          <input
            type="number"
            className="iv-text-input"
            value={balanceRatioText}
            onChange={handleBalanceRatioTextChange}
            onFocus={() => {
              balanceRatioEditing.current = true;
            }}
            onBlur={handleBalanceRatioTextBlur}
          />
        </div>
      )}

      {hasGamma && (
        <div className="slider-row">
          <div className="slider-row__header">
            <span className="slider-row__name">Gamma</span>
            <span className="slider-row__value">{formatGammaLabel(gammaDraft)}</span>
          </div>
          <input
            type="range"
            className="iv-slider"
            min={gammaConstraints.min}
            max={gammaConstraints.max}
            step="any"
            value={gammaDraft}
            onPointerDown={handleGammaSliderPointerDown}
            onPointerUp={handleGammaSliderPointerUp}
            onChange={handleGammaSliderChange}
          />
          <input
            type="number"
            className="iv-text-input"
            value={gammaText}
            onChange={handleGammaTextChange}
            onFocus={() => {
              gammaEditing.current = true;
            }}
            onBlur={handleGammaTextBlur}
          />
        </div>
      )}

      {hasLutEnable && (
        <div className="bool-row">
          <label className="slider-row__name" htmlFor="lut-enable-toggle">
            LUT Enable
          </label>
          <input
            id="lut-enable-toggle"
            type="checkbox"
            checked={lutEnableValue ?? false}
            onChange={handleLutEnableChange}
          />
        </div>
      )}

      {lutSelectorEntries.length > 0 && (
        <div className="if-enum-row">
          <span className="slider-row__name">LUT Selector</span>
          <select
            className="iv-auto-select"
            value={lutSelectorValue ?? ""}
            onChange={handleLutSelectorChange}
          >
            {lutSelectorEntries.map((entry) => (
              <option key={entry.name} value={entry.name}>
                {entry.display_name ?? entry.name}
              </option>
            ))}
          </select>
        </div>
      )}
    </div>
  );
}

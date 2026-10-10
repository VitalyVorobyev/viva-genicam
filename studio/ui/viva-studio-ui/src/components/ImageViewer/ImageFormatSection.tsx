import { useState, useRef, useEffect } from "react";
import type { ParseXmlResponse } from "../../xml_model/uigraph";
import type { NodeValueEntry } from "../../device/types";
import {
  extractNumericConstraints,
  clampValue,
  resolveIntStep,
  formatPixelDimension,
  deriveOffsetMax,
} from "./imageFormatUtils";
import { getLiveNumber } from "./liveValueUtils";
import { useNodeWriter } from "../../device/useNodeWriter";

interface ImageFormatSectionProps {
  isConnected: boolean;
  externalModel: ParseXmlResponse | null;
  liveValues: Map<string, NodeValueEntry>;
}

const WIDTH_DEFAULTS = { min: 1, max: 4096, inc: 1 };
const HEIGHT_DEFAULTS = { min: 1, max: 3072, inc: 1 };
const OFFSET_X_DEFAULTS = { min: 0, max: 4095, inc: 1 };
const OFFSET_Y_DEFAULTS = { min: 0, max: 3071, inc: 1 };

export function ImageFormatSection({
  isConnected,
  externalModel,
  liveValues,
}: ImageFormatSectionProps) {
  const writeNode = useNodeWriter();
  const graph = externalModel?.graph;
  const nodes = graph?.nodes_by_name;

  const widthC = extractNumericConstraints(nodes?.["Width"], WIDTH_DEFAULTS);
  const heightC = extractNumericConstraints(nodes?.["Height"], HEIGHT_DEFAULTS);
  const offsetXC = extractNumericConstraints(nodes?.["OffsetX"], OFFSET_X_DEFAULTS);
  const offsetYC = extractNumericConstraints(nodes?.["OffsetY"], OFFSET_Y_DEFAULTS);

  const widthStep = resolveIntStep(widthC);
  const heightStep = resolveIntStep(heightC);
  const offsetXStep = resolveIntStep(offsetXC);
  const offsetYStep = resolveIntStep(offsetYC);

  const [widthDraft, setWidthDraft] = useState<number>(
    () => getLiveNumber(liveValues, "Width") ?? widthC.min
  );
  const [widthText, setWidthText] = useState<string>(
    () => String(getLiveNumber(liveValues, "Width") ?? widthC.min)
  );
  const [heightDraft, setHeightDraft] = useState<number>(
    () => getLiveNumber(liveValues, "Height") ?? heightC.min
  );
  const [heightText, setHeightText] = useState<string>(
    () => String(getLiveNumber(liveValues, "Height") ?? heightC.min)
  );
  const [offsetXDraft, setOffsetXDraft] = useState<number>(
    () => getLiveNumber(liveValues, "OffsetX") ?? offsetXC.min
  );
  const [offsetXText, setOffsetXText] = useState<string>(
    () => String(getLiveNumber(liveValues, "OffsetX") ?? offsetXC.min)
  );
  const [offsetYDraft, setOffsetYDraft] = useState<number>(
    () => getLiveNumber(liveValues, "OffsetY") ?? offsetYC.min
  );
  const [offsetYText, setOffsetYText] = useState<string>(
    () => String(getLiveNumber(liveValues, "OffsetY") ?? offsetYC.min)
  );

  const widthDragging = useRef(false);
  const widthEditing = useRef(false);
  const heightDragging = useRef(false);
  const heightEditing = useRef(false);
  const offsetXDragging = useRef(false);
  const offsetXEditing = useRef(false);
  const offsetYDragging = useRef(false);
  const offsetYEditing = useRef(false);

  const pixelFormatEntries = nodes?.["PixelFormat"]?.enum_entries ?? [];
  const livePixelFormat = liveValues.get("PixelFormat");
  const pixelFormatValue =
    typeof livePixelFormat?.value === "string"
      ? livePixelFormat.value
      : "";

  const binningHNode = nodes?.["BinningHorizontal"];
  const binningVNode = nodes?.["BinningVertical"];
  const hasBinningH = binningHNode !== undefined;
  const hasBinningV = binningVNode !== undefined;
  const hasBinning = hasBinningH || hasBinningV;

  // Derive binning option lists from integer min/max/inc constraints
  function buildBinningOptions(node: typeof binningHNode): number[] {
    if (node === undefined) return [];
    const c = extractNumericConstraints(node, { min: 1, max: 4, inc: 1 });
    const step = resolveIntStep(c);
    const options: number[] = [];
    for (let v = c.min; v <= c.max; v += step) {
      options.push(v);
    }
    return options;
  }

  const binningHOptions = buildBinningOptions(binningHNode);
  const binningVOptions = buildBinningOptions(binningVNode);

  const liveBinningH = getLiveNumber(liveValues, "BinningHorizontal");
  const liveBinningV = getLiveNumber(liveValues, "BinningVertical");
  const binningHValue = liveBinningH !== null ? String(liveBinningH) : String(binningHOptions[0] ?? 1);
  const binningVValue = liveBinningV !== null ? String(liveBinningV) : String(binningVOptions[0] ?? 1);

  // Dynamic offset maxima derived from live sensor/ROI dimensions
  const liveSensorW = getLiveNumber(liveValues, "SensorWidth");
  const liveSensorH = getLiveNumber(liveValues, "SensorHeight");
  const liveWidth = getLiveNumber(liveValues, "Width");
  const liveHeight = getLiveNumber(liveValues, "Height");

  const effectiveOffsetXMax = deriveOffsetMax(liveSensorW, liveWidth ?? widthDraft, offsetXC.max);
  const effectiveOffsetYMax = deriveOffsetMax(liveSensorH, liveHeight ?? heightDraft, offsetYC.max);

  // Sync drafts from liveValues when not dragging
  useEffect(() => {
    if (widthDragging.current || widthEditing.current) return;
    const v = liveWidth;
    if (v !== null) {
      const clamped = clampValue(v, widthC.min, widthC.max);
      setWidthDraft(clamped);
      setWidthText(String(clamped));
    }
  }, [liveWidth, widthC.min, widthC.max]);

  useEffect(() => {
    if (heightDragging.current || heightEditing.current) return;
    const v = liveHeight;
    if (v !== null) {
      const clamped = clampValue(v, heightC.min, heightC.max);
      setHeightDraft(clamped);
      setHeightText(String(clamped));
    }
  }, [liveHeight, heightC.min, heightC.max]);

  const liveOffsetX = getLiveNumber(liveValues, "OffsetX");
  useEffect(() => {
    if (offsetXDragging.current || offsetXEditing.current) return;
    const v = liveOffsetX;
    if (v !== null) {
      const clamped = clampValue(v, offsetXC.min, effectiveOffsetXMax);
      setOffsetXDraft(clamped);
      setOffsetXText(String(clamped));
    }
  }, [liveOffsetX, offsetXC.min, effectiveOffsetXMax]);

  const liveOffsetY = getLiveNumber(liveValues, "OffsetY");
  useEffect(() => {
    if (offsetYDragging.current || offsetYEditing.current) return;
    const v = liveOffsetY;
    if (v !== null) {
      const clamped = clampValue(v, offsetYC.min, effectiveOffsetYMax);
      setOffsetYDraft(clamped);
      setOffsetYText(String(clamped));
    }
  }, [liveOffsetY, offsetYC.min, effectiveOffsetYMax]);

  if (!isConnected) {
    return <p className="sidebar-placeholder">No device connected.</p>;
  }

  // ── PixelFormat ──────────────────────────────────────────────────────────────

  const handlePixelFormatChange = async (e: React.ChangeEvent<HTMLSelectElement>) => {
    await writeNode("PixelFormat", e.target.value);
  };

  // ── Width ────────────────────────────────────────────────────────────────────

  const handleWidthPointerDown = () => { widthDragging.current = true; };
  const handleWidthPointerUp = (e: React.PointerEvent<HTMLInputElement>) => {
    widthDragging.current = false;
    const newVal = parseInt((e.target as HTMLInputElement).value, 10);
    const clamped = clampValue(newVal, widthC.min, widthC.max);
    setWidthDraft(clamped);
    setWidthText(String(clamped));
    void writeNode("Width", clamped);
  };
  const handleWidthSliderChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const newVal = parseInt(e.target.value, 10);
    setWidthDraft(newVal);
    setWidthText(String(newVal));
  };
  const handleWidthTextChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    setWidthText(e.target.value);
  };
  const handleWidthTextBlur = () => {
    widthEditing.current = false;
    const parsed = parseInt(widthText, 10);
    const clamped = isNaN(parsed) ? widthDraft : clampValue(parsed, widthC.min, widthC.max);
    setWidthDraft(clamped);
    setWidthText(String(clamped));
    void writeNode("Width", clamped);
  };

  // ── Height ───────────────────────────────────────────────────────────────────

  const handleHeightPointerDown = () => { heightDragging.current = true; };
  const handleHeightPointerUp = (e: React.PointerEvent<HTMLInputElement>) => {
    heightDragging.current = false;
    const newVal = parseInt((e.target as HTMLInputElement).value, 10);
    const clamped = clampValue(newVal, heightC.min, heightC.max);
    setHeightDraft(clamped);
    setHeightText(String(clamped));
    void writeNode("Height", clamped);
  };
  const handleHeightSliderChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const newVal = parseInt(e.target.value, 10);
    setHeightDraft(newVal);
    setHeightText(String(newVal));
  };
  const handleHeightTextChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    setHeightText(e.target.value);
  };
  const handleHeightTextBlur = () => {
    heightEditing.current = false;
    const parsed = parseInt(heightText, 10);
    const clamped = isNaN(parsed) ? heightDraft : clampValue(parsed, heightC.min, heightC.max);
    setHeightDraft(clamped);
    setHeightText(String(clamped));
    void writeNode("Height", clamped);
  };

  // ── OffsetX ──────────────────────────────────────────────────────────────────

  const handleOffsetXPointerDown = () => { offsetXDragging.current = true; };
  const handleOffsetXPointerUp = (e: React.PointerEvent<HTMLInputElement>) => {
    offsetXDragging.current = false;
    const newVal = parseInt((e.target as HTMLInputElement).value, 10);
    const clamped = clampValue(newVal, offsetXC.min, effectiveOffsetXMax);
    setOffsetXDraft(clamped);
    setOffsetXText(String(clamped));
    void writeNode("OffsetX", clamped);
  };
  const handleOffsetXSliderChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const newVal = parseInt(e.target.value, 10);
    setOffsetXDraft(newVal);
    setOffsetXText(String(newVal));
  };
  const handleOffsetXTextChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    setOffsetXText(e.target.value);
  };
  const handleOffsetXTextBlur = () => {
    offsetXEditing.current = false;
    const parsed = parseInt(offsetXText, 10);
    const clamped = isNaN(parsed) ? offsetXDraft : clampValue(parsed, offsetXC.min, effectiveOffsetXMax);
    setOffsetXDraft(clamped);
    setOffsetXText(String(clamped));
    void writeNode("OffsetX", clamped);
  };

  // ── OffsetY ──────────────────────────────────────────────────────────────────

  const handleOffsetYPointerDown = () => { offsetYDragging.current = true; };
  const handleOffsetYPointerUp = (e: React.PointerEvent<HTMLInputElement>) => {
    offsetYDragging.current = false;
    const newVal = parseInt((e.target as HTMLInputElement).value, 10);
    const clamped = clampValue(newVal, offsetYC.min, effectiveOffsetYMax);
    setOffsetYDraft(clamped);
    setOffsetYText(String(clamped));
    void writeNode("OffsetY", clamped);
  };
  const handleOffsetYSliderChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const newVal = parseInt(e.target.value, 10);
    setOffsetYDraft(newVal);
    setOffsetYText(String(newVal));
  };
  const handleOffsetYTextChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    setOffsetYText(e.target.value);
  };
  const handleOffsetYTextBlur = () => {
    offsetYEditing.current = false;
    const parsed = parseInt(offsetYText, 10);
    const clamped = isNaN(parsed) ? offsetYDraft : clampValue(parsed, offsetYC.min, effectiveOffsetYMax);
    setOffsetYDraft(clamped);
    setOffsetYText(String(clamped));
    void writeNode("OffsetY", clamped);
  };

  // ── Binning ──────────────────────────────────────────────────────────────────

  const handleBinningHChange = async (e: React.ChangeEvent<HTMLSelectElement>) => {
    await writeNode("BinningHorizontal", parseInt(e.target.value, 10));
  };
  const handleBinningVChange = async (e: React.ChangeEvent<HTMLSelectElement>) => {
    await writeNode("BinningVertical", parseInt(e.target.value, 10));
  };

  return (
    <div className="img-format-section">
      {/* PixelFormat */}
      {pixelFormatEntries.length > 0 && (
        <div className="if-enum-row">
          <span className="slider-row__name">Pixel Format</span>
          <select
            className="iv-auto-select"
            value={pixelFormatValue}
            onChange={handlePixelFormatChange}
          >
            {pixelFormatValue === "" && (
              <option value="" disabled>
                Unavailable
              </option>
            )}
            {pixelFormatEntries.map((entry) => (
              <option key={entry.name} value={entry.name}>
                {entry.display_name ?? entry.name}
              </option>
            ))}
          </select>
        </div>
      )}

      {/* Width */}
      <div className="slider-row">
        <div className="slider-row__header">
          <span className="slider-row__name">Width</span>
          <span className="slider-row__value">{formatPixelDimension(widthDraft)}</span>
        </div>
        <div className="slider-row__input-group">
          <input
            type="range"
            className="iv-slider"
            min={widthC.min}
            max={widthC.max}
            step={widthStep}
            value={widthDraft}
            onPointerDown={handleWidthPointerDown}
            onPointerUp={handleWidthPointerUp}
            onChange={handleWidthSliderChange}
          />
          <input
            type="number"
            className="iv-text-input"
            value={widthText}
            onChange={handleWidthTextChange}
            onFocus={() => {
              widthEditing.current = true;
            }}
            onBlur={handleWidthTextBlur}
          />
        </div>
      </div>

      {/* Height */}
      <div className="slider-row">
        <div className="slider-row__header">
          <span className="slider-row__name">Height</span>
          <span className="slider-row__value">{formatPixelDimension(heightDraft)}</span>
        </div>
        <div className="slider-row__input-group">
          <input
            type="range"
            className="iv-slider"
            min={heightC.min}
            max={heightC.max}
            step={heightStep}
            value={heightDraft}
            onPointerDown={handleHeightPointerDown}
            onPointerUp={handleHeightPointerUp}
            onChange={handleHeightSliderChange}
          />
          <input
            type="number"
            className="iv-text-input"
            value={heightText}
            onChange={handleHeightTextChange}
            onFocus={() => {
              heightEditing.current = true;
            }}
            onBlur={handleHeightTextBlur}
          />
        </div>
      </div>

      {/* OffsetX */}
      <div className="slider-row">
        <div className="slider-row__header">
          <span className="slider-row__name">Offset X</span>
          <span className="slider-row__value">{formatPixelDimension(offsetXDraft)}</span>
        </div>
        <div className="slider-row__input-group">
          <input
            type="range"
            className="iv-slider"
            min={offsetXC.min}
            max={effectiveOffsetXMax}
            step={offsetXStep}
            value={offsetXDraft}
            onPointerDown={handleOffsetXPointerDown}
            onPointerUp={handleOffsetXPointerUp}
            onChange={handleOffsetXSliderChange}
          />
          <input
            type="number"
            className="iv-text-input"
            value={offsetXText}
            onChange={handleOffsetXTextChange}
            onFocus={() => {
              offsetXEditing.current = true;
            }}
            onBlur={handleOffsetXTextBlur}
          />
        </div>
      </div>

      {/* OffsetY */}
      <div className="slider-row">
        <div className="slider-row__header">
          <span className="slider-row__name">Offset Y</span>
          <span className="slider-row__value">{formatPixelDimension(offsetYDraft)}</span>
        </div>
        <div className="slider-row__input-group">
          <input
            type="range"
            className="iv-slider"
            min={offsetYC.min}
            max={effectiveOffsetYMax}
            step={offsetYStep}
            value={offsetYDraft}
            onPointerDown={handleOffsetYPointerDown}
            onPointerUp={handleOffsetYPointerUp}
            onChange={handleOffsetYSliderChange}
          />
          <input
            type="number"
            className="iv-text-input"
            value={offsetYText}
            onChange={handleOffsetYTextChange}
            onFocus={() => {
              offsetYEditing.current = true;
            }}
            onBlur={handleOffsetYTextBlur}
          />
        </div>
      </div>

      {/* Binning (optional — only when nodes exist in UiGraph) */}
      {hasBinning && (
        <div className="binning-row">
          <span className="binning-row__title">Binning</span>
          <div className="binning-row__controls">
            {hasBinningH && (
              <div className="if-enum-row">
                <span className="slider-row__name">Horizontal</span>
                <select
                  className="iv-auto-select"
                  value={binningHValue}
                  onChange={handleBinningHChange}
                >
                  {binningHOptions.map((v) => (
                    <option key={v} value={String(v)}>
                      {v}
                    </option>
                  ))}
                </select>
              </div>
            )}
            {hasBinningV && (
              <div className="if-enum-row">
                <span className="slider-row__name">Vertical</span>
                <select
                  className="iv-auto-select"
                  value={binningVValue}
                  onChange={handleBinningVChange}
                >
                  {binningVOptions.map((v) => (
                    <option key={v} value={String(v)}>
                      {v}
                    </option>
                  ))}
                </select>
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

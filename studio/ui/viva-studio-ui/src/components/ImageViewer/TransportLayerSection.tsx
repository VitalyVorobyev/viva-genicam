import { useState, useRef, useEffect } from "react";
import type { ParseXmlResponse } from "../../xml_model/uigraph";
import type { NodeValueEntry } from "../../device/types";
import { getLiveNumber } from "./liveValueUtils";
import {
  extractNumericConstraints,
  clampValue,
  resolveIntStep,
  formatPacketSizeLabel,
  formatInterPacketDelayLabel,
} from "./transportLayerUtils";
import { useNodeWriter } from "../../device/useNodeWriter";

interface TransportLayerSectionProps {
  isConnected: boolean;
  externalModel: ParseXmlResponse | null;
  liveValues: Map<string, NodeValueEntry>;
}

const PACKET_SIZE_DEFAULTS = { min: 220, max: 9000, inc: 4 };
const INTER_PACKET_DELAY_DEFAULTS = { min: 0, max: 10000, inc: 1 };

export function TransportLayerSection({
  isConnected,
  externalModel,
  liveValues,
}: TransportLayerSectionProps) {
  const writeNode = useNodeWriter();
  const nodes = externalModel?.graph.nodes_by_name;

  const packetSizeC = extractNumericConstraints(
    nodes?.["GevSCPSPacketSize"],
    PACKET_SIZE_DEFAULTS,
  );
  const interPacketDelayC = extractNumericConstraints(
    nodes?.["GevSCPD"],
    INTER_PACKET_DELAY_DEFAULTS,
  );

  const packetSizeStep = resolveIntStep(packetSizeC);
  const interPacketDelayStep = resolveIntStep(interPacketDelayC);

  const [packetSizeDraft, setPacketSizeDraft] = useState<number>(packetSizeC.min);
  const [packetSizeText, setPacketSizeText] = useState<string>(String(packetSizeC.min));
  const [interPacketDelayDraft, setInterPacketDelayDraft] = useState<number>(
    interPacketDelayC.min,
  );
  const [interPacketDelayText, setInterPacketDelayText] = useState<string>(
    String(interPacketDelayC.min),
  );

  const packetSizeDragging = useRef(false);
  const packetSizeEditing = useRef(false);
  const interPacketDelayDragging = useRef(false);
  const interPacketDelayEditing = useRef(false);

  const livePacketSize = getLiveNumber(liveValues, "GevSCPSPacketSize");
  useEffect(() => {
    if (packetSizeDragging.current || packetSizeEditing.current) return;
    const v = livePacketSize;
    if (v !== null) {
      const clamped = clampValue(v, packetSizeC.min, packetSizeC.max);
      setPacketSizeDraft(clamped);
      setPacketSizeText(String(clamped));
    }
  }, [livePacketSize, packetSizeC.min, packetSizeC.max]);

  const liveInterPacketDelay = getLiveNumber(liveValues, "GevSCPD");
  useEffect(() => {
    if (interPacketDelayDragging.current || interPacketDelayEditing.current) return;
    const v = liveInterPacketDelay;
    if (v !== null) {
      const clamped = clampValue(v, interPacketDelayC.min, interPacketDelayC.max);
      setInterPacketDelayDraft(clamped);
      setInterPacketDelayText(String(clamped));
    }
  }, [liveInterPacketDelay, interPacketDelayC.min, interPacketDelayC.max]);

  if (!isConnected) {
    return <p className="sidebar-placeholder">No device connected.</p>;
  }

  const hasPacketSize = nodes?.["GevSCPSPacketSize"] !== undefined;
  const hasInterPacketDelay = nodes?.["GevSCPD"] !== undefined;

  if (!hasPacketSize && !hasInterPacketDelay) {
    return <p className="sidebar-placeholder">No transport layer nodes available.</p>;
  }

  return (
    <div className="exp-gain-section">
      {hasPacketSize && (
        <div className="slider-row">
          <div className="slider-row__header">
            <span className="slider-row__name">Packet Size</span>
            <span className="slider-row__value">{formatPacketSizeLabel(packetSizeDraft)}</span>
          </div>
          <input
            type="range"
            className="iv-slider"
            min={packetSizeC.min}
            max={packetSizeC.max}
            step={packetSizeStep}
            value={packetSizeDraft}
            onPointerDown={() => {
              packetSizeDragging.current = true;
            }}
            onPointerUp={(e: React.PointerEvent<HTMLInputElement>) => {
              packetSizeDragging.current = false;
              const newVal = parseInt((e.target as HTMLInputElement).value, 10);
              const clamped = clampValue(newVal, packetSizeC.min, packetSizeC.max);
              setPacketSizeDraft(clamped);
              setPacketSizeText(String(clamped));
              void writeNode("GevSCPSPacketSize", clamped);
            }}
            onChange={(e: React.ChangeEvent<HTMLInputElement>) => {
              const newVal = parseInt(e.target.value, 10);
              setPacketSizeDraft(newVal);
              setPacketSizeText(String(newVal));
            }}
          />
          <input
            type="number"
            className="iv-text-input"
            value={packetSizeText}
            onFocus={() => {
              packetSizeEditing.current = true;
            }}
            onChange={(e: React.ChangeEvent<HTMLInputElement>) => {
              setPacketSizeText(e.target.value);
            }}
            onBlur={() => {
              packetSizeEditing.current = false;
              const parsed = parseInt(packetSizeText, 10);
              const clamped = isNaN(parsed)
                ? packetSizeDraft
                : clampValue(parsed, packetSizeC.min, packetSizeC.max);
              setPacketSizeDraft(clamped);
              setPacketSizeText(String(clamped));
              void writeNode("GevSCPSPacketSize", clamped);
            }}
          />
        </div>
      )}

      {hasInterPacketDelay && (
        <div className="slider-row">
          <div className="slider-row__header">
            <span className="slider-row__name">Inter-Packet Delay</span>
            <span className="slider-row__value">
              {formatInterPacketDelayLabel(interPacketDelayDraft)}
            </span>
          </div>
          <input
            type="range"
            className="iv-slider"
            min={interPacketDelayC.min}
            max={interPacketDelayC.max}
            step={interPacketDelayStep}
            value={interPacketDelayDraft}
            onPointerDown={() => {
              interPacketDelayDragging.current = true;
            }}
            onPointerUp={(e: React.PointerEvent<HTMLInputElement>) => {
              interPacketDelayDragging.current = false;
              const newVal = parseInt((e.target as HTMLInputElement).value, 10);
              const clamped = clampValue(newVal, interPacketDelayC.min, interPacketDelayC.max);
              setInterPacketDelayDraft(clamped);
              setInterPacketDelayText(String(clamped));
              void writeNode("GevSCPD", clamped);
            }}
            onChange={(e: React.ChangeEvent<HTMLInputElement>) => {
              const newVal = parseInt(e.target.value, 10);
              setInterPacketDelayDraft(newVal);
              setInterPacketDelayText(String(newVal));
            }}
          />
          <input
            type="number"
            className="iv-text-input"
            value={interPacketDelayText}
            onFocus={() => {
              interPacketDelayEditing.current = true;
            }}
            onChange={(e: React.ChangeEvent<HTMLInputElement>) => {
              setInterPacketDelayText(e.target.value);
            }}
            onBlur={() => {
              interPacketDelayEditing.current = false;
              const parsed = parseInt(interPacketDelayText, 10);
              const clamped = isNaN(parsed)
                ? interPacketDelayDraft
                : clampValue(parsed, interPacketDelayC.min, interPacketDelayC.max);
              setInterPacketDelayDraft(clamped);
              setInterPacketDelayText(String(clamped));
              void writeNode("GevSCPD", clamped);
            }}
          />
        </div>
      )}
    </div>
  );
}

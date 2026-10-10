import { useState } from "react";
import type { UiNode } from "../../xml_model/uigraph";
import {
  PACKET_SIZE_MAX,
  PACKET_SIZE_MIN,
  loadPacketSize,
  savePacketSize,
} from "../../device/packetSize";
import { FeatureControl } from "../controls/FeatureControl";
import { formatFrameCount } from "./viewerUtils";

interface AcquisitionSectionProps {
  isConnected: boolean;
  isAcquiring: boolean;
  frameCount: number;
  nodes: Record<string, UiNode>;
  deviceId: string | null;
}

/**
 * The acquisition settings, and the frames received. The camera's own
 * `AcquisitionStart`/`AcquisitionStop` commands are left out: the header's
 * Start and Stop also run the stream that carries the frames to the viewer.
 */
export function AcquisitionSection({
  isConnected,
  isAcquiring,
  frameCount,
  nodes,
  deviceId,
}: AcquisitionSectionProps) {
  if (!isConnected) {
    return <p className="sidebar-placeholder">No device connected.</p>;
  }
  const frameDisplay =
    !isAcquiring && frameCount === 0 ? "—" : formatFrameCount(frameCount);

  return (
    <div className="acq-section acq-section--compact">
      <FeatureControl
        node={nodes["AcquisitionMode"]}
        disabledReason={isAcquiring ? "Stop acquisition to change the mode" : undefined}
      />
      <FeatureControl node={nodes["AcquisitionFrameRateEnable"]} widget="bool_toggle" />
      <FeatureControl node={nodes["AcquisitionFrameRate"]} widget="float_slider" />
      {deviceId && <PacketSizeSetting key={deviceId} deviceId={deviceId} isAcquiring={isAcquiring} />}
      <div className="acq-compact-row">
        <span className="slider-row__name">Frames</span>
        <span className="frame-counter__value">{frameDisplay}</span>
      </div>
    </div>
  );
}

/**
 * The packet size this camera's stream starts with: negotiated from the NIC
 * by default, or one the user measured for the link (backlog ST-22, #112).
 * Applies from the next start.
 */
function PacketSizeSetting({ deviceId, isAcquiring }: { deviceId: string; isAcquiring: boolean }) {
  const [size, setSize] = useState<number | null>(() => loadPacketSize(deviceId));
  const [text, setText] = useState(() => String(size ?? 1500));
  const choose = (next: number | null) => {
    setSize(next);
    savePacketSize(deviceId, next);
  };
  const commit = () => {
    const n = Number(text);
    if (Number.isInteger(n) && n >= PACKET_SIZE_MIN && n <= PACKET_SIZE_MAX) choose(n);
    else setText(String(size ?? 1500));
  };
  const note = isAcquiring ? "Applies from the next start" : undefined;
  return (
    <div className="acq-compact-row" title={note}>
      <span className="slider-row__name">Packet size</span>
      <select
        className="iv-auto-select"
        aria-label="Packet size"
        value={size === null ? "auto" : "fixed"}
        onChange={(e) => choose(e.target.value === "auto" ? null : Number(text) || 1500)}
      >
        <option value="auto">Auto</option>
        <option value="fixed">Fixed</option>
      </select>
      {size !== null && (
        <input
          type="number"
          className="iv-text-input"
          aria-label="Packet size in bytes"
          min={PACKET_SIZE_MIN}
          max={PACKET_SIZE_MAX}
          value={text}
          onChange={(e) => setText(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => {
            if (e.key === "Enter") commit();
          }}
        />
      )}
    </div>
  );
}

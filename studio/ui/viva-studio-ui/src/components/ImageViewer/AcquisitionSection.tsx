import { useState } from "react";
import type { SfncGroup } from "../../device/types";
import { SfncFeatureControl } from "./SfncFeatureControl";
import type { UiNode } from "../../xml_model/uigraph";
import {
  PACKET_SIZE_MAX,
  PACKET_SIZE_MIN,
  loadPacketSize,
  savePacketSize,
} from "../../device/packetSize";
import { FeatureCheckbox, FeatureControl } from "../controls/FeatureControl";
import { InspectorRow } from "../controls/InspectorRow";

interface AcquisitionSectionProps {
  isConnected: boolean;
  isAcquiring: boolean;
  nodes: Record<string, UiNode>;
  deviceId: string | null;
  rateFeature?: SfncGroup["features"][number];
}

/**
 * The acquisition settings. The camera's own `AcquisitionStart`/
 * `AcquisitionStop` commands are left out: the header's Start and Stop also
 * run the stream that carries the frames to the viewer. The frames received
 * are counted in the Statistics section.
 */
export function AcquisitionSection({
  isConnected,
  isAcquiring,
  nodes,
  deviceId,
  rateFeature,
}: AcquisitionSectionProps) {
  if (!isConnected) {
    return <p className="sidebar-placeholder">No device connected.</p>;
  }
  const rate = rateFeature ?? { node: "AcquisitionFrameRate", widget: "float_input" };
  const hasRate = [rate.node, ...(rate.aliases ?? [])].some((name) => nodes[name]);
  const rateEnable = nodes["AcquisitionFrameRateEnable"];

  return (
    <div className="feature-group">
      <FeatureControl
        node={nodes["AcquisitionMode"]}
        disabledReason={isAcquiring ? "Stop acquisition to change the mode" : undefined}
      />
      {/* The switch that enables the frame rate sits before its field. */}
      {hasRate ? (
        <SfncFeatureControl
          feature={rate}
          nodes={nodes}
          leading={<FeatureCheckbox node={rateEnable} />}
        />
      ) : (
        <FeatureControl node={rateEnable} widget="bool_toggle" />
      )}
      {deviceId && <PacketSizeSetting key={deviceId} deviceId={deviceId} isAcquiring={isAcquiring} />}
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
  const hint = isAcquiring ? "Applies from the next start" : undefined;
  return (
    <>
      <InspectorRow label="Packet size" blocked={null} hint={hint}>
        {({ id }) => (
          <select
            id={id}
            className="iv-select"
            aria-label="Packet size"
            value={size === null ? "auto" : "fixed"}
            onChange={(e) => choose(e.target.value === "auto" ? null : Number(text) || 1500)}
          >
            <option value="auto">Auto</option>
            <option value="fixed">Fixed</option>
          </select>
        )}
      </InspectorRow>
      {size !== null && (
        <InspectorRow label="Fixed size" blocked={null} hint={hint}>
          {({ id }) => (
            <span className="iv-number">
              <input
                id={id}
                type="number"
                className="iv-number__input"
                aria-label="Packet size in bytes"
                title={`${PACKET_SIZE_MIN} – ${PACKET_SIZE_MAX} B`}
                min={PACKET_SIZE_MIN}
                max={PACKET_SIZE_MAX}
                value={text}
                onChange={(e) => setText(e.target.value)}
                onBlur={commit}
                onKeyDown={(e) => {
                  if (e.key === "Enter") commit();
                  if (e.key === "Escape") setText(String(size));
                }}
              />
              <span className="iv-number__unit" aria-hidden="true">
                B
              </span>
            </span>
          )}
        </InspectorRow>
      )}
    </>
  );
}

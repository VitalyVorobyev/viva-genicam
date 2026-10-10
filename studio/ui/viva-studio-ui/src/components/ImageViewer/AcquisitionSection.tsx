import type { EnumEntry } from "../../xml_model/uigraph";
import type { NodeValueEntry } from "../../device/types";
import { useNodeWriter } from "../../device/useNodeWriter";
import { formatFrameCount } from "./viewerUtils";
import { resolveLiveEnumValue } from "./triggerUtils";

interface AcquisitionSectionProps {
  isConnected: boolean;
  isAcquiring: boolean;
  frameCount: number;
  onStartAcq: () => Promise<void>;
  onStopAcq: () => Promise<void>;
  acquisitionModeEntries: EnumEntry[];
  liveValues: Map<string, NodeValueEntry>;
}

export function AcquisitionSection({
  isConnected,
  isAcquiring,
  frameCount,
  acquisitionModeEntries,
  liveValues,
}: AcquisitionSectionProps) {
  const writeNode = useNodeWriter();

  if (!isConnected) {
    return <p className="sidebar-placeholder">No device connected.</p>;
  }

  const handleModeChange = async (e: React.ChangeEvent<HTMLSelectElement>) => {
    await writeNode("AcquisitionMode", e.target.value);
  };

  const frameDisplay =
    !isAcquiring && frameCount === 0 ? "\u2014" : formatFrameCount(frameCount);
  const acquisitionModeValue = resolveLiveEnumValue(liveValues, "AcquisitionMode");

  return (
    <div className="acq-section acq-section--compact">
      {acquisitionModeEntries.length > 0 && (
        <div className="acq-compact-row">
          <span className="slider-row__name">Mode</span>
          <select
            className="iv-auto-select"
            disabled={isAcquiring}
            value={acquisitionModeValue ?? ""}
            onChange={handleModeChange}
          >
            {acquisitionModeValue === null && (
              <option value="" disabled>
                Unavailable
              </option>
            )}
            {acquisitionModeEntries.map((entry) => (
              <option key={entry.name} value={entry.name}>
                {entry.display_name ?? entry.name}
              </option>
            ))}
          </select>
        </div>
      )}
      <div className="acq-compact-row">
        <span className="slider-row__name">Frames</span>
        <span className="frame-counter__value">{frameDisplay}</span>
      </div>
    </div>
  );
}

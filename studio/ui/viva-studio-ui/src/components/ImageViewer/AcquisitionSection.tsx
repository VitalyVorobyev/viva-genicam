import type { UiNode } from "../../xml_model/uigraph";
import { FeatureControl } from "../controls/FeatureControl";
import { formatFrameCount } from "./viewerUtils";

interface AcquisitionSectionProps {
  isConnected: boolean;
  isAcquiring: boolean;
  frameCount: number;
  nodes: Record<string, UiNode>;
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
      <div className="acq-compact-row">
        <span className="slider-row__name">Frames</span>
        <span className="frame-counter__value">{frameDisplay}</span>
      </div>
    </div>
  );
}

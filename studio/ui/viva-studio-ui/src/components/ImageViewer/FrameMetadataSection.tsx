import type { FrameMetadata } from "../../device/types";
import { InspectorRow } from "../controls/InspectorRow";

/** Values from the image actually painted, or its explicit frozen capture. */
export function FrameMetadataSection({ metadata, frozen = false }: { metadata: FrameMetadata | null; frozen?: boolean }) {
  if (!metadata) return <p className="sidebar-placeholder">{frozen ? "This capture has no frame metadata." : "Waiting for a preview frame with metadata."}</p>;
  return (
    <div className="feature-group">
      <p className="iv-stats__caption">{frozen ? "Frozen frame" : "Displayed frame"}</p>
      <Value label="Frame ID" value={metadata.block_id} />
      <Value label="Device ticks" value={metadata.ts_dev ?? "Unavailable"} hint="Raw device clock ticks; these are not seconds." />
      <Value label="Host time (ns)" value={metadata.ts_host_ns ?? "Not calibrated"} hint="Device time mapped to Unix nanoseconds by clock calibration. Never the frame's receipt time." />
      <Value label="Complete" value={metadata.complete ? "Yes" : "No"} />
      {metadata.chunk_error && <p role="status">Chunk data: {metadata.chunk_error}</p>}
      {metadata.truncated && <p role="status">Some frame metadata exceeds the display limit and was omitted.</p>}
      <h4>Chunks</h4>
      {metadata.chunks.length === 0 ? <p className="sidebar-placeholder">No chunk data in this frame.</p> : metadata.chunks.map(chunk => (
        <Value key={chunk.id} label={`ID ${chunk.id}`} value={`${chunk.bytes} bytes`} />
      ))}
      {metadata.features.map((feature, index) => (
        <Value key={`${feature.name}:${index}`} label={feature.name} value={feature.error ? `Error: ${feature.error}` : feature.value ?? "Unavailable"} />
      ))}
    </div>
  );
}

function Value({ label, value, hint }: { label: string; value: string; hint?: string }) {
  return <InspectorRow label={label} blocked={null} hint={hint}>{({ id }) => <output aria-live="off" id={id} title={value} style={{ overflowWrap: "anywhere" }}>{value}</output>}</InspectorRow>;
}

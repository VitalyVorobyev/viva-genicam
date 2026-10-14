import type { ReactNode } from "react";
import type { StreamTransport } from "../../device/acquisitionStatus";
import type { StreamCounters } from "../../device/types";
import { InspectorRow } from "../controls/InspectorRow";
import { PREVIEW_LIMIT, formatElapsed } from "./viewerUtils";

export interface StatisticsSectionProps {
  isConnected: boolean;
  /** Whether acquisition is running. */
  active: boolean;
  /** Frames a second arriving from the camera, over the last second. */
  fps: number | null;
  /** The stream's counters: this acquisition's, or the last one's after Stop. */
  stats: StreamCounters | null;
  /** Which counters the stream fills. */
  transport: StreamTransport;
  /** Frames the viewer drew in this acquisition, and how many a second. */
  displayed: { frames: number; fps: number };
}

const DISPLAYED_HINT = `${PREVIEW_LIMIT}; every received frame is counted above`;

/**
 * The stream's counters, as the library keeps them: what arrived from the
 * camera, what was lost on the way and why, and — apart from those — what
 * the viewer drew, which is rate-limited. GigE Vision shows its packets and
 * resends, USB3 Vision its transfers. The last values stay after Stop.
 */
export function StatisticsSection({
  isConnected,
  active,
  fps,
  stats,
  transport,
  displayed,
}: StatisticsSectionProps) {
  if (!isConnected || !stats) {
    return (
      <p className="sidebar-placeholder">
        {isConnected && active ? "Counting…" : "Start acquisition to see statistics."}
      </p>
    );
  }
  return (
    <div className="feature-group iv-stats">
      {!active && <p className="iv-stats__caption">Stopped</p>}
      <StatGroup title="Frames">
        <Stat
          label="Received"
          value={count(stats.frames_complete)}
          hint="Frames handed to the application with every packet present"
        />
        <Stat
          label="Incomplete"
          value={count(stats.frames_incomplete)}
          warn={stats.frames_incomplete > 0}
          hint="Frames closed with packets missing, no trailer, or an error status from the device"
        />
        <Stat
          label="Missed"
          value={count(stats.frames_missed)}
          warn={stats.frames_missed > 0}
          hint="Blocks never seen at all: gaps in the block-id sequence"
        />
        <Stat
          label="Dropped by host"
          value={count(stats.frames_backpressure)}
          warn={stats.frames_backpressure > 0}
          hint="Frames assembled and then discarded because the application did not take them before the delivery queue filled"
        />
        {stats.frames_unsupported > 0 && (
          <Stat
            label="Unsupported"
            value={count(stats.frames_unsupported)}
            hint="Blocks whose payload type the engine does not assemble"
          />
        )}
        <Stat label="Displayed" value={count(displayed.frames)} hint={`Frames the viewer drew. ${DISPLAYED_HINT}`} />
      </StatGroup>
      <StatGroup title="Rates">
        <Stat
          label="Frame rate"
          value={fps == null ? "—" : `${fps.toFixed(1)} fps`}
          hint="Frames a second received from the camera, over the last second"
        />
        <Stat
          label="Display rate"
          value={active ? `${displayed.fps.toFixed(1)} fps` : "—"}
          hint={`Frames a second the viewer drew. ${DISPLAYED_HINT}`}
        />
        <Stat
          label="Throughput"
          value={`${stats.avg_mbps.toFixed(1)} Mbps`}
          hint="Smoothed payload throughput of complete frames, in megabits a second"
        />
        {stats.avg_latency_ms != null && (
          <Stat
            label="Latency"
            value={`${stats.avg_latency_ms.toFixed(2)} ms`}
            hint="Smoothed delay from the device timestamp to delivery"
          />
        )}
        <Stat label="Elapsed" value={formatElapsed(stats.elapsed_ms)} hint="Time since the stream started" />
      </StatGroup>
      {transport === "gige" ? (
        <>
          <StatGroup title="Packets">
            <Stat label="Received" value={count(stats.packets_received)} hint="Datagrams read from the socket" />
            <Stat
              label="Missing"
              value={count(stats.packets_missing)}
              warn={stats.packets_missing > 0}
              hint="Packets never received, of frames closed incomplete. A lower bound when a trailer is missing"
            />
            <Stat label="Duplicate" value={count(stats.packets_duplicate)} hint="Packets received more than once" />
            <Stat
              label="Ignored"
              value={count(stats.packets_ignored)}
              hint="Packets received and not used: not GVSP, a format the engine does not assemble, beyond the end of the frame, late, an error status, or from an address other than the camera's"
            />
          </StatGroup>
          <StatGroup title="Resend">
            <Stat
              label="Requested"
              value={count(stats.resend_requested)}
              hint="Packets asked for again, each counted once however often it was asked"
            />
            <Stat label="Recovered" value={count(stats.resend_recovered)} hint="Packets asked for again that then arrived" />
            <Stat
              label="Unavailable"
              value={count(stats.resend_unavailable)}
              warn={stats.resend_unavailable > 0}
              hint="Packets asked for again that the camera answered it no longer has"
            />
          </StatGroup>
        </>
      ) : (
        <StatGroup title="USB">
          <Stat label="Transfers" value={count(stats.packets_received)} hint="Bulk transfers read from the camera" />
          <Stat
            label="Transfer errors"
            value={count(stats.transfer_errors)}
            warn={stats.transfer_errors > 0}
            hint="Transfers that failed — a stall, a fault — and that the stream recovered from by resetting its endpoint, at the cost of the block in progress"
          />
        </StatGroup>
      )}
    </div>
  );
}

function StatGroup({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div className="iv-stats__group" role="group" aria-label={title}>
      <span className="iv-stats__heading" aria-hidden="true">
        {title}
      </span>
      {children}
    </div>
  );
}

/** One read-only counter: the label left, the value right in the control column. */
function Stat({ label, value, hint, warn = false }: { label: string; value: string; hint: string; warn?: boolean }) {
  return (
    <InspectorRow label={label} blocked={null} hint={hint}>
      {({ id }) => (
        <output id={id} className={`iv-stat${warn ? " iv-stat--warn" : ""}`}>
          {value}
        </output>
      )}
    </InspectorRow>
  );
}

const COUNT = new Intl.NumberFormat("en-US");

function count(n: number): string {
  return COUNT.format(n);
}

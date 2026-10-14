import type { AcquisitionStatus, DeviceInfo, StreamCounters } from "./types";

/**
 * Frames the stream did not deliver complete: closed incomplete, never seen,
 * or dropped by the host because the app fell behind. The library's
 * `StreamStats::frames_lost`.
 */
export function framesLost(stats: StreamCounters): number {
  return stats.frames_incomplete + stats.frames_missed + stats.frames_backpressure;
}

/**
 * The header's line for a running acquisition: the rate at which frames
 * arrive from the camera, and the frames lost between camera and app once
 * there are any (backlog ST-36), with the loss broken down in the tooltip.
 * Both come from the library's stream statistics, in either mode.
 */
export function describeAcquisition(status: AcquisitionStatus): {
  text: string;
  dropping: boolean;
  title: string | undefined;
} {
  if (status.fps == null) return { text: "Acquiring", dropping: false, title: undefined };
  const rate = `${status.fps.toFixed(1)} fps`;
  const stats = status.stats;
  if (!stats) return { text: rate, dropping: false, title: RECEIVED };
  const title =
    `${RECEIVED}. Lost: incomplete ${stats.frames_incomplete}, ` +
    `missed ${stats.frames_missed}, dropped by host ${stats.frames_backpressure}`;
  const lost = framesLost(stats);
  if (lost === 0) return { text: rate, dropping: false, title };
  return { text: `${rate} · ${lost} lost`, dropping: true, title };
}

const RECEIVED = "Frames received from the camera";

/** What a client knows of an acquisition: the last status, and the last counters. */
export interface AcquisitionState {
  status: AcquisitionStatus;
  /**
   * The counters of the current acquisition, or of the last one once it has
   * stopped; `null` before the first count.
   */
  stats: StreamCounters | null;
}

export const NO_ACQUISITION: AcquisitionState = {
  status: { active: false, fps: null, stats: null },
  stats: null,
};

/**
 * Fold the next status into the state. A status without counters keeps the
 * last ones — the backend's "acquisition off" follows the stopped stream's
 * final count, which stays readable — except the one that starts an
 * acquisition, which begins a new count.
 */
export function followAcquisition(
  prev: AcquisitionState,
  next: AcquisitionStatus,
): AcquisitionState {
  if (next.stats) return { status: next, stats: next.stats };
  if (next.active && !prev.status.active) return { status: next, stats: null };
  return { status: next, stats: prev.stats };
}

/** Which counters a stream fills: GVSP's packets and resends, or USB's transfers. */
export type StreamTransport = "gige" | "u3v";

/**
 * The transport of the connected device, from its entry in the device list.
 * A remote service's devices are all `"zenoh"`; the USB3 Vision service's ids
 * start `cam-u3v-` (`viva_zenoh_api::u3v_device_id`), and embedded mode's
 * `u3v-`, which also covers a device the list no longer shows.
 */
export function streamTransport(device: DeviceInfo | undefined, deviceId: string): StreamTransport {
  if (device?.transport === "usb3") return "u3v";
  if (device?.transport === "gige") return "gige";
  return deviceId.startsWith("cam-u3v-") || deviceId.startsWith("u3v-") ? "u3v" : "gige";
}

import type { AcquisitionStatus } from "./types";

/**
 * The header's line for a running acquisition: the camera's frame rate, and
 * the frames lost between camera and app once there are any (backlog ST-36).
 * Both come from the library's stream statistics, in either mode.
 */
export function describeAcquisition(status: AcquisitionStatus): {
  text: string;
  dropping: boolean;
} {
  if (status.fps == null) return { text: "Acquiring", dropping: false };
  const rate = `${status.fps.toFixed(1)} fps`;
  if (status.dropped === 0) return { text: rate, dropping: false };
  return { text: `${rate} · ${status.dropped} dropped`, dropping: true };
}

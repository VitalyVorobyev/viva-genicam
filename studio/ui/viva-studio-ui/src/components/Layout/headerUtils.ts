import type { ConnectionState } from "../../device/types";

/**
 * Truncates a device name to `maxLen` characters.
 * If the name exceeds `maxLen`, it is trimmed to `maxLen` chars and an
 * ellipsis character (U+2026) is appended, keeping the total display length
 * visually compact.
 */
export function truncateDeviceName(name: string, maxLen = 22): string {
  if (name.length <= maxLen) {
    return name;
  }
  return name.slice(0, maxLen) + "\u2026";
}

export type DeviceChipState = "connected" | "connecting" | "disconnected";

export interface DeviceChipData {
  label: string;
  state: DeviceChipState;
}

/**
 * Maps a `ConnectionState` to the display data needed by the device-status
 * chip in the app header.
 */
export function formatDeviceChip(cs: ConnectionState): DeviceChipData {
  switch (cs.kind) {
    case "connected":
      return {
        label: truncateDeviceName(cs.device_name),
        state: "connected",
      };
    case "control_lost":
      return { label: `${truncateDeviceName(cs.device_name)} (read-only)`, state: "disconnected" };
    case "connecting":
      return { label: "Connecting\u2026", state: "connecting" };
    case "reconnecting":
      // Wording and state borrowed from DeviceDropdown, which renders the live
      // header affordance: it shows the attempt count and reuses the
      // "connecting" dot rather than giving reconnection a colour of its own.
      return {
        label: `Reconnecting (${cs.attempt}/${cs.max_attempts})\u2026`,
        state: "connecting",
      };
    case "disconnected":
      return { label: "No device", state: "disconnected" };
    case "error":
      return { label: "No device", state: "disconnected" };
  }
}

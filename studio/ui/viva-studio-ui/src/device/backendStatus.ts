import type { BackendStatus } from "../generated/ipc";

// The `backend_status` payload is generated from the Rust type (ST-28); a
// `zenoh_config_error` is set when ZENOH_CONFIG was given but did not load.
export type { BackendMode, BackendStatus } from "../generated/ipc";

export interface BackendChipData {
  label: string;
  state: "embedded" | "remote" | "degraded";
  /** Tooltip text; explains what the mode means for device discovery. */
  title: string;
}

const REMOTE_TITLE =
  "Remote mode: cameras come from viva-service over Zenoh (ZENOH_CONFIG is set).";

const EMBEDDED_TITLE =
  "Embedded mode: the app talks to cameras directly. Loopback is not scanned, " +
  "so a fake camera on 127.0.0.1 will not appear. Set ZENOH_CONFIG to use a " +
  "Zenoh service instead.";

/**
 * Maps the backend status to the display data for the header chip.
 *
 * A `zenoh_config_error` means remote mode was asked for and not entered: the
 * mode is still Embedded, but the chip has to read as a fault rather than as a
 * deliberate choice, because the user did choose otherwise.
 */
export function formatBackendChip(status: BackendStatus): BackendChipData {
  if (status.zenoh_config_error) {
    return {
      label: "Embedded",
      state: "degraded",
      title: status.zenoh_config_error,
    };
  }

  return status.mode === "Remote"
    ? { label: "Remote", state: "remote", title: REMOTE_TITLE }
    : { label: "Embedded", state: "embedded", title: EMBEDDED_TITLE };
}

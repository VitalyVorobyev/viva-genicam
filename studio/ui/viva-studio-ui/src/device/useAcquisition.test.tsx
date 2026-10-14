import { emit } from "@tauri-apps/api/event";
import { mockIPC } from "@tauri-apps/api/mocks";
import { act, renderHook, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { counters } from "../test/streamCounters";
import type { AcquisitionStatus } from "./types";
import { useAcquisition } from "./useAcquisition";

function status(active: boolean, frames: number | null): AcquisitionStatus {
  return {
    active,
    fps: active ? 30 : null,
    stats: frames === null ? null : counters({ frames_complete: frames }),
  };
}

describe("useAcquisition", () => {
  it("keeps the counters after Stop, and clears them on the next start", async () => {
    mockIPC(() => undefined, { shouldMockEvents: true });
    const { result } = renderHook(() => useAcquisition());
    // The listener is registered asynchronously.
    await waitFor(async () => {
      await act(() => emit("acquisition-status", status(true, 30)));
      expect(result.current.stats?.frames_complete).toBe(30);
    });

    // Stop: the stopped stream's final count, then "acquisition off".
    await act(() => emit("acquisition-status", status(false, 900)));
    await act(() => emit("acquisition-status", status(false, null)));
    expect(result.current.status.active).toBe(false);
    expect(result.current.stats?.frames_complete).toBe(900);

    // Start.
    await act(() => emit("acquisition-status", status(true, null)));
    expect(result.current.status.active).toBe(true);
    expect(result.current.stats).toBeNull();
  });
});

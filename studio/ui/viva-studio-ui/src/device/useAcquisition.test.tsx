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
  it("treats a rejected old Start as cancellation after Stop", async () => {
    let reject!: (error: unknown) => void;
    const opening = new Promise<never>((_, fail) => { reject = fail; });
    mockIPC((cmd) => cmd === "start_acquisition" ? opening : undefined, { shouldMockEvents: true });
    const { result } = renderHook(useAcquisition);
    await act(async () => {});
    let old!: Promise<unknown>;
    act(() => { old = result.current.start().catch((error: DOMException) => error.name); });
    await act(async () => result.current.stop());
    await act(async () => reject("Connection superseded"));
    expect(await old).toBe("AbortError");
    expect(result.current.streamerInfo).toBeNull();
  });
  it("discards an acquisition start that completes after connection loss", async () => {
    let finish!: (info: { ws_url: string; width: number; height: number }) => void;
    const pending = new Promise<{ ws_url: string; width: number; height: number }>((resolve) => { finish = resolve; });
    mockIPC((cmd) => cmd === "start_acquisition" ? pending : undefined, { shouldMockEvents: true });
    const { result } = renderHook(useAcquisition);
    await act(async () => {});
    let starting!: Promise<unknown>;
    act(() => { starting = result.current.start().catch((error: DOMException) => error.name); });
    await act(() => emit("connection-state-changed", { kind: "reconnecting", device_id: "camera", attempt: 0, max_attempts: 1, reason: "gone" }));
    await act(async () => finish({ ws_url: "ws://old", width: 1, height: 1 }));
    expect(await starting).toBe("AbortError");
    expect(result.current.streamerInfo).toBeNull();
    expect(result.current.status.active).toBe(false);
  });

  it.each(["reconnecting", "control_lost"])("closes the old preview on %s and never starts acquisition on recovery", async (kind) => {
    const starts: unknown[] = [];
    mockIPC((cmd) => {
      if (cmd === "start_acquisition") { starts.push(cmd); return { ws_url: "ws://old", width: 1, height: 1 }; }
      return undefined;
    }, { shouldMockEvents: true });
    const { result } = renderHook(useAcquisition);
    await act(async () => {});
    await act(async () => { await result.current.start(); });
    await act(() => emit("acquisition-status", status(true, 30)));
    await act(() => emit("connection-state-changed", { kind, device_id: "camera", reason: "gone" }));
    expect(result.current.streamerInfo).toBeNull();
    expect(result.current.status.active).toBe(false);
    await act(() => emit("connection-state-changed", { kind: "connected", device_id: "camera", device_name: "camera", model: "new" }));
    expect(result.current.streamerInfo).toBeNull();
    expect(starts).toHaveLength(1);
  });

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

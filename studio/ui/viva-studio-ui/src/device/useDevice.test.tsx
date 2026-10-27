import { emit } from "@tauri-apps/api/event";
import { mockIPC } from "@tauri-apps/api/mocks";
import { act, renderHook, waitFor } from "@testing-library/react";
import { expect, it } from "vitest";
import { makeModel } from "../test/model";
import { useDevice } from "./useDevice";

function deferred<T>() { let resolve!: (value: T) => void; let reject!: (reason: unknown) => void; const promise = new Promise<T>((r, j) => { resolve = r; reject = j; }); return { promise, resolve, reject }; }
const connected = (id: string) => ({ kind: "connected", device_id: id, device_name: id, model: "bench" });

it("treats a stale backend rejection as cancellation after an explicit disconnect", async () => {
  const opening = deferred<ReturnType<typeof makeModel>>();
  mockIPC((cmd) => {
    if (cmd === "list_discovered_devices") return [];
    if (cmd === "get_connection_state") return { kind: "disconnected" };
    if (cmd === "connect_device") return opening.promise;
    return undefined;
  }, { shouldMockEvents: true });
  const { result } = renderHook(useDevice);
  await act(async () => {});
  let old!: Promise<unknown>;
  act(() => { old = result.current.connect("old").catch((error: DOMException) => error.name); });
  await act(async () => result.current.disconnect());
  await act(async () => opening.reject("Connection superseded"));
  expect(await old).toBe("AbortError");
});

it("ignores a reconnect payload for an old device or a user disconnect in progress", async () => {
  const closing = deferred<void>();
  mockIPC((cmd) => {
    if (cmd === "list_discovered_devices") return [{ id: "new" }];
    if (cmd === "get_connection_state") return connected("new");
    if (cmd === "disconnect_device") return closing.promise;
    return undefined;
  }, { shouldMockEvents: true });
  const { result } = renderHook(useDevice);
  await waitFor(() => expect(result.current.connectionState.kind).toBe("connected"));
  await act(async () => {});
  const model = makeModel({ Gain: { kind: "Float" } });
  await act(async () => emit("device-reconnected", { device_id: "old", node_count: 2, model }));
  expect(result.current.replacementModel).toBeNull();
  let done!: Promise<void>;
  act(() => { done = result.current.disconnect(); });
  await act(async () => emit("device-reconnected", { device_id: "new", node_count: 2, model }));
  expect(result.current.replacementModel).toBeNull();
  await act(async () => { closing.resolve(); await done; });
  await act(async () => emit("device-reconnected", { device_id: "new", node_count: 2, model }));
  expect(result.current.replacementModel).toBeNull();
});

it("rejects a delayed old connect response after a new device connects", async () => {
  const old = deferred<ReturnType<typeof makeModel>>();
  const fresh = makeModel({ ExposureTime: { kind: "Float" } });
  mockIPC((cmd, raw) => {
    if (cmd === "list_discovered_devices") return [];
    if (cmd === "get_connection_state") return { kind: "disconnected" };
    if (cmd === "connect_device") return (raw as { deviceId: string }).deviceId === "old" ? old.promise : fresh;
    return undefined;
  }, { shouldMockEvents: true });
  const { result } = renderHook(useDevice);
  await act(async () => {});
  let previous!: Promise<unknown>;
  act(() => { previous = result.current.connect("old").catch((error: DOMException) => error.name); });
  let received: unknown;
  await act(async () => { received = await result.current.connect("new"); });
  expect(received).toEqual(fresh);
  await act(async () => { old.resolve(makeModel({ ObsoleteGain: {} })); });
  expect(await previous).toBe("AbortError");
});

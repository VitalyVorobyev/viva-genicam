import { afterEach, expect, it, vi } from "vitest";
import { followPreview } from "./previewStream";

class Socket {
  static instances: Socket[] = [];
  binaryType = "";
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onerror: (() => void) | null = null;
  onclose: (() => void) | null = null;
  close = vi.fn();
  constructor() { Socket.instances.push(this); }
}

const cleanups: Array<() => void> = [];
afterEach(() => {
  cleanups.splice(0).forEach(stop => stop());
  Socket.instances = [];
  vi.unstubAllGlobals();
});
const listener = () => ({ message: vi.fn(), close: vi.fn(), error: vi.fn() });

it("shares one socket and replays only the latest encoded frame to a late viewer", () => {
  vi.stubGlobal("WebSocket", Socket);
  const first = listener(), second = listener();
  const stopFirst = followPreview("ws://preview", first, "camera-a");
  cleanups.push(stopFirst);
  const socket = Socket.instances[0];
  const info = '{"type":"info","width":2,"height":1}';
  socket.onmessage?.({ data: info });
  socket.onmessage?.({ data: new ArrayBuffer(3) });
  const latest = new ArrayBuffer(8);
  socket.onmessage?.({ data: latest });
  const stopSecond = followPreview("ws://preview", second, "camera-a");
  cleanups.push(stopSecond);
  expect(Socket.instances).toHaveLength(1);
  expect(second.message.mock.calls).toEqual([[info], [latest]]);
  stopFirst();
  expect(socket.close).not.toHaveBeenCalled();
  socket.onerror?.();
  expect(second.error).toHaveBeenCalledOnce();
  expect(first.error).not.toHaveBeenCalled();
  stopSecond();
  expect(socket.close).toHaveBeenCalledOnce();
});

it("isolates device identities even when a preview endpoint is reused", () => {
  vi.stubGlobal("WebSocket", Socket);
  cleanups.push(followPreview("ws://preview", listener(), "camera-a"));
  Socket.instances[0].onmessage?.({ data: new ArrayBuffer(8) });
  const next = listener();
  cleanups.push(followPreview("ws://preview", next, "camera-b"));
  expect(Socket.instances).toHaveLength(2);
  expect(next.message).not.toHaveBeenCalled();
});

it("clears cached bytes after closure and ignores unexpected binary message types", () => {
  vi.stubGlobal("WebSocket", Socket);
  const first = listener();
  const stopFirst = followPreview("ws://preview", first);
  cleanups.push(stopFirst);
  const socket = Socket.instances[0];
  socket.onmessage?.({ data: new ArrayBuffer(8) });
  socket.onmessage?.({ data: { bad: true } });
  expect(first.message).toHaveBeenCalledOnce();
  socket.onclose?.();
  expect(first.close).toHaveBeenCalledOnce();
  const next = listener();
  cleanups.push(followPreview("ws://preview", next));
  expect(next.message).not.toHaveBeenCalled();
  expect(Socket.instances).toHaveLength(2);
  stopFirst(); // Retiring an old subscriber must not remove its replacement.
  cleanups.push(followPreview("ws://preview", listener()));
  expect(Socket.instances).toHaveLength(2);
});

import { act, fireEvent, render } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { ViewerCanvas } from "./ViewerCanvas";
import type { NativeFrame } from "./nativeFrame";

class Socket {
  static latest: Socket;
  binaryType = "";
  onmessage: ((event: { data: ArrayBuffer }) => void) | null = null;
  close() {}
  constructor() { Socket.latest = this; }
}

afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); });

it("freezes the matching native preview and refuses a late live decode until Resume", async () => {
  vi.stubGlobal("WebSocket", Socket);
  const drawImage = vi.fn();
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({ drawImage, clearRect: vi.fn() } as unknown as CanvasRenderingContext2D);
  const pending: Array<(value: ImageBitmap) => void> = [];
  vi.stubGlobal("createImageBitmap", vi.fn(() => new Promise<ImageBitmap>(resolve => pending.push(resolve))));
  const bitmap = (id: string) => ({ width: 2, height: 1, close: vi.fn(), id }) as unknown as ImageBitmap;
  const props = { wsUrl: "ws://127.0.0.1:3001/ws", onFrameStats: vi.fn() };
  const { rerender } = render(<ViewerCanvas {...props} />);
  act(() => { Socket.latest.onmessage?.({ data: new ArrayBuffer(8) }); });
  expect(pending).toHaveLength(1);
  const frame: NativeFrame = { width: 2, height: 1, bits: 12, channels: 1, mosaic: false, pixelFormat: "Mono12", samples: new Uint16Array([1, 4095]), bmp: new Uint8Array([66, 77]), png: new Uint8Array() };
  rerender(<ViewerCanvas {...props} nativeFrame={frame} />);
  expect(pending).toHaveLength(2);
  await act(async () => { pending[0](bitmap("late live")); pending[1](bitmap("frozen")); });
  expect(drawImage).toHaveBeenCalledOnce();
  expect((drawImage.mock.calls[0][0] as unknown as { id: string }).id).toBe("frozen");
  act(() => { Socket.latest.onmessage?.({ data: new ArrayBuffer(8) }); });
  expect(pending).toHaveLength(2);
  rerender(<ViewerCanvas {...props} nativeFrame={null} />);
  act(() => { Socket.latest.onmessage?.({ data: new ArrayBuffer(8) }); });
  await act(async () => { pending[2](bitmap("resumed")); });
  expect(drawImage).toHaveBeenCalledTimes(2);
  expect((drawImage.mock.calls[1][0] as unknown as { id: string }).id).toBe("resumed");
});

it("does not expose native sample values until their captured BMP has painted", async () => {
  vi.stubGlobal("WebSocket", Socket);
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => { callback(0); return 1; });
  vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockReturnValue(100);
  vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(50);
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({ drawImage: vi.fn(), clearRect: vi.fn() } as unknown as CanvasRenderingContext2D);
  const pending: Array<(value: ImageBitmap) => void> = [];
  vi.stubGlobal("createImageBitmap", vi.fn(() => new Promise<ImageBitmap>(resolve => pending.push(resolve))));
  const bitmap = () => ({ width: 2, height: 1, close: vi.fn() }) as unknown as ImageBitmap;
  const onPixelHover = vi.fn();
  const onNativePreviewReady = vi.fn();
  const props = { wsUrl: "ws://127.0.0.1:3001/ws", onFrameStats: vi.fn(), onPixelHover, onNativePreviewReady };
  const { rerender, container } = render(<ViewerCanvas {...props} />);
  act(() => { Socket.latest.onmessage?.({ data: new ArrayBuffer(8) }); });
  await act(async () => { pending[0](bitmap()); });
  const frame: NativeFrame = { width: 2, height: 1, bits: 12, channels: 1, mosaic: false, pixelFormat: "Mono12", samples: new Uint16Array([4095, 7]), bmp: new Uint8Array([66, 77]), png: new Uint8Array() };
  rerender(<ViewerCanvas {...props} nativeFrame={frame} />);
  const canvasWrap = container.querySelector(".iv-canvas-wrap")!;
  fireEvent.mouseMove(canvasWrap, { clientX: 49, clientY: 25 });
  expect(onPixelHover.mock.calls[onPixelHover.mock.calls.length - 1]?.[0]?.sample).toBeNull();
  expect(onNativePreviewReady).not.toHaveBeenCalled();
  await act(async () => { pending[1](bitmap()); });
  fireEvent.mouseMove(canvasWrap, { clientX: 49, clientY: 25 });
  expect(onPixelHover.mock.calls[onPixelHover.mock.calls.length - 1]?.[0]?.sample).toEqual({ kind: "mono", value: 4095 });
  expect(onNativePreviewReady).toHaveBeenCalledWith(frame, undefined);
});


it("publishes metadata only after its matching bitmap paints and rejects stale session decodes", async () => {
  vi.stubGlobal("WebSocket", Socket);
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({ drawImage: vi.fn(), clearRect: vi.fn() } as unknown as CanvasRenderingContext2D);
  const pending: Array<(value: ImageBitmap) => void> = [];
  vi.stubGlobal("createImageBitmap", vi.fn(() => new Promise<ImageBitmap>(resolve => pending.push(resolve))));
  const metadata = (block_id: string) => ({ block_id, ts_dev: null, ts_host_ns: null, complete: true,
    chunks: [], features: [], chunk_error: null, truncated: false });
  const packet = (id: string) => {
    const json = new TextEncoder().encode(JSON.stringify(metadata(id)));
    const bytes = new Uint8Array(10 + json.length);
    bytes.set([86, 70, 82, 77]); new DataView(bytes.buffer).setUint32(4, json.length, true);
    bytes.set(json, 8); bytes.set([66, 77], 8 + json.length); return bytes.buffer;
  };
  const bitmap = () => ({ width: 2, height: 1, close: vi.fn() }) as unknown as ImageBitmap;
  const onFrameMetadata = vi.fn();
  const props = { wsUrl: "ws://127.0.0.1:3001/ws", onFrameStats: vi.fn(), onFrameMetadata };
  const { rerender } = render(<ViewerCanvas {...props} streamKey="a" />);
  act(() => { Socket.latest.onmessage?.({ data: packet("1") }); Socket.latest.onmessage?.({ data: packet("2") }); });
  expect(onFrameMetadata).toHaveBeenLastCalledWith(null);
  await act(async () => { pending[1](bitmap()); pending[0](bitmap()); });
  expect(onFrameMetadata).toHaveBeenLastCalledWith(metadata("2"));
  expect(onFrameMetadata.mock.calls.some(([value]) => value?.block_id === "1")).toBe(false);
  act(() => { Socket.latest.onmessage?.({ data: packet("3") }); });
  rerender(<ViewerCanvas {...props} streamKey="b" />);
  await act(async () => { pending[2](bitmap()); });
  expect(onFrameMetadata).toHaveBeenLastCalledWith(null);
  expect(onFrameMetadata.mock.calls.some(([value]) => value?.block_id === "3")).toBe(false);
});

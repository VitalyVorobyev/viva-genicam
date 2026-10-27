import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { ConfigurationPreview } from "./ConfigurationPreview";
import { ViewerCanvas } from "../ImageViewer/ViewerCanvas";

class Socket {
  static instances: Socket[] = [];
  binaryType = "";
  onmessage: ((event: { data: ArrayBuffer }) => void) | null = null;
  close = vi.fn();
  constructor() { Socket.instances.push(this); }
}

const streamerInfo = { ws_url: "ws://127.0.0.1:3333/ws", port: 3333, width: 2, height: 1 };
const props = { streamerInfo, deviceId: "camera-a", isConnected: true, isAcquiring: true, isActive: true };
afterEach(() => {
  localStorage.clear();
  Socket.instances = [];
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

it("starts collapsed; opening never starts acquisition and closing releases its consumer", () => {
  vi.stubGlobal("WebSocket", Socket);
  const onExpandedChange = vi.fn();
  render(<ConfigurationPreview {...props} onExpandedChange={onExpandedChange} />);
  expect(onExpandedChange.mock.calls).toEqual([[false]]);
  const toggle = screen.getByRole("button", { name: /image preview/i });
  expect(toggle.getAttribute("aria-expanded")).toBe("false");
  expect(Socket.instances).toHaveLength(0);
  fireEvent.click(toggle);
  expect(toggle.getAttribute("aria-expanded")).toBe("true");
  expect(Socket.instances).toHaveLength(1);
  expect(onExpandedChange.mock.calls).toEqual([[false], [true]]);
  fireEvent.click(toggle);
  expect(Socket.instances[0].close).toHaveBeenCalledOnce();
  expect(localStorage.getItem("configuration-preview-expanded")).toBe("false");
});

it("needs no connection while offline or hidden and clears a switched device's canvas", () => {
  vi.stubGlobal("WebSocket", Socket);
  localStorage.setItem("configuration-preview-expanded", "true");
  const { rerender, container } = render(<ConfigurationPreview {...props} isConnected={false} />);
  expect(screen.getByText("Connect a camera to preview its image.")).toBeTruthy();
  expect(Socket.instances).toHaveLength(0);
  rerender(<ConfigurationPreview {...props} />);
  const originalCanvas = container.querySelector("canvas");
  rerender(<ConfigurationPreview {...props} deviceId="camera-b" />);
  expect(Socket.instances[0].close).toHaveBeenCalledOnce();
  expect(container.querySelector("canvas")).not.toBe(originalCanvas);
  rerender(<ConfigurationPreview {...props} isActive={false} />);
  expect(Socket.instances[1].close).toHaveBeenCalledOnce();
  expect(container.querySelector("canvas")).toBeNull();
});

it("shares the Image viewer socket and only the active preview decodes", async () => {
  vi.stubGlobal("WebSocket", Socket);
  vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockReturnValue(100);
  vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(50);
  const drawImage = vi.fn();
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({ drawImage, clearRect: vi.fn() } as unknown as CanvasRenderingContext2D);
  const decode = vi.fn(async () => ({ width: 2, height: 1, close: vi.fn() }) as unknown as ImageBitmap);
  vi.stubGlobal("createImageBitmap", decode);
  localStorage.setItem("configuration-preview-expanded", "true");
  const { rerender } = render(<>
    <ViewerCanvas wsUrl={streamerInfo.ws_url} streamKey="camera-a" onFrameStats={vi.fn()} active={false} />
    <ConfigurationPreview {...props} />
  </>);
  expect(Socket.instances).toHaveLength(1);
  await act(async () => { Socket.instances[0].onmessage?.({ data: new ArrayBuffer(8) }); });
  expect(decode).toHaveBeenCalledOnce();
  expect(drawImage).toHaveBeenCalledOnce();
  rerender(<>
    <ViewerCanvas wsUrl={streamerInfo.ws_url} streamKey="camera-a" onFrameStats={vi.fn()} active />
    <ConfigurationPreview {...props} isActive={false} />
  </>);
  await act(async () => {});
  expect(decode).toHaveBeenCalledTimes(2); // The hidden Image tab's cached frame.
  await act(async () => { Socket.instances[0].onmessage?.({ data: new ArrayBuffer(8) }); });
  expect(Socket.instances).toHaveLength(1);
  expect(decode).toHaveBeenCalledTimes(3);
});


it("does not decode a frozen Image canvas while it is hidden", async () => {
  vi.stubGlobal("WebSocket", Socket);
  const drawImage = vi.fn();
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({ drawImage, clearRect: vi.fn() } as unknown as CanvasRenderingContext2D);
  const decode = vi.fn(async () => ({ width: 2, height: 1, close: vi.fn() }) as unknown as ImageBitmap);
  vi.stubGlobal("createImageBitmap", decode);
  const nativeFrame = { width: 2, height: 1, bits: 8, channels: 1 as const, mosaic: false, pixelFormat: "Mono8", samples: new Uint16Array([0, 255]), bmp: new Uint8Array([66, 77]), png: new Uint8Array() };
  const { rerender } = render(<ViewerCanvas wsUrl={streamerInfo.ws_url} onFrameStats={vi.fn()} active={false} nativeFrame={nativeFrame} />);
  expect(decode).not.toHaveBeenCalled();
  rerender(<ViewerCanvas wsUrl={streamerInfo.ws_url} onFrameStats={vi.fn()} active nativeFrame={nativeFrame} />);
  await act(async () => {});
  expect(decode).toHaveBeenCalledOnce();
  expect(drawImage).toHaveBeenCalledOnce();
});

it("remains usable when the browser cannot persist the fold choice", () => {
  vi.stubGlobal("WebSocket", Socket);
  vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => { throw new Error("storage blocked"); });
  vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("storage blocked"); });
  render(<ConfigurationPreview {...props} />);
  const toggle = screen.getByRole("button", { name: /image preview/i });
  expect(toggle.getAttribute("aria-expanded")).toBe("false");
  fireEvent.click(toggle);
  expect(toggle.getAttribute("aria-expanded")).toBe("true");
  expect(Socket.instances).toHaveLength(1);
});

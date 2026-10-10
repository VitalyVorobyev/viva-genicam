import { cleanup } from "@testing-library/react";
import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach } from "vitest";

afterEach(() => {
  cleanup();
  clearMocks();
  // clearMocks() empties the IPC mock but leaves the object behind, and
  // isTauri() only checks that it exists.
  delete window.__TAURI_INTERNALS__;
});

// jsdom has no layout engine, so it ships no ResizeObserver.
class NoopResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}
globalThis.ResizeObserver ??= NoopResizeObserver;

import { mockIPC } from "@tauri-apps/api/mocks";
import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { ToastProvider } from "../../context/ToastContext";
import { FeatureStoreProvider } from "../../device/FeatureStoreContext";
import type { ConfigurationLoaded, ConfigurationSaveReport } from "../../device/types";
import { ToastContainer } from "../Layout/ToastContainer";
import { ConfigurationPanel } from "./ConfigurationPanel";

const PATH = "/home/user/camera.txt";

/** The backend and the file dialogs; records the commands the panel sends. */
function backend(loaded: ConfigurationLoaded, picked: string | null = PATH) {
  const calls: Array<[string, unknown]> = [];
  mockIPC((cmd, args) => {
    if (cmd === "plugin:dialog|save" || cmd === "plugin:dialog|open") return picked;
    if (cmd === "save_camera_configuration") {
      calls.push([cmd, args]);
      return {
        path: (args as { path: string }).path,
        values: 42,
        omitted: [{ feature: "Gain[GainSelector=DigitalAll]", reason: "not available" }],
      } satisfies ConfigurationSaveReport;
    }
    if (cmd === "load_camera_configuration") {
      calls.push([cmd, args]);
      return loaded;
    }
    if (cmd === "query_feature_states_bulk") return {};
    return undefined;
  }, { shouldMockEvents: true });
  return calls;
}

function show({ readOnly = false, acquiring = false } = {}) {
  return render(
    <ToastProvider>
      <FeatureStoreProvider connected={true} readOnly={readOnly}>
        <ConfigurationPanel connected={true} acquiring={acquiring} />
      </FeatureStoreProvider>
      <ToastContainer />
    </ToastProvider>,
  );
}

/** Let the store's event listeners finish registering before the test ends. */
const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, 20)));

const CLEAN: ConfigurationLoaded = { written: 42, written_on_retry: 0, failures: [], warnings: [] };

describe("ConfigurationPanel", () => {
  it("saves to the picked file and lists what was left out", async () => {
    const calls = backend(CLEAN);
    show();
    await userEvent.setup().click(screen.getByRole("button", { name: "Save to file…" }));
    expect(await screen.findByText(`Saved 42 values to ${PATH}. 1 left out.`)).toBeTruthy();
    expect(calls).toEqual([["save_camera_configuration", { path: PATH }]]);
    expect(screen.getByText("Gain[GainSelector=DigitalAll]: not available")).toBeTruthy();
  });

  it("shows the values a load could not write, and its warnings", async () => {
    const calls = backend({
      written: 40,
      written_on_retry: 2,
      failures: ["Gain = 99: out of range"],
      warnings: ["saved from model OtherCam, this camera is FakeGigE"],
    });
    show();
    await userEvent.setup().click(screen.getByRole("button", { name: "Load from file…" }));
    expect(await screen.findByText(/Restored 40 values from/)).toBeTruthy();
    expect(calls).toEqual([["load_camera_configuration", { path: PATH }]]);
    expect(screen.getByText("Gain = 99: out of range")).toBeTruthy();
    expect(screen.getByRole("alert", { name: "Configuration warnings" }).textContent).toContain("OtherCam");
  });

  it("does nothing when the dialog is cancelled", async () => {
    const calls = backend(CLEAN, null);
    show();
    await userEvent.setup().click(screen.getByRole("button", { name: "Load from file…" }));
    await settle();
    expect(calls).toEqual([]);
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("offers neither in a read-only session", async () => {
    backend(CLEAN);
    show({ readOnly: true });
    await settle();
    expect(screen.getByRole("button", { name: "Save to file…" })).toHaveProperty("disabled", true);
    expect(screen.getByRole("button", { name: "Load from file…" })).toHaveProperty("disabled", true);
    expect(screen.getByText(/Read-only session/)).toBeTruthy();
  });

  it("warns that a save while acquiring leaves out locked features", async () => {
    backend(CLEAN);
    show({ acquiring: true });
    await settle();
    expect(screen.getByText(/The camera is acquiring/)).toBeTruthy();
  });
});

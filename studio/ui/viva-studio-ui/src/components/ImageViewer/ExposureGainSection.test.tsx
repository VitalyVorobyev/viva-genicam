import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { ToastProvider } from "../../context/ToastContext";
import type { NodeValueEntry } from "../../device/types";
import { makeModel } from "../../test/model";
import { ToastContainer } from "../Layout/ToastContainer";
import { ExposureGainSection } from "./ExposureGainSection";

const model = makeModel({
  ExposureTime: { kind: "Float", constraints: { min: 10, max: 100_000 } },
  Gain: { kind: "Float", constraints: { min: 0, max: 24 } },
});

function live(exposure: number, gain: number): Map<string, NodeValueEntry> {
  return new Map([
    ["ExposureTime", { value: exposure, access_mode: "RW" }],
    ["Gain", { value: gain, access_mode: "RW" }],
  ]);
}

function section(liveValues: Map<string, NodeValueEntry>) {
  return (
    <ToastProvider>
      <ExposureGainSection isConnected={true} externalModel={model} liveValues={liveValues} />
      <ToastContainer />
    </ToastProvider>
  );
}

describe("ExposureGainSection", () => {
  it("tells the user when the camera refuses a write", async () => {
    mockIPC((cmd) => {
      if (cmd === "write_node") throw new Error("node is locked: ExposureTime");
      return undefined;
    });
    render(section(live(1000, 0)));
    const [exposure] = screen.getAllByRole("spinbutton");
    await userEvent.clear(exposure!);
    await userEvent.type(exposure!, "2000");
    await userEvent.tab();

    expect(await screen.findByText(/node is locked: ExposureTime/)).toBeTruthy();
  });

  it("keeps what the user is typing when another feature changes", async () => {
    mockIPC(() => undefined);
    const { rerender } = render(section(live(1000, 0)));
    const [exposure] = screen.getAllByRole("spinbutton");
    await userEvent.clear(exposure!);
    await userEvent.type(exposure!, "1234");

    // A node-value-changed event for Gain replaces the whole map.
    rerender(section(live(1000, 6)));

    expect((exposure as HTMLInputElement).value).toBe("1234");
  });
});

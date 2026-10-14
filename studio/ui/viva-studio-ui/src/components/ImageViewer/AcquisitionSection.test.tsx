import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { ToastProvider } from "../../context/ToastContext";
import { FeatureStoreProvider } from "../../device/FeatureStoreContext";
import { loadPacketSize } from "../../device/packetSize";
import type { UiNode } from "../../xml_model/uigraph";
import { makeModel } from "../../test/model";
import { AcquisitionSection } from "./AcquisitionSection";

function section(deviceId: string, nodes: Record<string, UiNode> = {}) {
  return (
    <ToastProvider>
      <FeatureStoreProvider connected={false}>
        <AcquisitionSection
          isConnected={true}
          isAcquiring={false}
          nodes={nodes}
          deviceId={deviceId}
        />
      </FeatureStoreProvider>
    </ToastProvider>
  );
}

describe("AcquisitionSection, packet size (ST-22)", () => {
  it("negotiates by default, and keeps a size the user sets for this camera", async () => {
    render(section("cam-test-1"));
    const mode = screen.getByRole("combobox", { name: "Packet size" }) as HTMLSelectElement;
    expect(mode.value).toBe("auto");
    expect(loadPacketSize("cam-test-1")).toBeNull();

    await userEvent.selectOptions(mode, "fixed");
    const size = screen.getByRole("spinbutton", { name: "Packet size in bytes" });
    await userEvent.clear(size);
    await userEvent.type(size, "1400{Enter}");
    expect(loadPacketSize("cam-test-1")).toBe(1400);
    // Another camera's link is its own.
    expect(loadPacketSize("cam-test-2")).toBeNull();

    await userEvent.selectOptions(mode, "auto");
    expect(loadPacketSize("cam-test-1")).toBeNull();
  });

  it("refuses a size GigE Vision cannot carry", async () => {
    render(section("cam-test-3"));
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "Packet size" }), "fixed");
    const size = screen.getByRole("spinbutton", { name: "Packet size in bytes" });
    await userEvent.clear(size);
    await userEvent.type(size, "70000{Enter}");
    expect(loadPacketSize("cam-test-3")).toBe(1500);
  });
});

describe("AcquisitionSection, frame rate", () => {
  it("draws the frame-rate switch in the frame-rate row, before its field", () => {
    const { graph } = makeModel({
      AcquisitionFrameRate: { kind: "Float", display_name: "Acquisition Frame Rate" },
      AcquisitionFrameRateEnable: { kind: "Boolean", display_name: "Acquisition Frame Rate Enable" },
    });
    render(section("cam-test-4", graph.nodes_by_name));
    const rate = screen.getByRole("spinbutton", { name: "Acquisition Frame Rate" });
    const enable = screen.getByRole("checkbox", { name: "Acquisition Frame Rate Enable" });
    const row = rate.closest(".iv-row");
    expect(row).not.toBeNull();
    expect(row!.contains(enable)).toBe(true);
    expect(enable.compareDocumentPosition(rate) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    // No slider, and no row of its own for the switch.
    expect(screen.queryByRole("slider")).toBeNull();
    expect(document.querySelectorAll(".iv-row")).toHaveLength(2); // frame rate, packet size
  });
});

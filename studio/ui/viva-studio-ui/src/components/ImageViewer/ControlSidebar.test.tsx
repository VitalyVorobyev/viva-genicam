import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { beforeEach, describe, expect, it } from "vitest";
import { ToastProvider } from "../../context/ToastContext";
import { FeatureStoreProvider } from "../../device/FeatureStoreContext";
import type { SfncGroup } from "../../device/types";
import { ControlSidebar } from "./ControlSidebar";

const GROUPS: SfncGroup[] = [
  {
    id: "acquisition_control",
    title: "Acquisition Control",
    icon: "play",
    default_open: true,
    features: [{ node: "AcquisitionMode", widget: "enum_select" }],
  },
  {
    id: "image_format",
    title: "Image Format",
    icon: "crop",
    default_open: false,
    features: [{ node: "Width", widget: "int_input" }],
  },
];

function Sidebar({ initiallyCollapsed }: { initiallyCollapsed: boolean }) {
  const [collapsed, setCollapsed] = useState(initiallyCollapsed);
  return (
    <ToastProvider>
      <FeatureStoreProvider connected={false}>
        <ControlSidebar
          collapsed={collapsed}
          onToggle={() => setCollapsed((c) => !c)}
          isConnected={false}
          isAcquiring={false}
          externalModel={null}
          cameraModel="test-camera"
          deviceId={null}
          statistics={{
            active: false,
            fps: null,
            stats: null,
            transport: "gige",
            displayed: { frames: 0, fps: 0 },
          }}
        />
      </FeatureStoreProvider>
    </ToastProvider>
  );
}

describe("ControlSidebar", () => {
  beforeEach(() => {
    localStorage.clear();
    mockIPC((cmd) => (cmd === "get_sfnc_groups" ? GROUPS : undefined));
  });

  it("opens the panel at the section whose rail icon was clicked", async () => {
    render(<Sidebar initiallyCollapsed={true} />);
    await userEvent.click(await screen.findByRole("button", { name: "Image Format" }));

    const header = await screen.findByRole("button", { name: "Image Format", expanded: true });
    expect(header).toBeTruthy();
    // The other sections keep their state.
    expect(screen.getByRole("button", { name: "Acquisition Control" }).getAttribute("aria-expanded")).toBe(
      "true",
    );
  });

  // The stream's counters sit right after the acquisition settings, and the
  // rail reaches them like any section.
  it("places Statistics after Acquisition Control", async () => {
    render(<Sidebar initiallyCollapsed={false} />);
    await screen.findByRole("button", { name: "Image Format" });
    const order = Array.from(document.querySelectorAll(".sidebar-section__label")).map((l) => l.textContent);
    expect(order).toEqual(["Acquisition Control", "Statistics", "Frame metadata", "Image Format"]);
    expect(screen.getByRole("button", { name: "Statistics" }).getAttribute("aria-expanded")).toBe("true");
  });

  it("collapses to the rail and back with one button", async () => {
    render(<Sidebar initiallyCollapsed={false} />);
    await userEvent.click(screen.getByRole("button", { name: "Hide controls" }));
    expect(await screen.findByRole("button", { name: "Show controls" })).toBeTruthy();
    expect(screen.queryByRole("separator")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Show controls" }));
    expect(screen.getByRole("separator", { name: "Resize controls" })).toBeTruthy();
  });
});

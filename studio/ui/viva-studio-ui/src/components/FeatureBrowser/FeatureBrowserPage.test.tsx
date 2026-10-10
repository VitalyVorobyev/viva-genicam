import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it } from "vitest";
import { ToastProvider } from "../../context/ToastContext";
import type { NodeValueEntry } from "../../device/types";
import { makeModel } from "../../test/model";
import { FeatureBrowserPage } from "./FeatureBrowserPage";

const model = makeModel(
  {
    Width: { kind: "Integer", display_name: "Width", constraints: { min: 16, max: 4096 } },
    PixelFormat: {
      kind: "Enumeration",
      display_name: "Pixel Format",
      enum_entries: [{ name: "Mono8" }, { name: "Mono16" }],
    },
  },
  {
    Root: { name: "Root", display_name: "Root", features: ["ImageFormat"] },
    ImageFormat: {
      name: "ImageFormat",
      display_name: "Image Format",
      features: ["Width", "PixelFormat"],
    },
  },
);

// What the connect-time bulk read leaves behind: values, not FeatureStates.
const liveValues = new Map<string, NodeValueEntry>([
  ["Width", { value: 640, access_mode: "RW" }],
  ["PixelFormat", { value: "Mono8", access_mode: "RW" }],
]);

function renderPage() {
  return render(
    <ToastProvider>
      <FeatureBrowserPage externalModel={model} liveValues={liveValues} isConnected={true} />
    </ToastProvider>,
  );
}

beforeEach(() => {
  mockIPC((cmd) => (cmd === "get_current_model" ? null : undefined));
});

describe("FeatureBrowserPage, connected", () => {
  it("does not turn a feature the user only looked at into a pending write", async () => {
    renderPage();
    await userEvent.click(await screen.findByRole("option", { name: /Width/ }));

    expect(screen.queryByRole("button", { name: /^Apply \d+$/ })).toBeNull();
  });

  it("shows a connected enumeration's live entry without a validation error", async () => {
    renderPage();
    await userEvent.click(await screen.findByRole("option", { name: /Pixel Format/ }));

    expect((screen.getByRole("combobox") as HTMLSelectElement).value).toBe("Mono8");
    expect(screen.queryByText("Value must be a valid enumeration entry.")).toBeNull();
  });
});

import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it } from "vitest";
import { ToastProvider } from "../../context/ToastContext";
import { FeatureStoreProvider } from "../../device/FeatureStoreContext";
import type { FeatureState } from "../../device/types";
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

function live(value: number | string, kind: string): FeatureState {
  return { value, access_mode: "RW", kind, is_implemented: true, is_available: true };
}

// The camera, as the backend's IPC answers for it.
const states: Record<string, FeatureState> = {
  Width: { ...live(640, "Integer"), numeric: { min: 16, max: 4096 } },
  PixelFormat: { ...live("Mono8", "Enumeration"), enum_available: ["Mono8", "Mono16"] },
};

function answer(cmd: string, args: unknown) {
  if (cmd === "get_current_model") return null;
  if (cmd === "query_feature_states_bulk") {
    const names = (args as { names: string[] }).names;
    return Object.fromEntries(names.filter((n) => n in states).map((n) => [n, states[n]]));
  }
  return undefined;
}

function page(isActive = true) {
  return (
    <ToastProvider>
      <FeatureStoreProvider connected={true}>
        <FeatureBrowserPage externalModel={model} isConnected={true} isActive={isActive} />
      </FeatureStoreProvider>
    </ToastProvider>
  );
}

function renderPage() {
  return render(page());
}

beforeEach(() => {
  mockIPC(answer);
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

    await waitFor(() =>
      expect((screen.getByRole("combobox") as HTMLSelectElement).value).toBe("Mono8"),
    );
    expect(screen.queryByText("Value must be a valid enumeration entry.")).toBeNull();
  });
});

describe("FeatureBrowserPage, keyboard shortcuts", () => {
  // AppLayout keeps the page mounted with display:none while another tab is
  // shown, so a shortcut typed on the Image tab must not reach it.
  it("ignores Ctrl+Enter and Ctrl+F while its tab is hidden", async () => {
    const writes: unknown[] = [];
    mockIPC((cmd, args) => {
      if (cmd === "write_node") {
        writes.push(args);
        return live(800, "Integer");
      }
      return answer(cmd, args);
    });
    const { rerender } = render(page(true));
    await userEvent.click(await screen.findByRole("option", { name: /Width/ }));
    const input = screen.getByRole("spinbutton");
    await userEvent.clear(input);
    await userEvent.type(input, "800");
    input.blur();

    rerender(page(false));
    await userEvent.keyboard("{Control>}{Enter}{/Control}");
    await userEvent.keyboard("{Control>}f{/Control}");

    expect(writes).toEqual([]);
    expect(document.activeElement).not.toBe(screen.getByRole("searchbox"));
  });
});

import { mockIPC } from "@tauri-apps/api/mocks";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useEffect } from "react";
import { describe, expect, it } from "vitest";
import { ToastProvider } from "../../context/ToastContext";
import { FeatureStoreProvider, useFeatureStore } from "../../device/FeatureStoreContext";
import type { FeatureStore } from "../../device/featureStore";
import type { FeatureState } from "../../device/types";
import { makeModel } from "../../test/model";
import { ToastContainer } from "../Layout/ToastContainer";
import { FeatureControl } from "./FeatureControl";

const model = makeModel({
  ExposureTime: { kind: "Float", display_name: "Exposure Time" },
  PixelFormat: {
    kind: "Enumeration",
    enum_entries: [
      { name: "Mono8", display_name: "Mono 8" },
      { name: "Mono16" },
      { name: "RGB8" },
    ],
  },
});
const nodes = model.graph.nodes_by_name;

function exposure(value: number, extra: Partial<FeatureState> = {}): FeatureState {
  return {
    value,
    access_mode: "RW",
    kind: "Float",
    is_implemented: true,
    is_available: true,
    numeric: { min: 10, max: 100_000 },
    unit: "us",
    ...extra,
  };
}

/** The camera, as the backend's IPC answers for it; records every write. */
function camera(states: Record<string, FeatureState>, refuse?: string) {
  const writes: Array<{ nodeName: string; value: unknown }> = [];
  mockIPC((cmd, args) => {
    const a = args as Record<string, unknown>;
    if (cmd === "query_feature_states_bulk") {
      return Object.fromEntries(
        (a.names as string[]).filter((n) => n in states).map((n) => [n, states[n]]),
      );
    }
    if (cmd === "write_node") {
      writes.push({ nodeName: a.nodeName as string, value: a.value });
      if (refuse) throw new Error(refuse);
      return states[a.nodeName as string];
    }
    return undefined;
  });
  return writes;
}

let store: FeatureStore;
function StoreProbe({ onStore }: { onStore: (store: FeatureStore) => void }) {
  const found = useFeatureStore();
  useEffect(() => onStore(found), [found, onStore]);
  return null;
}
const keepStore = (found: FeatureStore) => {
  store = found;
};

function show(name: keyof typeof nodes) {
  return render(
    <ToastProvider>
      <FeatureStoreProvider connected={true}>
        <StoreProbe onStore={keepStore} />
        <FeatureControl node={nodes[name]} />
      </FeatureStoreProvider>
      <ToastContainer />
    </ToastProvider>,
  );
}

const field = () => screen.getByRole("spinbutton", { name: "Exposure Time" }) as HTMLInputElement;

describe("FeatureControl", () => {
  it("draws the camera's value and range", async () => {
    camera({ ExposureTime: exposure(1000) });
    show("ExposureTime");
    await waitFor(() => expect(field().value).toBe("1000"));
    const slider = screen.getByRole("slider") as HTMLInputElement;
    expect(slider.min).toBe("10");
    expect(slider.max).toBe("100000");
  });

  it("tells the user when the camera refuses a write", async () => {
    camera({ ExposureTime: exposure(1000) }, "node is locked: ExposureTime");
    show("ExposureTime");
    await waitFor(() => expect(field().value).toBe("1000"));
    await userEvent.clear(field());
    await userEvent.type(field(), "2000{Enter}");
    expect(await screen.findByText(/node is locked: ExposureTime/)).toBeTruthy();
  });

  it("keeps what the user typed when the camera reports a change", async () => {
    camera({ ExposureTime: exposure(1000) });
    show("ExposureTime");
    await waitFor(() => expect(field().value).toBe("1000"));
    await userEvent.clear(field());
    await userEvent.type(field(), "1234");
    act(() => store.set("ExposureTime", exposure(1500)));
    expect(field().value).toBe("1234");
  });

  // Codex on #167: focus alone is not an edit.
  it("follows the camera while it only has focus, and writes nothing on leaving", async () => {
    const writes = camera({ ExposureTime: exposure(1000) });
    show("ExposureTime");
    await waitFor(() => expect(field().value).toBe("1000"));
    act(() => field().focus());
    act(() => store.set("ExposureTime", exposure(1500)));
    expect(field().value).toBe("1500");
    act(() => field().blur());
    expect(writes).toEqual([]);
  });

  it("greys out a locked feature and says why", async () => {
    camera({ ExposureTime: exposure(1000, { access_mode: "RO" }) });
    show("ExposureTime");
    await waitFor(() => expect(field().disabled).toBe(true));
    expect((screen.getByRole("slider") as HTMLInputElement).disabled).toBe(true);
    expect(screen.getByText("Read-only now")).toBeTruthy();
  });

  it("greys out a feature the camera's state makes unavailable", async () => {
    camera({ ExposureTime: exposure(1000, { is_available: false, access_mode: "RO" }) });
    show("ExposureTime");
    expect(await screen.findByText(/Not available/)).toBeTruthy();
    expect(field().disabled).toBe(true);
  });

  it("draws nothing for a feature the camera does not implement", async () => {
    camera({ ExposureTime: exposure(1000, { is_implemented: false }) });
    show("ExposureTime");
    await waitFor(() => expect(store.get("ExposureTime")).toBeDefined());
    expect(screen.queryByRole("spinbutton")).toBeNull();
  });

  it("offers the enumeration entries the camera accepts now", async () => {
    camera({
      PixelFormat: {
        value: "Mono8",
        access_mode: "RW",
        kind: "Enumeration",
        is_implemented: true,
        is_available: true,
        enum_available: ["Mono8", "Mono16"],
      },
    });
    show("PixelFormat");
    const select = (await screen.findByRole("combobox", { name: "PixelFormat" })) as HTMLSelectElement;
    await waitFor(() => expect(select.value).toBe("Mono8"));
    const options = [...select.options].map((o) => o.textContent);
    expect(options).toEqual(["Mono 8", "Mono16"]);
  });
});

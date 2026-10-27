import { emit } from "@tauri-apps/api/event";
import { mockIPC } from "@tauri-apps/api/mocks";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactNode } from "react";
import { beforeEach, expect, it, vi } from "vitest";
import type { FeatureState, SfncGroup } from "../../device/types";
import { makeModel } from "../../test/model";
import { AppLayout } from "./AppLayout";

// Keep the real layout, ImageViewer, shared feature store and controls. The
// unrelated XML editor does not need to parse fixtures or load presets here.
vi.mock("../FeatureBrowser/FeatureBrowserPage", () => ({ FeatureBrowserPage: ({ preview }: { preview?: ReactNode }) => <div>{preview}</div> }));
vi.mock("../ImageViewer/ViewerCanvas", () => ({ ViewerCanvas: () => null }));
const id = "st61-camera";
const connected = { kind: "connected", device_id: id, device_name: "Bench camera", model: "ST61" };
const groups: SfncGroup[] = [
  { id: "acquisition_control", title: "Acquisition Control", icon: "play", default_open: true,
    features: [{ node: "AcquisitionFrameRate", aliases: ["AcquisitionFrameRateAbs"], widget: "float_input" }] },
  { id: "exposure_gain", title: "Exposure & Gain", icon: "sun", default_open: true,
    features: [{ node: "ExposureTime", aliases: ["ExposureTimeAbs", "ExposureTimeRaw"], widget: "float_input" },
      { node: "Gain", aliases: ["GainAbs", "GainRaw"], widget: "float_input" }] },
];
function numeric(value: number, unit: string, extra: Partial<FeatureState> = {}): FeatureState {
  return { kind: "Float", value, access_mode: "RW", is_implemented: true, is_available: true,
    numeric: { min: 1, max: 5000, inc: 1 }, unit, ...extra };
}
beforeEach(() => localStorage.clear());

it("restores Image fast controls from replacement XML without reconnecting the camera again", async () => {
  const initial = makeModel({ ExposureTime: { kind: "Float" }, Gain: { kind: "Float" }, AcquisitionFrameRate: { kind: "Float" } });
  const replacement = makeModel({
    ExposureTimeAbs: { kind: "Float", display_name: "Exposure absolute" },
    GainRaw: { kind: "Integer", display_name: "Raw gain" },
    AcquisitionFrameRateAbs: { kind: "Float", display_name: "Frame rate absolute" },
  });
  const states: Record<string, FeatureState> = {
    ExposureTime: numeric(100, "us"), Gain: numeric(2, "dB"), AcquisitionFrameRate: numeric(30, "Hz"),
    ExposureTimeAbs: numeric(2000, "us", { access_mode: "RO" }),
    GainRaw: numeric(12, "counts", { kind: "Integer" }), AcquisitionFrameRateAbs: numeric(50, "Hz"),
  };
  const writes: Array<{ nodeName: string; value: unknown }> = [];
  let connections = 0;
  mockIPC(async (cmd, raw) => {
    const args = raw as Record<string, unknown>;
    if (cmd === "list_discovered_devices") return [{ id, name: "Bench camera", model: "ST61", serial: "", transport: "zenoh" }];
    if (cmd === "get_connection_state") return { kind: "disconnected" };
    if (cmd === "get_sfnc_groups") return groups;
    if (cmd === "backend_status") return null;
    if (cmd === "connect_device") { connections++; await emit("connection-state-changed", connected); return initial; }
    if (cmd === "query_feature_states_bulk") return Object.fromEntries((args.names as string[]).filter((name) => states[name]).map((name) => [name, states[name]]));
    if (cmd === "write_node") {
      const nodeName = args.nodeName as string;
      writes.push({ nodeName, value: args.value });
      states[nodeName] = { ...states[nodeName], value: args.value as number };
      return states[nodeName];
    }
    return undefined;
  }, { shouldMockEvents: true });
  render(<AppLayout />);
  await userEvent.click(await screen.findByRole("button", { name: /1 device/ }));
  await userEvent.click(await screen.findByRole("button", { name: /Bench camera.*ST61/ }));
  await userEvent.click(screen.getByRole("button", { name: "Image" }));
  await waitFor(() => expect((screen.getByRole("spinbutton", { name: "ExposureTime" }) as HTMLInputElement).value).toBe("100"));

  await act(async () => {
    await emit("connection-state-changed", { kind: "reconnecting", device_id: id, attempt: 0, max_attempts: 5, reason: "reboot" });
    await emit("disconnect-reason", { device_id: id, message: "reboot" });
  });
  await act(async () => {
    await emit("connection-state-changed", connected);
    await emit("device-reconnected", { device_id: id, node_count: replacement.summary.node_count, model: replacement });
  });
  const exposure = await screen.findByRole("spinbutton", { name: "Exposure absolute" }) as HTMLInputElement;
  await waitFor(() => expect(exposure.value).toBe("2000"));
  expect(exposure.disabled).toBe(true);
  expect((screen.getByRole("spinbutton", { name: "Frame rate absolute" }) as HTMLInputElement).value).toBe("50");
  const gain = screen.getByRole("spinbutton", { name: "Raw gain" }) as HTMLInputElement;
  expect(gain.value).toBe("12");
  expect(gain.closest(".iv-row")?.textContent).toContain("counts");
  await userEvent.clear(gain);
  await userEvent.type(gain, "17{Enter}");
  await waitFor(() => expect(writes).toContainEqual({ nodeName: "GainRaw", value: 17 }));
  expect(connections).toBe(1);
  const order = [...document.querySelectorAll(".sidebar-section__label")].map((el) => el.textContent);
  expect(order.slice(0, 3)).toEqual(["Acquisition Control", "Exposure & Gain", "Statistics"]);
});

it.each([false, true])("keeps configuration visible on Start only when its preview is expanded (%s)", async (expanded) => {
  mockIPC(async (cmd) => {
    if (cmd === "list_discovered_devices") return [{ id, name: "Bench camera", model: "ST61", serial: "", transport: "zenoh" }];
    if (cmd === "get_connection_state") return { kind: "disconnected" };
    if (cmd === "get_sfnc_groups") return [];
    if (cmd === "backend_status") return null;
    if (cmd === "connect_device") { await emit("connection-state-changed", connected); return makeModel({}); }
    if (cmd === "query_feature_states_bulk") return {};
    if (cmd === "start_acquisition") return { ws_url: "ws://127.0.0.1:1", width: 640, height: 480 };
    return undefined;
  }, { shouldMockEvents: true });
  render(<AppLayout />);
  await userEvent.click(await screen.findByRole("button", { name: /1 device/ }));
  await userEvent.click(await screen.findByRole("button", { name: /Bench camera.*ST61/ }));
  if (expanded) await userEvent.click(screen.getByRole("button", { name: /Image preview/ }));
  await userEvent.click(screen.getByRole("button", { name: "Start Acq" }));
  const chosen = expanded ? "Features" : "Image";
  expect(screen.getByRole("button", { name: chosen }).className).toContain("app-header__tab--active");
});

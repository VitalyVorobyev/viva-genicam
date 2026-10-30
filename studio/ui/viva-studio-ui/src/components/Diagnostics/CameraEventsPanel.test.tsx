import { emit } from "@tauri-apps/api/event";
import { mockIPC } from "@tauri-apps/api/mocks";
import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { ToastProvider } from "../../context/ToastContext";
import { FeatureStoreProvider } from "../../device/FeatureStoreContext";
import type { CameraEvent, EventMonitorStatus } from "../../device/types";
import { ToastContainer } from "../Layout/ToastContainer";
import { CameraEventsProvider, MAX_CAMERA_EVENTS } from "../../device/CameraEventsContext";
import { CameraEventsPanel } from "./CameraEventsPanel";

function event(ts: number, overrides: Partial<CameraEvent> = {}): CameraEvent {
  return {
    event_id: "0x9001",
    name: "ExposureEnd",
    ts_dev: String(ts),
    ts_host_ns: null,
    stream_channel: 0,
    block_id: String(ts),
    features: [
      { name: "EventExposureEndFrameID", value: String(ts), error: null },
      { name: "EventExposureEndTimestamp", value: null, error: "not attached" },
    ],
    ...overrides,
  };
}

/** The backend as the panel talks to it; records start and stop calls. */
function backend(entries: string[], stopWarnings: string[] = []) {
  const calls: Array<[string, unknown]> = [];
  mockIPC(
    (cmd, args) => {
      if (cmd === "list_camera_events") return entries;
      if (cmd === "start_camera_events") {
        calls.push([cmd, args]);
        const events = (args as { events: string[] }).events;
        return { active: true, events, warnings: [] } satisfies EventMonitorStatus;
      }
      if (cmd === "stop_camera_events") {
        calls.push([cmd, args]);
        return { active: false, events: [], warnings: stopWarnings } satisfies EventMonitorStatus;
      }
      if (cmd === "query_feature_states_bulk") return {};
      return undefined;
    },
    { shouldMockEvents: true },
  );
  return calls;
}

function tree(readOnly: boolean, panel: boolean) {
  return (
    <ToastProvider>
      <FeatureStoreProvider connected={true} readOnly={readOnly}>
        <CameraEventsProvider connected={true}>
          {panel && <CameraEventsPanel connected={true} />}
        </CameraEventsProvider>
      </FeatureStoreProvider>
      <ToastContainer />
    </ToastProvider>
  );
}

function show(readOnly = false) {
  const view = render(tree(readOnly, true));
  return { ...view, setPanel: (panel: boolean) => view.rerender(tree(readOnly, panel)) };
}

describe("CameraEventsPanel", () => {
  it("starts the chosen events and stops them", async () => {
    const calls = backend(["ExposureEnd", "FrameStart"]);
    show();
    const user = userEvent.setup();
    await user.click(await screen.findByLabelText("FrameStart"));
    const start = screen.getByRole("button", { name: "Start" });
    expect(start).toHaveProperty("disabled", false);
    await user.click(start);
    expect(await screen.findByText("Monitoring FrameStart")).toBeTruthy();
    expect(screen.getByRole("group", { name: "Events to monitor" })).toHaveProperty("disabled", true);

    await user.click(screen.getByRole("button", { name: "Stop" }));
    expect(await screen.findByText("Not monitoring")).toBeTruthy();
    expect(calls).toEqual([
      ["start_camera_events", { events: ["FrameStart"] }],
      ["stop_camera_events", {}],
    ]);
  });

  it("shows each event newest first, with its features on demand", async () => {
    backend(["ExposureEnd"]);
    show();
    await screen.findByLabelText("ExposureEnd");
    await act(() => emit("camera-event", event(1)));
    await act(() => emit("camera-event", event(2, { name: null, event_id: "0x9C40", ts_host_ns: "1700000000000000001" })));

    const rows = within(screen.getByRole("list", { name: "Received camera events" })).getAllByRole("listitem");
    expect(rows).toHaveLength(2);
    expect(within(rows[0]).getByText("Unnamed event")).toBeTruthy();
    expect(within(rows[0]).getByText("0x9C40")).toBeTruthy();
    expect(within(rows[0]).getByText("host 1700000000000000001 ns")).toBeTruthy();
    expect(within(rows[1]).getByText("ExposureEnd")).toBeTruthy();
    expect(within(rows[1]).getByText("host time not calibrated")).toBeTruthy();

    await userEvent.setup().click(within(rows[1]).getByText("ExposureEnd"));
    expect(within(rows[1]).getByText("EventExposureEndFrameID")).toBeTruthy();
    expect(within(rows[1]).getByText("Error: not attached")).toBeTruthy();
  });

  it("keeps monitoring, the choice and arriving events while the tab is not shown", async () => {
    backend(["ExposureEnd", "FrameStart"]);
    const view = show();
    const user = userEvent.setup();
    await user.click(await screen.findByLabelText("FrameStart"));
    await user.click(screen.getByRole("button", { name: "Start" }));
    await screen.findByText("Monitoring FrameStart");

    view.setPanel(false);
    await act(() => emit("camera-event", event(7)));
    view.setPanel(true);

    expect(await screen.findByText("Monitoring FrameStart")).toBeTruthy();
    expect(screen.getByLabelText("FrameStart")).toHaveProperty("checked", true);
    const rows = within(screen.getByRole("list", { name: "Received camera events" })).getAllByRole("listitem");
    expect(rows).toHaveLength(1);
  });

  it(`keeps only the newest ${MAX_CAMERA_EVENTS} events`, async () => {
    backend(["ExposureEnd"]);
    show();
    await screen.findByLabelText("ExposureEnd");
    await act(async () => {
      for (let ts = 0; ts < MAX_CAMERA_EVENTS + 5; ts++) await emit("camera-event", event(ts));
    });
    const rows = within(screen.getByRole("list", { name: "Received camera events" })).getAllByRole("listitem");
    expect(rows).toHaveLength(MAX_CAMERA_EVENTS);
    expect(within(rows[0]).getByText(`ticks ${MAX_CAMERA_EVENTS + 4}`)).toBeTruthy();
  });

  it("shows what stopping could not restore, including a stop the backend made itself", async () => {
    backend(["ExposureEnd"], ["EventNotification[ExposureEnd] was not restored: timeout"]);
    show();
    const user = userEvent.setup();
    await user.click(await screen.findByLabelText("ExposureEnd"));
    await user.click(screen.getByRole("button", { name: "Start" }));
    await user.click(await screen.findByRole("button", { name: "Stop" }));
    expect((await screen.findByRole("alert")).textContent).toContain("was not restored: timeout");

    await act(() =>
      emit("camera-events-status", {
        active: false,
        events: [],
        warnings: ["Event settings were not restored: the camera was lost before monitoring stopped"],
      }),
    );
    expect(screen.getByRole("alert").textContent).toContain("the camera was lost");
  });

  it("says when the camera has no events, and offers no Start", async () => {
    backend([]);
    show();
    expect(await screen.findByText(/declares no EventSelector/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Start" })).toBeNull();
  });

  it("offers no Start in a read-only session", async () => {
    backend(["ExposureEnd"]);
    show(true);
    const user = userEvent.setup();
    await user.click(await screen.findByLabelText("ExposureEnd"));
    expect(screen.getByRole("button", { name: "Start" })).toHaveProperty("disabled", true);
    expect(screen.getByText(/Read-only session/)).toBeTruthy();
  });
});

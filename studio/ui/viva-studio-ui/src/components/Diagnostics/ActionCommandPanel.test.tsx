import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { ToastProvider } from "../../context/ToastContext";
import { FeatureStoreProvider } from "../../device/FeatureStoreContext";
import type { ActionCommandResult, FeatureState } from "../../device/types";
import { ToastContainer } from "../Layout/ToastContainer";
import { ActionCommandPanel } from "./ActionCommandPanel";

function integer(value: number | null, access_mode = "RW"): FeatureState {
  return { value, access_mode, kind: "Integer", is_implemented: true, is_available: true };
}

/** The backend as the panel talks to it; records the actions sent. */
function backend(destination: string | Error, states: Record<string, FeatureState> = {}) {
  const sent: unknown[] = [];
  mockIPC((cmd, args) => {
    const a = args as Record<string, unknown>;
    if (cmd === "default_action_destination") {
      if (destination instanceof Error) throw destination.message;
      return destination;
    }
    if (cmd === "query_feature_states_bulk") {
      return Object.fromEntries(
        (a.names as string[]).filter((n) => n in states).map((n) => [n, states[n]]),
      );
    }
    if (cmd === "send_action_command") {
      sent.push(a);
      return {
        destination: a.destination as string,
        sent: 1,
        acks: 1,
        refusals: [{ source: "192.168.2.20", status: "NOT_IMPLEMENTED" }],
      } satisfies ActionCommandResult;
    }
    return undefined;
  }, { shouldMockEvents: true });
  return sent;
}

async function openPanel() {
  render(
    <ToastProvider>
      <FeatureStoreProvider connected={true}>
        <ActionCommandPanel />
      </FeatureStoreProvider>
      <ToastContainer />
    </ToastProvider>,
  );
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: "Send an action command…" }));
  return user;
}

describe("ActionCommandPanel", () => {
  it("pre-fills the camera's keys and the subnet broadcast, and shows acks and refusals", async () => {
    const sent = backend("192.168.2.255", {
      ActionDeviceKey: integer(0x42),
      ActionGroupKey: integer(1),
      ActionGroupMask: integer(0xffffffff),
    });
    const user = await openPanel();
    expect(await screen.findByDisplayValue("0x00000042")).toBeTruthy();
    expect(screen.getByDisplayValue("0xFFFFFFFF")).toBeTruthy();
    expect(screen.getByLabelText("Destination")).toHaveProperty("value", "192.168.2.255");
    expect(screen.getByRole("note").textContent).toContain("broadcast");

    await user.type(screen.getByLabelText("Scheduled time (ticks)"), "123456789012345678");
    await user.click(screen.getByRole("button", { name: "Send" }));
    expect(await screen.findByText("Sent to 192.168.2.255: acknowledged by 1 device.")).toBeTruthy();
    expect(screen.getByText("192.168.2.20 refused: NOT_IMPLEMENTED")).toBeTruthy();
    expect(sent).toEqual([
      {
        destination: "192.168.2.255",
        deviceKey: "0x00000042",
        groupKey: "0x00000001",
        groupMask: "0xFFFFFFFF",
        scheduledTime: "123456789012345678",
        timeoutMs: 200,
      },
    ]);
  });

  it("asks for a write-only device key and refuses to send without it", async () => {
    const sent = backend("255.255.255.255", { ActionDeviceKey: integer(null, "WO") });
    const user = await openPanel();
    expect(await screen.findByText(/ActionDeviceKey is write-only/)).toBeTruthy();
    const send = screen.getByRole("button", { name: "Send" });
    expect(send).toHaveProperty("disabled", true);

    await user.type(screen.getByLabelText("Device key (hex)"), "42");
    await user.type(screen.getByLabelText("Group key (hex)"), "1");
    await user.type(screen.getByLabelText("Group mask (hex)"), "1");
    await user.clear(screen.getByLabelText("Destination"));
    await user.type(screen.getByLabelText("Destination"), "300.1.1.1");
    expect(send).toHaveProperty("disabled", true);
    await user.clear(screen.getByLabelText("Destination"));
    await user.type(screen.getByLabelText("Destination"), "10.0.0.255");
    expect(send).toHaveProperty("disabled", false);
    await user.click(send);
    expect(await screen.findByText(/Sent to 10.0.0.255/)).toBeTruthy();
    expect(sent).toHaveLength(1);
  });

  it("explains why remote mode sends none", async () => {
    backend(new Error("Action commands require embedded mode"));
    await openPanel();
    expect(await screen.findByText(/require embedded mode/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Send" })).toBeNull();
  });
});

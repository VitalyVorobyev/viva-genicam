import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { ToastProvider } from "../../context/ToastContext";
import { FeatureStoreProvider } from "../../device/FeatureStoreContext";
import type { FeatureState, NetworkConfig } from "../../device/types";
import { ToastContainer } from "../Layout/ToastContainer";
import { DevicePanel } from "./DevicePanel";

function text(value: string): FeatureState {
  return {
    value,
    access_mode: "RO",
    kind: "String",
    is_implemented: true,
    is_available: true,
  };
}

const NETWORK: NetworkConfig = {
  current_ip: "169.254.10.20",
  persistent_ip: "0.0.0.0",
  persistent_subnet: "0.0.0.0",
  persistent_gateway: "0.0.0.0",
  mac: "DE:AD:BE:EF:CA:FE",
};

/** The camera as the backend answers for it; records persistent-IP writes. */
function camera(network: NetworkConfig | string) {
  const states: Record<string, FeatureState> = {
    DeviceVendorName: text("Viva"),
    DeviceModelName: text("FakeGigE"),
    DeviceSerialNumber: text("SN-0001"),
  };
  const writes: unknown[] = [];
  mockIPC((cmd, args) => {
    const a = args as Record<string, unknown>;
    if (cmd === "query_feature_states_bulk") {
      return Object.fromEntries(
        (a.names as string[]).filter((n) => n in states).map((n) => [n, states[n]]),
      );
    }
    if (cmd === "get_network_config") {
      if (typeof network === "string") throw new Error(network);
      return network;
    }
    if (cmd === "set_persistent_ip") {
      writes.push(a);
      return null;
    }
    return undefined;
  });
  return writes;
}

function show() {
  return render(
    <ToastProvider>
      <FeatureStoreProvider connected={true}>
        <DevicePanel connected={true} />
      </FeatureStoreProvider>
      <ToastContainer />
    </ToastProvider>,
  );
}

describe("DevicePanel", () => {
  // What a user reads out when filing a report (backlog ST-37).
  it("shows who the camera is and where it is on the network", async () => {
    camera(NETWORK);
    show();
    expect(await screen.findByText("FakeGigE")).toBeTruthy();
    expect(screen.getByText("SN-0001")).toBeTruthy();
    expect(await screen.findByText("DE:AD:BE:EF:CA:FE")).toBeTruthy();
    expect(screen.getByText("169.254.10.20")).toBeTruthy();
    // A feature the camera does not have gets no row.
    expect(screen.queryByText("Firmware")).toBeNull();
  });

  it("writes the persistent address the user enters", async () => {
    const writes = camera(NETWORK);
    show();
    const user = userEvent.setup();
    const ip = await screen.findByLabelText("Persistent IP address");
    await user.clear(ip);
    await user.type(ip, "192.168.1.100");
    const subnet = screen.getByLabelText("Subnet mask");
    await user.clear(subnet);
    await user.type(subnet, "255.255.255.0");
    const gateway = screen.getByLabelText("Default gateway");
    await user.clear(gateway);
    await user.type(gateway, "192.168.1.1");
    await user.click(screen.getByRole("button", { name: "Save to camera" }));

    expect(writes).toEqual([
      { ip: "192.168.1.100", subnet: "255.255.255.0", gateway: "192.168.1.1" },
    ]);
    expect(await screen.findByText(/from its next restart/)).toBeTruthy();
  });

  it("says why the address cannot be set, rather than offering a form", async () => {
    camera("Network configuration not supported in this mode");
    show();
    expect(await screen.findByText(/not supported in this mode/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Save to camera" })).toBeNull();
  });
});

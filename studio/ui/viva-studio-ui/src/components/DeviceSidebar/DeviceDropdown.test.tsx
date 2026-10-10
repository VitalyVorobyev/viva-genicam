import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { ToastProvider } from "../../context/ToastContext";
import type { DeviceInfo } from "../../device/types";
import { ToastContainer } from "../Layout/ToastContainer";
import { DeviceDropdown } from "./DeviceDropdown";

const camera: DeviceInfo = {
  id: "gige-000cdf010203",
  name: "Camera",
  model: "FS-3200T",
  serial: "U321",
  transport: "gige",
  ip: "192.168.50.20",
  mac: "00:0C:DF:01:02:03",
};

function show() {
  return render(
    <ToastProvider>
      <DeviceDropdown
        devices={[camera]}
        connectionState={{ kind: "disconnected" }}
        onConnect={() => {}}
        onDisconnect={() => {}}
      />
      <ToastContainer />
    </ToastProvider>,
  );
}

describe("DeviceDropdown", () => {
  // A camera on another subnet is discovered but cannot be opened; FORCEIP
  // gives it an address on this one until it restarts (backlog ST-37).
  it("gives a discovered camera a temporary address", async () => {
    const sent: unknown[] = [];
    mockIPC((cmd, args) => {
      if (cmd === "force_ip") sent.push(args);
      return null;
    });
    show();
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: /1 device/ }));
    await user.click(screen.getByRole("button", { name: "Set IP…" }));

    const ip = screen.getByLabelText("Temporary IP address");
    await user.clear(ip);
    await user.type(ip, "169.254.10.20");
    const subnet = screen.getByLabelText("Subnet mask");
    await user.clear(subnet);
    await user.type(subnet, "255.255.0.0");
    await user.click(screen.getByRole("button", { name: "Send" }));

    expect(sent).toEqual([
      { mac: "00:0C:DF:01:02:03", ip: "169.254.10.20", subnet: "255.255.0.0", gateway: "0.0.0.0" },
    ]);
    expect(await screen.findByText(/until it restarts/)).toBeTruthy();
  });
});

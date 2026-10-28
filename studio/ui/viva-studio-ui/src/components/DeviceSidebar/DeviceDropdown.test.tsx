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

it("offers explicit control and monitor access while explaining another subnet", async () => {
  const choices: unknown[] = [];
  render(<ToastProvider><DeviceDropdown devices={[{ ...camera, shares_subnet_with_host: false }]}
    connectionState={{ kind: "disconnected" }} embedded
    onConnect={(id, access) => choices.push([id, access])} onDisconnect={() => {}} />
  </ToastProvider>);
  await userEvent.click(screen.getByRole("button", { name: /1 device/ }));
  expect(screen.getByText("Different subnet")).toBeTruthy();
  expect((screen.getByRole("button", { name: /U321.*FS-3200T/ }) as HTMLButtonElement).disabled).toBe(false);
  expect(screen.getByRole("button", { name: "Set IP…" })).toBeTruthy();
  await userEvent.click(screen.getByRole("button", { name: "Monitor camera U321" }));
  expect(choices).toEqual([[camera.id, "monitor"]]);
});

it.each(["remote", "usb"])("labels direct monitor unavailable for %s and never offers remote network writes", async (mode) => {
  const device = mode === "usb" ? { ...camera, transport: "usb3" } : camera;
  render(<ToastProvider><DeviceDropdown devices={[device]} connectionState={{ kind: "disconnected" }}
    embedded={mode !== "remote"} onConnect={() => {}} onDisconnect={() => {}} /></ToastProvider>);
  await userEvent.click(screen.getByRole("button", { name: /1 device/ }));
  const monitor = screen.getByRole("button", { name: "Monitor camera U321" }) as HTMLButtonElement;
  expect(monitor.disabled).toBe(true);
  if (mode === "remote") {
    expect(screen.getByText(/Direct monitor access is unavailable in remote mode/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Set IP…" })).toBeNull();
  }
});

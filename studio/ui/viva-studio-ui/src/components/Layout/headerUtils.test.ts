import { describe, it, expect } from "vitest";
import { truncateDeviceName, formatDeviceChip } from "./headerUtils";

describe("truncateDeviceName", () => {
  it("test_truncate_short_name_unchanged — name under limit passes through unchanged", () => {
    const name = "ShortName";
    expect(truncateDeviceName(name)).toBe("ShortName");
  });

  it("test_truncate_long_name_ellipsis — name over limit is truncated with ellipsis", () => {
    const name = "AVeryLongDeviceNameThatExceedsTheMaximumLength";
    const result = truncateDeviceName(name, 22);
    // slice(0, 22) = "AVeryLongDeviceNameTha" (22 chars) + ellipsis
    expect(result).toBe("AVeryLongDeviceNameTha\u2026");
    expect(result.length).toBe(23); // 22 chars + ellipsis (U+2026 is 1 code unit)
  });

  it("test_truncate_exact_boundary — name exactly at limit is not truncated", () => {
    const name = "A".repeat(22);
    expect(truncateDeviceName(name, 22)).toBe(name);
  });
});

describe("formatDeviceChip", () => {
  it("identifies a reachable camera that lost control as read-only", () => {
    expect(formatDeviceChip({ kind: "control_lost", device_id: "cam0", device_name: "Bench", model: "M", reason: "taken" }))
      .toEqual({ label: "Bench (read-only)", state: "disconnected" });
  });
  it("test_format_chip_connected — returns label with truncated device name and connected state", () => {
    const result = formatDeviceChip({
      kind: "connected",
      device_id: "cam0",
      device_name: "BaslerAcA1920-155uc",
      model: "acA1920-155uc",
    });
    expect(result.label).toBe("BaslerAcA1920-155uc");
    expect(result.state).toBe("connected");
  });

  it("test_format_chip_disconnected — returns No device and disconnected state", () => {
    const result = formatDeviceChip({ kind: "disconnected" });
    expect(result.label).toBe("No device");
    expect(result.state).toBe("disconnected");
  });

  it("test_format_chip_connecting — returns Connecting ellipsis and connecting state", () => {
    const result = formatDeviceChip({ kind: "connecting", device_id: "cam0" });
    expect(result.label).toBe("Connecting\u2026");
    expect(result.state).toBe("connecting");
  });

  it("test_format_chip_reconnecting — reconnecting shows the attempt count", () => {
    const result = formatDeviceChip({
      kind: "reconnecting",
      device_id: "cam0",
      attempt: 2,
      max_attempts: 5,
      reason: "heartbeat timeout",
    });
    expect(result.label).toBe("Reconnecting (2/5)…");
    // Same treatment DeviceDropdown gives it: reconnection reuses "connecting".
    expect(result.state).toBe("connecting");
  });

  it("test_format_chip_error — error maps to disconnected chip with No device label", () => {
    const result = formatDeviceChip({ kind: "error", message: "Timeout" });
    expect(result.label).toBe("No device");
    expect(result.state).toBe("disconnected");
  });
});

import { describe, it, expect } from "vitest";
import { deviceDetail, deviceLabel } from "./deviceLabel";
import type { DeviceInfo } from "./types";

const base: DeviceInfo = {
  id: "cam-deadbeefcafe",
  name: "BFS-PGE-31S4C",
  model: "BFS-PGE-31S4C",
  serial: "",
  transport: "zenoh",
};

describe("deviceLabel", () => {
  it("prefers the user-defined name", () => {
    const d = { ...base, user_name: "Left", serial: "S1", ip: "192.168.1.10" };
    expect(deviceLabel(d)).toBe("Left");
    expect(deviceDetail(d)).toBe("BFS-PGE-31S4C · S1 · 192.168.1.10");
  });

  it("falls back to the serial", () => {
    const d = { ...base, serial: "S1", ip: "192.168.1.10" };
    expect(deviceLabel(d)).toBe("S1");
    expect(deviceDetail(d)).toBe("BFS-PGE-31S4C · 192.168.1.10");
  });

  it("falls back to the address when there is no serial", () => {
    const d = { ...base, ip: "192.168.1.10" };
    expect(deviceLabel(d)).toBe("192.168.1.10");
    expect(deviceDetail(d)).toBe("BFS-PGE-31S4C");
  });

  it("falls back to the name for a service that sends no identity", () => {
    expect(deviceLabel(base)).toBe("BFS-PGE-31S4C");
    expect(deviceDetail(base)).toBe("");
  });

  // #137: two cameras of one model must not render identically.
  it("tells two cameras of the same model apart", () => {
    const a = { ...base, serial: "S1", ip: "192.168.1.10" };
    const b = { ...base, id: "cam-deadbeefcaff", serial: "S2", ip: "192.168.1.11" };
    expect(deviceLabel(a)).not.toBe(deviceLabel(b));
  });
});

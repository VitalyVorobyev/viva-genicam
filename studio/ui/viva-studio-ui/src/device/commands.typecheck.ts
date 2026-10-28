// Compile-only assertions: tsc must reject stale command names and wire arguments.
import { invokeCommand } from "./commands";
import type { FeatureState } from "../generated/ipc";

export function commandContractTypechecks() {
  const state: Promise<FeatureState> = invokeCommand("write_node", {
    nodeName: "Width",
    value: 640,
  });
  const optional = invokeCommand("start_acquisition");
  // @ts-expect-error unknown command
  invokeCommand("renamed_write_node", { nodeName: "Width", value: 640 });
  // @ts-expect-error argument names use Tauri's camelCase wire convention
  invokeCommand("connect_device", { device_id: "127.0.0.1" });
  // @ts-expect-error required arguments cannot be omitted
  invokeCommand("connect_device");
  // @ts-expect-error packet size must be numeric
  invokeCommand("start_acquisition", { packetSize: "1500" });
  // @ts-expect-error response type is derived from the command's success type
  const wrong: Promise<string> = invokeCommand("backend_status");
  return { state, optional, wrong };
}

// Direct access is constrained by the generated Rust enum.
export function accessContractTypechecks() {
invokeCommand("connect_device", { deviceId: "fake-camera", access: "monitor" });
// @ts-expect-error Direct Studio access exposes control and monitor only.
invokeCommand("connect_device", { deviceId: "fake-camera", access: "exclusive" });
}

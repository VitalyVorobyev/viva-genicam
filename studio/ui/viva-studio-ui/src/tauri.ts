import { invokeCommand } from "./device/commands";
export function isTauri(): boolean {
  return typeof window.__TAURI_INTERNALS__ !== "undefined";
}

export async function ping(): Promise<string> {
  return await invokeCommand("ping");
}

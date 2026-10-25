import type { StudioCommands } from "../generated/ipc";

type Arguments<K extends keyof StudioCommands> =
  Record<string, never> extends StudioCommands[K]["args"]
    ? [args?: StudioCommands[K]["args"]]
    : [args: StudioCommands[K]["args"]];

/** Invoke only a registered command, using its generated wire contract. */
export async function invokeCommand<K extends keyof StudioCommands>(
  command: K,
  ...args: Arguments<K>
): Promise<StudioCommands[K]["result"]> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<StudioCommands[K]["result"]>(command, args[0]);
}

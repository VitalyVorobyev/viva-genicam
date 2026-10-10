import type { StudioEvents } from "./types";

/**
 * Listen for a backend event. The payload's type comes from the generated
 * `StudioEvents` map (backlog ST-28), so the event name and its shape cannot
 * disagree. Resolves to the function that stops listening.
 */
export async function listenTo<K extends keyof StudioEvents>(
  name: K,
  handler: (payload: StudioEvents[K]) => void,
): Promise<() => void> {
  const { listen } = await import("@tauri-apps/api/event");
  return listen<StudioEvents[K]>(name, (event) => handler(event.payload));
}

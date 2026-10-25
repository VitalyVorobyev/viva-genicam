import { invokeCommand } from "./commands";
import { useEffect, useState } from "react";
import { isTauri } from "../tauri";
import type { BackendStatus } from "./backendStatus";

/**
 * Reads the backend mode once at startup.
 *
 * The mode is fixed for the life of the process — it is decided in `main()`
 * before any window exists — so there is nothing to subscribe to and no reason
 * to poll. Returns `null` until the answer arrives, and outside Tauri.
 */
export function useBackendStatus(): BackendStatus | null {
  const [status, setStatus] = useState<BackendStatus | null>(null);

  useEffect(() => {
    if (!isTauri()) return;

    let cancelled = false;

    async function load() {
      try {
        const result = await invokeCommand("backend_status");
        if (!cancelled) setStatus(result);
      } catch (e) {
        console.error("Failed to query backend status:", e);
      }
    }

    void load();

    return () => {
      cancelled = true;
    };
  }, []);

  return status;
}

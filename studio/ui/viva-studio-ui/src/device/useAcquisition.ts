import { invokeCommand } from "./commands";
import { useCallback, useEffect, useState } from "react";
import { isTauri } from "../tauri";
import { NO_ACQUISITION, followAcquisition } from "./acquisitionStatus";
import { listenTo } from "./events";
import type { StreamerInfo } from "./types";

/**
 * The acquisition: its last status, the stream's counters — kept after Stop,
 * cleared by the next start — and the stream the viewer reads.
 */
export function useAcquisition() {
  const [acquisition, setAcquisition] = useState(NO_ACQUISITION);
  const [streamerInfo, setStreamerInfo] = useState<StreamerInfo | null>(null);

  useEffect(() => {
    if (!isTauri()) return;

    let unlistenStatus: (() => void) | null = null;
    let unlistenConnection: (() => void) | null = null;
    let cancelled = false;

    listenTo("acquisition-status", (next) => {
      setAcquisition((prev) => followAcquisition(prev, next));
    }).then((fn) => {
      if (cancelled) fn();
      else unlistenStatus = fn;
    });

    listenTo("connection-state-changed", (state) => {
      if (state.kind === "disconnected" || state.kind === "error") {
        setStreamerInfo(null);
        setAcquisition(NO_ACQUISITION);
      }
    }).then((fn) => {
      if (cancelled) fn();
      else unlistenConnection = fn;
    });

    return () => {
      cancelled = true;
      unlistenStatus?.();
      unlistenConnection?.();
    };
  }, []);

  /** Start streaming; `packetSize` is the user's, or `null` to negotiate. */
  const start = useCallback(async (packetSize: number | null = null): Promise<StreamerInfo> => {
    const info = await invokeCommand("start_acquisition", { packetSize });
    setStreamerInfo(info);
    return info;
  }, []);

  const stop = useCallback(async () => {
    await invokeCommand("stop_acquisition");
  }, []);

  return { status: acquisition.status, stats: acquisition.stats, streamerInfo, start, stop };
}

import { useCallback, useEffect, useState } from "react";
import { isTauri } from "../tauri";
import { listenTo } from "./events";
import type { AcquisitionStatus, StreamerInfo } from "./types";

export function useAcquisition() {
  const [status, setStatus] = useState<AcquisitionStatus>({
    active: false,
    fps: null,
    dropped: 0,
  });
  const [streamerInfo, setStreamerInfo] = useState<StreamerInfo | null>(null);

  useEffect(() => {
    if (!isTauri()) return;

    let unlistenStatus: (() => void) | null = null;
    let unlistenConnection: (() => void) | null = null;
    let cancelled = false;

    listenTo("acquisition-status", (next) => {
      setStatus(next);
    }).then((fn) => {
      if (cancelled) fn();
      else unlistenStatus = fn;
    });

    listenTo("connection-state-changed", (state) => {
      if (state.kind === "disconnected" || state.kind === "error") {
        setStreamerInfo(null);
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

  const start = useCallback(async (): Promise<StreamerInfo> => {
    const { invoke } = await import("@tauri-apps/api/core");
    const info = await invoke<StreamerInfo>("start_acquisition");
    setStreamerInfo(info);
    return info;
  }, []);

  const stop = useCallback(async () => {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("stop_acquisition");
  }, []);

  return { status, streamerInfo, start, stop };
}

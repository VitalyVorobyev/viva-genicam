import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { isTauri } from "../tauri";
import { listenTo } from "./events";
import type { CameraEvent, EventMonitorStatus } from "./types";

/** Events kept; the oldest go first. */
export const MAX_CAMERA_EVENTS = 500;

export type ReceivedCameraEvent = CameraEvent & { key: number };

const INACTIVE: EventMonitorStatus = { active: false, events: [], warnings: [] };

interface CameraEventsValue {
  status: EventMonitorStatus;
  setStatus: (status: EventMonitorStatus) => void;
  received: ReceivedCameraEvent[];
  clear: () => void;
  chosen: string[];
  setChosen: (update: (chosen: string[]) => string[]) => void;
}

const CameraEventsContext = createContext<CameraEventsValue | null>(null);

/**
 * Camera events and the monitoring status, held above the tabs so they keep
 * arriving while the Diagnostics tab is not shown (backlog ST-51). Everything
 * is dropped when the session ends.
 */
export function CameraEventsProvider({ connected, children }: { connected: boolean; children: ReactNode }) {
  const [status, setStatus] = useState<EventMonitorStatus>(INACTIVE);
  const [received, setReceived] = useState<ReceivedCameraEvent[]>([]);
  const [chosen, setChosen] = useState<string[]>([]);
  const nextKey = useRef(0);
  const [wasConnected, setWasConnected] = useState(connected);

  // Reset while rendering, not in an effect: a new session starts empty.
  if (wasConnected !== connected) {
    setWasConnected(connected);
    if (!connected) {
      setStatus(INACTIVE);
      setReceived([]);
      setChosen([]);
    }
  }

  useEffect(() => {
    if (!isTauri()) return;
    let cancelled = false;
    const unlisten: Array<() => void> = [];
    const keep = (fn: () => void) => {
      if (cancelled) fn();
      else unlisten.push(fn);
    };
    void listenTo("camera-event", (event) => {
      const key = nextKey.current++;
      setReceived((list) => [{ ...event, key }, ...list].slice(0, MAX_CAMERA_EVENTS));
    }).then(keep);
    void listenTo("camera-events-status", setStatus).then(keep);
    return () => {
      cancelled = true;
      for (const fn of unlisten) fn();
    };
  }, []);

  return (
    <CameraEventsContext.Provider
      value={{ status, setStatus, received, clear: () => setReceived([]), chosen, setChosen }}
    >
      {children}
    </CameraEventsContext.Provider>
  );
}

export function useCameraEvents(): CameraEventsValue {
  const ctx = useContext(CameraEventsContext);
  if (!ctx) {
    throw new Error("useCameraEvents must be used within CameraEventsProvider");
  }
  return ctx;
}

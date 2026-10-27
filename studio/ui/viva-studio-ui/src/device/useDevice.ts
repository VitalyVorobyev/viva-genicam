import { invokeCommand } from "./commands";
import { useCallback, useEffect, useRef, useState } from "react";
import { isTauri } from "../tauri";
import { listenTo } from "./events";
import type { ConnectionState, DeviceInfo, DisconnectReason } from "./types";
import type { ParseXmlResponse } from "../xml_model/uigraph";

export function useDevice() {
  const [devices, setDevices] = useState<DeviceInfo[]>([]);
  const [connectionState, setConnectionState] = useState<ConnectionState>({
    kind: "disconnected",
  });
  const [replacementModel, setReplacementModel] = useState<ParseXmlResponse | null>(null);
  const currentConnection = useRef<ConnectionState>({ kind: "disconnected" });
  const operation = useRef(0);
  const manualTransition = useRef(false);
  const [disconnectReason, setDisconnectReason] = useState<DisconnectReason | null>(null);

  useEffect(() => {
    if (!isTauri()) return;

    let cancelled = false;
    const unlisteners: Array<() => void> = [];
    const keep = (fn: () => void) => cancelled ? fn() : unlisteners.push(fn);

    async function init() {
      if (cancelled) return;

      // Load initial state without overwriting a user operation begun meanwhile.
      const initialOperation = operation.current;
      try {
        const [deviceList, connState] = await Promise.all([
          invokeCommand("list_discovered_devices"),
          invokeCommand("get_connection_state"),
        ]);
        if (!cancelled) {
          setDevices(deviceList);
          if (operation.current === initialOperation) {
            currentConnection.current = connState;
            setConnectionState(connState);
          }
        }
      } catch (e) {
        console.error("Failed to load initial device state:", e);
      }

      if (cancelled) return;

      // Subscribe to device-discovery events
      // Replace rather than ignore a known id: the id outlives an address
      // change, so a later announce can carry a new `ip`.
      const u1 = await listenTo("device-discovered", (device) => {
        setDevices((prev) =>
          prev.some((d) => d.id === device.id)
            ? prev.map((d) => (d.id === device.id ? device : d))
            : [...prev, device]
        );
      });

      keep(u1);
      const u2 = await listenTo("device-lost", (lost) => {
        setDevices((prev) => prev.filter((d) => d.id !== lost.device_id));
      });

      keep(u2);
      const u3 = await listenTo("connection-state-changed", (state) => {
        currentConnection.current = state;
        setConnectionState(state);
      });

      keep(u3);
      const u4 = await listenTo("disconnect-reason", (reason) => {
        setDisconnectReason(reason);
      });

      keep(u4);
      const u5 = await listenTo("device-reconnected", ({ device_id, model }) => {
        const current = currentConnection.current;
        if (manualTransition.current) return;
        if ((current.kind === "connected" || current.kind === "reconnecting") && current.device_id === device_id) {
          setReplacementModel(model);
        }
      });

      keep(u5);
    }

    init();

    return () => {
      cancelled = true;
      unlisteners.forEach((fn) => fn());
    };
  }, []);

  const connect = useCallback(async (deviceId: string): Promise<ParseXmlResponse> => {
    const ticket = ++operation.current;
    manualTransition.current = true;
    currentConnection.current = { kind: "connecting", device_id: deviceId };
    setDisconnectReason(null);
    setReplacementModel(null);
    try {
      const model = await invokeCommand("connect_device", { deviceId });
      if (ticket !== operation.current) throw new DOMException("Connection superseded", "AbortError");
      return model;
    } finally {
      if (ticket === operation.current) manualTransition.current = false;
    }
  }, []);

  const disconnect = useCallback(async () => {
    const ticket = ++operation.current;
    manualTransition.current = true;
    currentConnection.current = { kind: "disconnected" };
    setReplacementModel(null);
    try {
      await invokeCommand("disconnect_device");
    } finally {
      if (ticket === operation.current) manualTransition.current = false;
    }
  }, []);

  return { devices, connectionState, disconnectReason, connect, disconnect, replacementModel };
}

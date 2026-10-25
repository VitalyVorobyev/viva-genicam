import { invokeCommand } from "./commands";
import { useCallback, useEffect, useState } from "react";
import { isTauri } from "../tauri";
import { listenTo } from "./events";
import type { ConnectionState, DeviceInfo, DisconnectReason } from "./types";
import type { ParseXmlResponse } from "../xml_model/uigraph";

export function useDevice() {
  const [devices, setDevices] = useState<DeviceInfo[]>([]);
  const [connectionState, setConnectionState] = useState<ConnectionState>({
    kind: "disconnected",
  });
  const [disconnectReason, setDisconnectReason] = useState<DisconnectReason | null>(null);

  useEffect(() => {
    if (!isTauri()) return;

    let cancelled = false;
    const unlisteners: Array<() => void> = [];

    async function init() {
      if (cancelled) return;

      // Load initial state
      try {
        const [deviceList, connState] = await Promise.all([
          invokeCommand("list_discovered_devices"),
          invokeCommand("get_connection_state"),
        ]);
        if (!cancelled) {
          setDevices(deviceList);
          setConnectionState(connState);
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

      const u2 = await listenTo("device-lost", (lost) => {
        setDevices((prev) => prev.filter((d) => d.id !== lost.device_id));
      });

      const u3 = await listenTo("connection-state-changed", (state) => {
        setConnectionState(state);
      });

      const u4 = await listenTo("disconnect-reason", (reason) => {
        setDisconnectReason(reason);
      });

      unlisteners.push(u1, u2, u3, u4);
    }

    init();

    return () => {
      cancelled = true;
      unlisteners.forEach((fn) => fn());
    };
  }, []);

  const connect = useCallback(async (deviceId: string): Promise<ParseXmlResponse> => {
    setDisconnectReason(null);
    return await invokeCommand("connect_device", { deviceId });
  }, []);

  const disconnect = useCallback(async () => {
    await invokeCommand("disconnect_device");
  }, []);

  return { devices, connectionState, disconnectReason, connect, disconnect };
}

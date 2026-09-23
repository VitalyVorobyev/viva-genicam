import { useCallback, useEffect, useRef, useState } from "react";
import { deviceDetail, deviceLabel } from "../../device/deviceLabel";
import type { ConnectionState, DeviceInfo, DisconnectReason } from "../../device/types";

interface DeviceDropdownProps {
  devices: DeviceInfo[];
  connectionState: ConnectionState;
  disconnectReason?: DisconnectReason | null;
  onConnect: (deviceId: string) => void;
  onDisconnect: () => void;
}

/** Compact device selector for the header bar. Shows status pill + dropdown. */
export function DeviceDropdown({
  devices,
  connectionState,
  disconnectReason = null,
  onConnect,
  onDisconnect,
}: DeviceDropdownProps) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  // Close on outside click
  useEffect(() => {
    if (!open) return;
    function onClickOutside(e: MouseEvent) {
      if (ref.current && !ref.current.contains(e.target as Node)) {
        setOpen(false);
      }
    }
    document.addEventListener("mousedown", onClickOutside);
    return () => document.removeEventListener("mousedown", onClickOutside);
  }, [open]);

  // Close on Escape
  useEffect(() => {
    if (!open) return;
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") setOpen(false);
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open]);

  const handleConnect = useCallback(
    (id: string) => {
      onConnect(id);
      setOpen(false);
    },
    [onConnect],
  );

  const isConnected = connectionState.kind === "connected";
  const isConnecting = connectionState.kind === "connecting";
  const isReconnecting = connectionState.kind === "reconnecting";
  const isBusy = isConnecting || isConnected || isReconnecting;
  const hasError = connectionState.kind === "error";

  const dotClass = isConnected
    ? "device-dd__dot--connected"
    : isConnecting || isReconnecting
      ? "device-dd__dot--connecting"
      : hasError
        ? "device-dd__dot--error"
        : "device-dd__dot--idle";

  // Name the connected camera the way the list did, so it stays clear which of
  // two identical cameras is open.
  const connectedDevice = isConnected
    ? devices.find((d) => d.id === connectionState.device_id)
    : undefined;
  const connectedName = connectedDevice
    ? deviceLabel(connectedDevice)
    : isConnected
      ? connectionState.device_name
      : "";
  const connectedMeta = connectedDevice
    ? deviceDetail(connectedDevice)
    : isConnected
      ? connectionState.model
      : "";

  const label = isConnected
    ? connectedName
    : isConnecting
      ? "Connecting\u2026"
      : isReconnecting
        ? `Reconnecting (${connectionState.attempt}/${connectionState.max_attempts})\u2026`
        : devices.length > 0
          ? `${devices.length} device${devices.length !== 1 ? "s" : ""}`
          : "No device";

  return (
    <div className="device-dd" ref={ref}>
      <button
        type="button"
        className="device-dd__trigger"
        onClick={() => setOpen((p) => !p)}
        aria-expanded={open}
        aria-haspopup="listbox"
        title={isConnected ? `Connected to ${label}` : label}
      >
        <span className={`device-dd__dot ${dotClass}`} />
        <span className="device-dd__label">{label}</span>
        <span className="device-dd__caret">{open ? "\u25B4" : "\u25BE"}</span>
      </button>

      {open && (
        <div className="device-dd__panel" role="listbox">
          {/* Error/disconnect banner */}
          {hasError && (
            <div className="device-dd__error">
              {disconnectReason?.message ?? connectionState.message}
            </div>
          )}

          {/* Connected device */}
          {isConnected && (
            <div className="device-dd__connected">
              <div className="device-dd__device-name">{connectedName}</div>
              <div className="device-dd__device-meta">{connectedMeta}</div>
              <button type="button" className="btn--secondary btn--sm" onClick={onDisconnect}>
                Disconnect
              </button>
            </div>
          )}

          {/* Available devices */}
          {!isConnected && devices.length === 0 && (
            <div className="device-dd__scanning">
              <span className="device-dd__scanning-dots">
                <span className="device-sidebar__scanning-dot" />
                <span className="device-sidebar__scanning-dot" />
                <span className="device-sidebar__scanning-dot" />
              </span>
              {"Scanning\u2026"}
            </div>
          )}
          {!isConnected &&
            devices.map((d) => (
              <button
                key={d.id}
                type="button"
                className="device-dd__option"
                disabled={isBusy}
                onClick={() => handleConnect(d.id)}
              >
                <span className="device-dd__dot device-dd__dot--idle" />
                <span className="device-dd__option-text">
                  <span className="device-dd__option-name">{deviceLabel(d)}</span>
                  {deviceDetail(d) && (
                    <span className="device-dd__option-detail">{deviceDetail(d)}</span>
                  )}
                </span>
                {d.transport && (
                  <span className={`device-dd__transport device-dd__transport--${d.transport}`}>
                    {d.transport === "gige" ? "GigE" : d.transport === "usb3" ? "USB3" : d.transport}
                  </span>
                )}
              </button>
            ))}
        </div>
      )}
    </div>
  );
}

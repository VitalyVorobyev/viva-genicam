import { useCallback, useEffect, useRef, useState } from "react";
import { isTauri } from "../../tauri";
import { useDevice } from "../../device/useDevice";
import { useBackendStatus } from "../../device/useBackendStatus";
import { formatBackendChip } from "../../device/backendStatus";
import { FeatureStoreProvider } from "../../device/FeatureStoreContext";
import { loadPacketSize } from "../../device/packetSize";
import { useAcquisition } from "../../device/useAcquisition";
import { describeAcquisition, streamTransport } from "../../device/acquisitionStatus";
import { useImageMeta } from "../../device/useImageMeta";
import { AppLogProvider, useAppLog } from "../../context/AppLogContext";
import { ToastProvider, useToast } from "../../context/ToastContext";
import { DeviceDropdown } from "../DeviceSidebar/DeviceDropdown";
import { FeatureBrowserPage } from "../FeatureBrowser/FeatureBrowserPage";
import { ImageViewer } from "../ImageViewer/ImageViewer";
import { DiagnosticsTab } from "../Diagnostics/DiagnosticsTab";
import { ErrorBoundary } from "./ErrorBoundary";
import { ToastContainer } from "./ToastContainer";
import type { ParseXmlResponse } from "../../xml_model/uigraph";

type MainTab = "features" | "image" | "diagnostics";

// AppLayout is wrapped by AppLogProvider so all children can call useAppLog.
export function AppLayout() {
  return (
    <AppLogProvider>
      <ToastProvider>
        <ErrorBoundary>
          <AppLayoutInner />
        </ErrorBoundary>
      </ToastProvider>
    </AppLogProvider>
  );
}

function AppLayoutInner() {
  const [activeTab, setActiveTab] = useState<MainTab>("features");
  const [externalModel, setExternalModel] = useState<ParseXmlResponse | null>(null);

  const { devices, connectionState, disconnectReason, connect, disconnect } = useDevice();
  const [lastConnectedDeviceId, setLastConnectedDeviceId] = useState<string | null>(
    () => {
      try { return localStorage.getItem("viva-studio:last-device"); } catch { return null; }
    }
  );
  // Offered at most once per session; read only by the effect that offers it.
  const autoReconnectOffered = useRef(false);
  const {
    status: acqStatus,
    stats: acqStats,
    streamerInfo,
    start: startAcq,
    stop: stopAcq,
  } = useAcquisition();
  const acquisition = describeAcquisition(acqStatus);
  const { imageMeta } = useImageMeta();
  const backendStatus = useBackendStatus();
  const { log } = useAppLog();
  const { addToast } = useToast();

  const isConnected = connectionState.kind === "connected";
  const connectedDeviceName =
    connectionState.kind === "connected" ? connectionState.device_name : null;
  const connectedModel =
    connectionState.kind === "connected" ? connectionState.model : null;
  const connectedId = connectionState.kind === "connected" ? connectionState.device_id : null;
  const transport = streamTransport(
    devices.find((d) => d.id === connectedId),
    connectedId ?? "",
  );

  // T7.7 — keep document.title in sync with connection state
  useEffect(() => {
    document.title = connectedDeviceName
      ? `Viva Studio — ${connectedDeviceName}`
      : "Viva Studio";
  }, [connectedDeviceName]);

  // A ZENOH_CONFIG that was set but failed to load leaves the app in embedded
  // mode — running, but unable to see the service the user pointed it at (#132).
  // The header chip shows the state; this makes sure nobody misses the reason.
  useEffect(() => {
    const error = backendStatus?.zenoh_config_error;
    if (!error) return;
    addToast("error", error);
    log("error", error);
  }, [backendStatus, addToast, log]);

  // Track the last successfully connected device ID for the reconnect prompt:
  // the state follows the connection while rendering, and storage is written
  // after.
  if (connectionState.kind === "connected" && connectionState.device_id !== lastConnectedDeviceId) {
    setLastConnectedDeviceId(connectionState.device_id);
  }
  useEffect(() => {
    if (connectionState.kind !== "connected") return;
    try { localStorage.setItem("viva-studio:last-device", connectionState.device_id); } catch { /* */ }
  }, [connectionState]);

  // PH-07: Offer auto-reconnect when last-connected device reappears after app restart.
  useEffect(() => {
    if (autoReconnectOffered.current) return;
    if (connectionState.kind !== "disconnected") return;
    if (!lastConnectedDeviceId) return;
    const lastDevice = devices.find((d) => d.id === lastConnectedDeviceId);
    if (!lastDevice) return;

    autoReconnectOffered.current = true;
    const name = lastDevice.name || lastDevice.id;
    addToast("info", `Previously connected device "${name}" detected`);
  }, [devices, connectionState, lastConnectedDeviceId, addToast]);

  // Drop the stale model when an unexpected disconnect is reported.
  const [prevDisconnectReason, setPrevDisconnectReason] = useState(disconnectReason);
  if (disconnectReason !== prevDisconnectReason) {
    setPrevDisconnectReason(disconnectReason);
    if (disconnectReason !== null) setExternalModel(null);
  }

  const handleConnect = useCallback(
    async (deviceId: string) => {
      log("info", "Connecting to device\u2026", deviceId);
      try {
        const response = await connect(deviceId);
        setExternalModel(response);
        setActiveTab("features");
        log("success", `Connected`, deviceId);
        addToast("success", "Connected to device");
      } catch (e) {
        const msg =
          typeof e === "object" && e !== null && "message" in e
            ? String((e as { message?: unknown }).message)
            : String(e);
        log("error", `Connection failed: ${msg}`, deviceId);
        addToast("error", `Connection failed: ${msg}`);
      }
    },
    [connect, log, addToast]
  );

  const handleDisconnect = useCallback(async () => {
    const name = connectedDeviceName ?? "device";
    try {
      await disconnect();
      log("info", `Disconnected from ${name}`);
      addToast("info", `Disconnected from ${name}`);
    } catch (e) {
      log("error", `Disconnect error: ${String(e)}`);
      addToast("error", `Disconnect error: ${String(e)}`);
    }
  }, [disconnect, connectedDeviceName, log, addToast]);

  const handleStartAcquisition = useCallback(async () => {
    log("info", "Starting acquisition\u2026");
    try {
      setActiveTab("image");
      await startAcq(
        connectionState.kind === "connected" ? loadPacketSize(connectionState.device_id) : null,
      );
      log("success", "Acquisition started");
      addToast("success", "Acquisition started");
    } catch (e) {
      log("error", `Acquisition start failed: ${String(e)}`);
      addToast("error", `Acquisition start failed: ${String(e)}`);
    }
  }, [startAcq, connectionState, log, addToast]);

  const handleStopAcquisition = useCallback(async () => {
    log("info", "Stopping acquisition\u2026");
    try {
      await stopAcq();
      log("success", "Acquisition stopped");
      addToast("info", "Acquisition stopped");
    } catch (e) {
      log("error", `Acquisition stop failed: ${String(e)}`);
      addToast("error", `Acquisition stop failed: ${String(e)}`);
    }
  }, [stopAcq, log, addToast]);

  return (
    <FeatureStoreProvider connected={isConnected}>
    <div className="app-layout">
      <header className="app-header">
        {/* Left: brand wordmark + backend mode (reference info, not an action) */}
        <div className="app-header__brand">
          <h1 className="app-header__brand-name">Viva Studio</h1>
          {backendStatus && (() => {
            const chip = formatBackendChip(backendStatus);
            return (
              <span
                className={`app-header__mode-chip app-header__mode-chip--${chip.state}`}
                title={chip.title}
              >
                {chip.label}
              </span>
            );
          })()}
        </div>

        {/* Center: tab navigation */}
        <nav className="app-header__tabs">
          <button
            type="button"
            className={
              activeTab === "features"
                ? "app-header__tab app-header__tab--active"
                : "app-header__tab"
            }
            onClick={() => setActiveTab("features")}
          >
            Features
          </button>
          <button
            type="button"
            className={
              activeTab === "image"
                ? "app-header__tab app-header__tab--active"
                : "app-header__tab"
            }
            onClick={() => setActiveTab("image")}
          >
            Image
          </button>
          <button
            type="button"
            className={
              activeTab === "diagnostics"
                ? "app-header__tab app-header__tab--active"
                : "app-header__tab"
            }
            onClick={() => setActiveTab("diagnostics")}
          >
            Diagnostics
          </button>
        </nav>

        {/* Right: acquisition + device dropdown */}
        <div className="app-header__actions">
          {isConnected && !acqStatus.active && (
            <button type="button" className="btn btn--sm" onClick={handleStartAcquisition}>
              Start Acq
            </button>
          )}
          {acqStatus.active && (
            <div className="app-header__acq-indicator">
              <span className="app-header__acq-dot" />
              <span
                className={acquisition.dropping ? "app-header__acq-text--dropping" : undefined}
                title={acquisition.title}
              >
                {acquisition.text}
              </span>
            </div>
          )}
          {isConnected && acqStatus.active && (
            <button
              type="button"
              className="btn--stop-sm"
              onClick={handleStopAcquisition}
            >
              Stop
            </button>
          )}
          {isTauri() && (
            <DeviceDropdown
              devices={devices}
              connectionState={connectionState}
              disconnectReason={disconnectReason}
              onConnect={handleConnect}
              onDisconnect={handleDisconnect}
            />
          )}
        </div>
      </header>

      <ToastContainer />

      <div className="app-body">
        <main className="main-area">
          <div className="main-content">
            {/* Feature Browser stays mounted so tree state is preserved between tabs */}
            <div style={{ display: activeTab === "features" ? "contents" : "none" }}>
              <FeatureBrowserPage
                externalModel={externalModel}
                isConnected={isConnected}
                isActive={activeTab === "features"}
              />
            </div>
            {/* The viewer stays mounted too, keeping its stream and zoom */}
            <div style={{ display: activeTab === "image" ? "contents" : "none" }}>
              <ImageViewer
                streamerInfo={streamerInfo}
                isConnected={isConnected}
                isAcquiring={acqStatus.active}
                externalModel={externalModel}
                deviceId={connectedId}
                deviceName={connectedDeviceName ?? undefined}
                cameraModel={connectedModel ?? undefined}
                imageMeta={imageMeta}
                receivedFps={acqStatus.fps}
                stats={acqStats}
                transport={transport}
                isActive={activeTab === "image"}
              />
            </div>
            {activeTab === "diagnostics" && <DiagnosticsTab connected={isConnected} />}
          </div>
        </main>
      </div>
    </div>
    </FeatureStoreProvider>
  );
}

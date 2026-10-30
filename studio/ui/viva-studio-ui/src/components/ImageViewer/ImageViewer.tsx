import { useState, useRef, useEffect, useCallback } from "react";
import type { StreamerInfo, ImageMeta, StreamCounters, FrameMetadata } from "../../device/types";
import type { StreamTransport } from "../../device/acquisitionStatus";
import type { ParseXmlResponse } from "../../xml_model/uigraph";
import type { ZoomPanState } from "./useZoomPan";
import { ViewerToolbar } from "./ViewerToolbar";
import { ViewerCanvas } from "./ViewerCanvas";
import type { PixelHoverInfo, StreamInfoFrame } from "./ViewerCanvas";
import { ViewerStatusBar } from "./ViewerStatusBar";
import { ControlSidebar } from "./ControlSidebar";
import { AnalysisPanel } from "./AnalysisPanel";
import { useViewerLayout } from "./useViewerLayout";
import { suggestFilename } from "./snapshotUtils";
import { captureNativeFrame, type NativeFrame } from "./nativeFrame";
import { saveSnapshot } from "./snapshotSave";
import type { ImageRect } from "./roiUtils";
import type { LineSegment } from "./lineProfileUtils";
import { useFeatureWriter } from "../../device/FeatureStoreContext";

interface ImageViewerProps {
  streamerInfo: StreamerInfo | null;
  isConnected: boolean;
  isAcquiring: boolean;
  externalModel: ParseXmlResponse | null;
  deviceId: string | null;
  deviceName?: string;
  cameraModel?: string;
  imageMeta: ImageMeta | null;
  /** Frames a second arriving from the camera, from the last status. */
  receivedFps: number | null;
  /** The stream's counters: this acquisition's, or the last one's. */
  stats: StreamCounters | null;
  /** Which counters the stream fills. */
  transport: StreamTransport;
  /** False while another tab is shown; the viewer stays mounted. */
  isActive?: boolean;
}

export function ImageViewer({
  streamerInfo,
  isConnected,
  isAcquiring,
  externalModel,
  deviceId,
  deviceName,
  cameraModel,
  imageMeta,
  receivedFps,
  stats,
  transport,
  isActive = true,
}: ImageViewerProps) {
  const writeNode = useFeatureWriter();
  const [frameMetadata, setFrameMetadata] = useState<FrameMetadata | null>(null);
  const [fps, setFps] = useState<number>(0);
  const [frameCount, setFrameCount] = useState<number>(0);
  const [zoomLabel, setZoomLabel] = useState<string>("Fit");
  const [pixelHover, setPixelHover] = useState<PixelHoverInfo | null>(null);
  const [isSnapshotBusy, setIsSnapshotBusy] = useState<boolean>(false);
  const [showHistogram, setShowHistogram] = useState<boolean>(false);
  const [showRoiTool, setShowRoiTool] = useState<boolean>(false);
  const [roiRect, setRoiRect] = useState<ImageRect | null>(null);
  const [showLineTool, setShowLineTool] = useState<boolean>(false);
  const [lineSegment, setLineSegment] = useState<LineSegment | null>(null);
  const [wsStreamInfo, setWsStreamInfo] = useState<{
    pixel_format: string;
    width: number;
    height: number;
  } | null>(null);
  const snapshotRef = useRef<Uint8Array | null>(null);
  const [nativeFrame, setNativeFrame] = useState<NativeFrame | null>(null);
  const [renderedNativeFrame, setRenderedNativeFrame] = useState<NativeFrame | null>(null);
  const [captureError, setCaptureError] = useState<string | null>(null);
  const captureAbort = useRef<AbortController | null>(null);
  const [captureBusy, setCaptureBusy] = useState(false);
  const { sidebarCollapsed, toggleSidebar } = useViewerLayout();
  const resetZoomRef = useRef<(() => void) | null>(null);
  const zoomTo100Ref = useRef<(() => void) | null>(null);

  // Follow the acquisition and the stream while rendering, not in effects
  // that would render twice: a new acquisition counts from zero, a stopped
  // one shows no rate and no hovered pixel, and a closed stream forgets its
  // format.
  const [prevAcquiring, setPrevAcquiring] = useState(isAcquiring);
  if (isAcquiring !== prevAcquiring) {
    setPrevAcquiring(isAcquiring);
    if (isAcquiring) {
      setFrameCount(0);
    } else {
      setFps(0);
      setPixelHover(null);
    }
  }
  const [prevStreamerInfo, setPrevStreamerInfo] = useState(streamerInfo);
  const [prevDeviceId, setPrevDeviceId] = useState(deviceId);
  if (streamerInfo !== prevStreamerInfo || deviceId !== prevDeviceId) {
    setPrevDeviceId(deviceId);
    setPrevStreamerInfo(streamerInfo);
    setFrameMetadata(null);
    if (!streamerInfo) setWsStreamInfo(null);
    setNativeFrame(null);
    setRenderedNativeFrame(null);
    setCaptureError(null);
  }
  useEffect(() => {
    if (!streamerInfo) snapshotRef.current = null;
    return () => { captureAbort.current?.abort(); };
  }, [streamerInfo, deviceId]);

  // Turning a tool off clears what it drew.
  const setRoiTool = (on: boolean) => {
    setShowRoiTool(on);
    if (!on) setRoiRect(null);
  };
  const setLineTool = (on: boolean) => {
    setShowLineTool(on);
    if (!on) setLineSegment(null);
  };

  const handleZoomPanChange = useCallback((s: ZoomPanState) => {
    setZoomLabel(s.zoomLabel);
  }, []);

  const handleFrame = useCallback(() => {
    setFrameCount((count) => count + 1);
  }, []);

  const handleResetZoom = useCallback(() => {
    resetZoomRef.current?.();
  }, []);

  const handleZoomTo100 = useCallback(() => {
    zoomTo100Ref.current?.();
  }, []);

  const handleStreamInfoChange = useCallback((info: StreamInfoFrame) => {
    setWsStreamInfo({
      pixel_format: info.pixel_format,
      width: info.width,
      height: info.height,
    });
  }, []);

  const pixelFormat = nativeFrame?.pixelFormat ?? imageMeta?.pixel_format ?? wsStreamInfo?.pixel_format ?? "Mono8";
  const readyNativeFrame = renderedNativeFrame === nativeFrame ? nativeFrame : null;
  const handleNativePreviewReady = useCallback((frame: NativeFrame | null, error?: string) => {
    setRenderedNativeFrame(frame);
    if (error) setCaptureError(`Frozen preview could not be drawn: ${error}`);
  }, []);

  // What arrived from the camera, and apart from it what the viewer drew.
  const statistics = {
    active: isAcquiring,
    fps: receivedFps,
    stats,
    transport,
    displayed: { frames: frameCount, fps: nativeFrame ? 0 : fps },
  };

  const handleSnapshot = useCallback(async () => {
    if (isSnapshotBusy || captureBusy || !streamerInfo) return;
    setIsSnapshotBusy(true);
    const controller = new AbortController();
    captureAbort.current = controller;
    try {
      const frame = nativeFrame ?? await captureNativeFrame(streamerInfo.ws_url, controller.signal);
      if (controller.signal.aborted) return;
      const blob = new Blob([frame.png], { type: "image/png" });
      const filename = suggestFilename(frame.pixelFormat, frame.width, frame.height);
      await saveSnapshot(blob, filename);
    } catch (err) {
      if (!controller.signal.aborted) setCaptureError(`Snapshot failed: ${String(err)}`);
    } finally {
      setIsSnapshotBusy(false);
    }
  }, [isSnapshotBusy, captureBusy, nativeFrame, streamerInfo]);

  const freeze = async () => {
    if (nativeFrame) { setFrameMetadata(null); setNativeFrame(null); setRenderedNativeFrame(null); setPixelHover(null); return; }
    if (captureBusy || isSnapshotBusy || !streamerInfo) return;
    const controller = new AbortController();
    captureAbort.current = controller;
    setCaptureBusy(true);
    setCaptureError(null);
    try {
      const frame = await captureNativeFrame(streamerInfo.ws_url, controller.signal);
      if (!controller.signal.aborted) { setRenderedNativeFrame(null); setNativeFrame(frame); setPixelHover(null); }
    } catch (err) {
      if (!controller.signal.aborted) setCaptureError(`Native capture failed: ${String(err)}`);
    } finally { setCaptureBusy(false); }
  };

  const handleApplyRoi = useCallback(async () => {
    if (!roiRect) return;
    // Write Width and Height before Offsets to avoid transient invalid states.
    // Stop at the first refusal (the writer has said why) and keep the
    // selection, so the user can adjust it and try again.
    const writes: Array<[string, number]> = [
      ["Width", roiRect.w],
      ["Height", roiRect.h],
      ["OffsetX", roiRect.x],
      ["OffsetY", roiRect.y],
    ];
    for (const [name, value] of writes) {
      if (!(await writeNode(name, value))) return;
    }
    setRoiRect(null);
    setShowRoiTool(false);
  }, [roiRect, writeNode]);

  if (!streamerInfo) {
    return (
      <div className="image-viewer-v2">
        <div className="iv-canvas-column">
          <ViewerToolbar deviceName={deviceName} onResetZoom={handleResetZoom} />
          <div className="iv-canvas-wrap iv-canvas-wrap--idle">
            <p>Start acquisition to view the live image stream.</p>
          </div>
        </div>
        <ControlSidebar
          collapsed={sidebarCollapsed}
          onToggle={toggleSidebar}
          isConnected={isConnected}
          isAcquiring={isAcquiring}
          externalModel={externalModel}
          cameraModel={cameraModel ?? null}
          deviceId={deviceId}
          statistics={statistics}
          frameMetadata={nativeFrame ? readyNativeFrame?.metadata ?? null : frameMetadata}
          frozen={nativeFrame !== null}
        />
      </div>
    );
  }

  return (
    <div className="image-viewer-v2">
      <div className="iv-canvas-column">
        <ViewerToolbar
          zoomLabel={zoomLabel}
          deviceName={deviceName}
          onResetZoom={handleResetZoom}
          onZoomTo100={handleZoomTo100}
          onSnapshot={isSnapshotBusy || captureBusy ? undefined : handleSnapshot}
          frozen={nativeFrame !== null}
          onToggleFreeze={freeze}
          freezeBusy={captureBusy || isSnapshotBusy}
          showHistogram={showHistogram}
          onToggleHistogram={() => setShowHistogram((v) => !v)}
          showRoiTool={showRoiTool}
          onToggleRoiTool={() => {
            setRoiTool(!showRoiTool);
            setLineTool(false);
          }}
          roiRect={roiRect}
          onApplyRoi={isConnected ? handleApplyRoi : undefined}
          showLineTool={showLineTool}
          onToggleLineTool={() => {
            setLineTool(!showLineTool);
            setRoiTool(false);
          }}
        />
        {captureError && <div role="alert">{captureError}</div>}
        <div className="iv-statusbar">
          {nativeFrame
            ? readyNativeFrame
              ? `Frozen · ${nativeFrame.bits}-bit ${nativeFrame.mosaic ? "raw Bayer" : nativeFrame.channels === 1 ? "mono" : "RGB"} · Resume returns to live preview`
              : captureError ? "Frozen preview unavailable · Resume to try again" : "Preparing frozen preview…"
            : "Live preview · Freeze for native pixel values, histogram and line profile"}
        </div>
        <ViewerCanvas
          key={`${deviceId}:${streamerInfo.ws_url}`}
          streamKey={deviceId ?? undefined}
          active={isActive}
          nativeFrame={nativeFrame}
          onNativePreviewReady={handleNativePreviewReady}
          wsUrl={streamerInfo.ws_url}
          onFrameStats={setFps}
          onFrame={handleFrame}
          onFrameMetadata={setFrameMetadata}
          onZoomPanChange={handleZoomPanChange}
          pixelFormat={pixelFormat}
          onPixelHover={setPixelHover}
          resetZoomRef={resetZoomRef}
          isStreaming={isAcquiring}
          snapshotRef={snapshotRef}
          onStreamInfoChange={handleStreamInfoChange}
          zoomTo100Ref={zoomTo100Ref}
          roiMode={showRoiTool}
          onRoiSelect={setRoiRect}
          lineMode={showLineTool}
          onLineSelect={setLineSegment}
        />
        <AnalysisPanel
          showHistogram={showHistogram}
          showLineTool={showLineTool}
          lineSegment={lineSegment}
          frameRef={snapshotRef}
          nativeFrame={readyNativeFrame}
          pending={nativeFrame !== null && readyNativeFrame === null}
        />
        <ViewerStatusBar
          fps={nativeFrame ? 0 : fps}
          width={nativeFrame?.width ?? imageMeta?.width ?? wsStreamInfo?.width ?? streamerInfo.width}
          height={nativeFrame?.height ?? imageMeta?.height ?? wsStreamInfo?.height ?? streamerInfo.height}
          pixelFormat={pixelFormat}
          pixelInfo={
            pixelHover
              ? { coords: pixelHover.coords, formatted: pixelHover.formatted }
              : null
          }
        />
      </div>
      <ControlSidebar
        collapsed={sidebarCollapsed}
        onToggle={toggleSidebar}
        isConnected={isConnected}
        isAcquiring={isAcquiring}
        externalModel={externalModel}
        cameraModel={cameraModel ?? null}
        deviceId={deviceId}
        statistics={statistics}
        frameMetadata={nativeFrame ? readyNativeFrame?.metadata ?? null : frameMetadata}
        frozen={nativeFrame !== null}
      />
    </div>
  );
}

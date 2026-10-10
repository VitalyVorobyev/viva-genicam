import { useState, useRef, useEffect, useCallback } from "react";
import type { StreamerInfo, NodeValueEntry, ImageMeta } from "../../device/types";
import type { ParseXmlResponse, EnumEntry } from "../../xml_model/uigraph";
import type { ZoomPanState } from "./useZoomPan";
import { ViewerToolbar } from "./ViewerToolbar";
import { ViewerCanvas } from "./ViewerCanvas";
import type { PixelHoverInfo, StreamInfoFrame } from "./ViewerCanvas";
import { ViewerStatusBar } from "./ViewerStatusBar";
import { ControlSidebar } from "./ControlSidebar";
import { AnalysisPanel } from "./AnalysisPanel";
import { useViewerLayout } from "./useViewerLayout";
import { snapshotImageData, encodeImageDataToPng, suggestFilename } from "./snapshotUtils";
import { saveSnapshot } from "./snapshotSave";
import { isTauri } from "../../tauri";
import type { ImageRect } from "./roiUtils";
import type { LineSegment } from "./lineProfileUtils";

interface ImageViewerProps {
  streamerInfo: StreamerInfo | null;
  isConnected: boolean;
  isAcquiring: boolean;
  onStartAcq: () => Promise<void>;
  onStopAcq: () => Promise<void>;
  externalModel: ParseXmlResponse | null;
  liveValues: Map<string, NodeValueEntry>;
  deviceName?: string;
  cameraModel?: string;
  imageMeta: ImageMeta | null;
  isRecording?: boolean;
  onToggleRecording?: () => void;
  recordingFrameCount?: number;
  recordingElapsed?: number;
}

async function writeNode(nodeName: string, value: number): Promise<void> {
  if (!isTauri()) return;
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("write_node", { nodeName, value });
  } catch (err) {
    console.error(`write_node(${nodeName}) failed:`, err);
  }
}

export function ImageViewer({
  streamerInfo,
  isConnected,
  isAcquiring,
  onStartAcq,
  onStopAcq,
  externalModel,
  liveValues,
  deviceName,
  cameraModel,
  imageMeta,
  isRecording,
  onToggleRecording,
  recordingFrameCount,
  recordingElapsed,
}: ImageViewerProps) {
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
  const prevAcquiring = useRef<boolean>(false);
  const snapshotRef = useRef<Uint8Array | null>(null);
  const { sidebarCollapsed, toggleSidebar } = useViewerLayout();
  const resetZoomRef = useRef<(() => void) | null>(null);
  const zoomTo100Ref = useRef<(() => void) | null>(null);

  // Reset frame counter each time acquisition transitions false → true
  useEffect(() => {
    if (isAcquiring && !prevAcquiring.current) {
      setFrameCount(0);
    }
    prevAcquiring.current = isAcquiring;
  }, [isAcquiring]);

  useEffect(() => {
    if (!isAcquiring) {
      setFps(0);
      setPixelHover(null);
    }
  }, [isAcquiring]);

  useEffect(() => {
    if (!streamerInfo) {
      setWsStreamInfo(null);
      snapshotRef.current = null;
    }
  }, [streamerInfo]);

  // Clear ROI when tool is turned off
  useEffect(() => {
    if (!showRoiTool) {
      setRoiRect(null);
    }
  }, [showRoiTool]);

  // Clear line segment when line tool is turned off
  useEffect(() => {
    if (!showLineTool) {
      setLineSegment(null);
    }
  }, [showLineTool]);

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

  const pixelFormat = imageMeta?.pixel_format ?? wsStreamInfo?.pixel_format ?? "Mono8";

  const handleSnapshot = useCallback(async () => {
    if (isSnapshotBusy) return;
    const bytes = snapshotRef.current;
    if (!bytes) return;

    const width = imageMeta?.width ?? wsStreamInfo?.width ?? streamerInfo?.width ?? 0;
    const height = imageMeta?.height ?? wsStreamInfo?.height ?? streamerInfo?.height ?? 0;
    const imageData = snapshotImageData(bytes, { width, height, pixelFormat });
    if (!imageData) {
      // Unsupported format or truncated buffer — silent no-op
      return;
    }

    setIsSnapshotBusy(true);
    try {
      const blob = await encodeImageDataToPng(imageData);
      if (!blob) return;
      const filename = suggestFilename(pixelFormat, width, height);
      await saveSnapshot(blob, filename);
    } catch (err) {
      console.error("Snapshot save failed:", err);
    } finally {
      setIsSnapshotBusy(false);
    }
  }, [isSnapshotBusy, imageMeta, wsStreamInfo, streamerInfo, pixelFormat]);

  const handleApplyRoi = useCallback(async () => {
    if (!roiRect) return;
    // Write Width and Height before Offsets to avoid transient invalid states
    await writeNode("Width", roiRect.w);
    await writeNode("Height", roiRect.h);
    await writeNode("OffsetX", roiRect.x);
    await writeNode("OffsetY", roiRect.y);
    // Clear selection after applying
    setRoiRect(null);
    setShowRoiTool(false);
  }, [roiRect]);

  const acquisitionModeEntries: EnumEntry[] =
    externalModel?.graph.nodes_by_name["AcquisitionMode"]?.enum_entries ?? [];

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
          frameCount={frameCount}
          onStartAcq={onStartAcq}
          onStopAcq={onStopAcq}
          acquisitionModeEntries={acquisitionModeEntries}
          liveValues={liveValues}
          externalModel={externalModel}
          cameraModel={cameraModel ?? null}
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
          onSnapshot={isSnapshotBusy ? undefined : handleSnapshot}
          showHistogram={showHistogram}
          onToggleHistogram={() => setShowHistogram((v) => !v)}
          showRoiTool={showRoiTool}
          onToggleRoiTool={() => {
            setShowRoiTool((v) => !v);
            setShowLineTool(false);
          }}
          roiRect={roiRect}
          onApplyRoi={isConnected ? handleApplyRoi : undefined}
          showLineTool={showLineTool}
          onToggleLineTool={() => {
            setShowLineTool((v) => !v);
            setShowRoiTool(false);
          }}
          isRecording={isRecording}
          onToggleRecording={onToggleRecording}
          recordingFrameCount={recordingFrameCount}
          recordingElapsed={recordingElapsed}
        />
        <ViewerCanvas
          wsUrl={streamerInfo.ws_url}
          onFrameStats={setFps}
          onFrame={handleFrame}
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
        />
        <ViewerStatusBar
          fps={fps}
          width={imageMeta?.width ?? wsStreamInfo?.width ?? streamerInfo.width}
          height={imageMeta?.height ?? wsStreamInfo?.height ?? streamerInfo.height}
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
        frameCount={frameCount}
        onStartAcq={onStartAcq}
        onStopAcq={onStopAcq}
        acquisitionModeEntries={acquisitionModeEntries}
        liveValues={liveValues}
        externalModel={externalModel}
        cameraModel={cameraModel ?? null}
      />
    </div>
  );
}

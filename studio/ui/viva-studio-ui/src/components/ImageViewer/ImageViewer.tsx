import { useState, useRef, useEffect, useCallback } from "react";
import type { StreamerInfo, ImageMeta } from "../../device/types";
import type { ParseXmlResponse } from "../../xml_model/uigraph";
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
}: ImageViewerProps) {
  const writeNode = useFeatureWriter();
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
  if (streamerInfo !== prevStreamerInfo) {
    setPrevStreamerInfo(streamerInfo);
    if (!streamerInfo) setWsStreamInfo(null);
  }
  useEffect(() => {
    if (!streamerInfo) snapshotRef.current = null;
  }, [streamerInfo]);

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

  const pixelFormat = imageMeta?.pixel_format ?? wsStreamInfo?.pixel_format ?? "Mono8";

  const handleSnapshot = useCallback(async () => {
    if (isSnapshotBusy) return;
    const bytes = snapshotRef.current;
    if (!bytes) return;

    const imageData = snapshotImageData(bytes);
    if (!imageData) {
      // Not a BMP the decoder understands — nothing to save.
      return;
    }

    setIsSnapshotBusy(true);
    try {
      const blob = await encodeImageDataToPng(imageData);
      if (!blob) return;
      const filename = suggestFilename(pixelFormat, imageData.width, imageData.height);
      await saveSnapshot(blob, filename);
    } catch (err) {
      console.error("Snapshot save failed:", err);
    } finally {
      setIsSnapshotBusy(false);
    }
  }, [isSnapshotBusy, pixelFormat]);

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
          frameCount={frameCount}
          externalModel={externalModel}
          cameraModel={cameraModel ?? null}
          deviceId={deviceId}
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
        externalModel={externalModel}
        cameraModel={cameraModel ?? null}
        deviceId={deviceId}
      />
    </div>
  );
}

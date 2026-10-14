import { formatFps, formatFrameCount, formatResolution } from "./viewerUtils";
import type { ImageCoords } from "./viewerUtils";

interface ViewerStatusBarProps {
  fps: number;
  /** Frames received in this acquisition; not shown before the first. */
  frameCount?: number;
  width: number;
  height: number;
  pixelFormat: string;
  pixelInfo?: { coords: ImageCoords; formatted: string } | null;
  fpsWarnBelow?: number;
}

export function ViewerStatusBar({
  fps,
  frameCount = 0,
  width,
  height,
  pixelFormat,
  pixelInfo,
  fpsWarnBelow = 10,
}: ViewerStatusBarProps) {
  const resolution = formatResolution(width, height);
  const fpsText = formatFps(fps);
  const fpsWarn = fps > 0 && fps < fpsWarnBelow;

  return (
    <div className="iv-statusbar">
      <span>{resolution}</span>
      {pixelFormat !== "" && (
        <>
          <span className="iv-statusbar__sep">·</span>
          <span>{pixelFormat}</span>
        </>
      )}
      <span className="iv-statusbar__sep">·</span>
      <span className={fpsWarn ? "iv-statusbar__fps--warn" : undefined}>{fpsText}</span>
      {frameCount > 0 && (
        <>
          <span className="iv-statusbar__sep">·</span>
          <span>{formatFrameCount(frameCount)}</span>
        </>
      )}
      {pixelInfo && (
        <>
          <span className="iv-statusbar__sep">·</span>
          <span>
            ({pixelInfo.coords.x}, {pixelInfo.coords.y}) · {pixelInfo.formatted}
          </span>
        </>
      )}
    </div>
  );
}

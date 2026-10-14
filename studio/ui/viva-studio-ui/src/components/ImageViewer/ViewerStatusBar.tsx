import { PREVIEW_LIMIT, formatFps, formatResolution } from "./viewerUtils";
import type { ImageCoords } from "./viewerUtils";

interface ViewerStatusBarProps {
  /** Frames a second the viewer draws, not the camera's rate. */
  fps: number;
  width: number;
  height: number;
  pixelFormat: string;
  pixelInfo?: { coords: ImageCoords; formatted: string } | null;
  fpsWarnBelow?: number;
}

export function ViewerStatusBar({
  fps,
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
      <span
        className={fpsWarn ? "iv-statusbar__fps--warn" : undefined}
        title={`Frames a second the viewer draws. ${PREVIEW_LIMIT}; every received frame is counted in Statistics`}
      >
        display {fpsText}
      </span>
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

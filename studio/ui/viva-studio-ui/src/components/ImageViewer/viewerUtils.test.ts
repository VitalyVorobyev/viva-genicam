import { describe, it, expect } from "vitest";
import {
  canvasLayout,
  observedBoxSize,
  fitContain,
  imageRendering,
  formatFps,
  formatResolution,
  formatFrameCount,
  mouseToImageCoords,
  clampedPixel,
  imageToWrap,
  wrapToImage,
} from "./viewerUtils";

describe("fitContain", () => {
  it("test_fitContain_landscape_smaller_than_container", () => {
    // Image 640×480 inside 1280×960 box — scale capped at 1.0, no upscale
    const result = fitContain(640, 480, 1280, 960);
    expect(result.width).toBe(640);
    expect(result.height).toBe(480);
    expect(result.left).toBe(320);
    expect(result.top).toBe(240);
  });

  it("test_fitContain_landscape_larger_container_constrained_by_width", () => {
    // Image 1920×1080, box 960×1080 — width is the constraint
    const result = fitContain(1920, 1080, 960, 1080);
    expect(result.width).toBe(960);
    expect(result.height).toBe(540);
    expect(result.left).toBe(0);
    expect(result.top).toBe(270);
  });

  it("test_fitContain_landscape_larger_container_constrained_by_height", () => {
    // Image 1920×1080, box 1920×540 — height is the constraint
    const result = fitContain(1920, 1080, 1920, 540);
    expect(result.width).toBe(960);
    expect(result.height).toBe(540);
    expect(result.left).toBe(480);
    expect(result.top).toBe(0);
  });

  it("test_fitContain_square_image", () => {
    // Image 512×512, box 512×512 — scale exactly 1.0
    const result = fitContain(512, 512, 512, 512);
    expect(result.width).toBe(512);
    expect(result.height).toBe(512);
    expect(result.left).toBe(0);
    expect(result.top).toBe(0);
  });

  it("test_fitContain_zero_container", () => {
    // Container is 0×0 — must not divide by zero, returns zeroed result
    const result = fitContain(640, 480, 0, 0);
    expect(result.width).toBe(0);
    expect(result.height).toBe(0);
    expect(result.left).toBe(0);
    expect(result.top).toBe(0);
  });
});

describe("formatFps", () => {
  it("test_formatFps_typical", () => {
    expect(formatFps(29.97)).toBe("29.97 fps");
  });

  it("test_formatFps_integer", () => {
    expect(formatFps(30)).toBe("30.00 fps");
  });

  it("test_formatFps_zero", () => {
    expect(formatFps(0)).toBe("0.00 fps");
  });
});

describe("formatResolution", () => {
  it("test_formatResolution_standard", () => {
    expect(formatResolution(1920, 1080)).toBe("1920 \u00d7 1080");
  });

  it("test_formatResolution_square", () => {
    expect(formatResolution(512, 512)).toBe("512 \u00d7 512");
  });
});

describe("formatFrameCount", () => {
  it("test_formatFrameCount_zero", () => {
    expect(formatFrameCount(0)).toBe("0 frames");
  });

  it("test_formatFrameCount_one", () => {
    expect(formatFrameCount(1)).toBe("1 frame");
  });

  it("test_formatFrameCount_many", () => {
    expect(formatFrameCount(42)).toBe("42 frames");
  });

  it("test_formatFrameCount_large", () => {
    expect(formatFrameCount(1234)).toBe("1,234 frames");
  });
});

describe("mouseToImageCoords", () => {
  // Base params: 100×100 image, 100×100 box, fitScale=1, scale=1, no pan
  const base = {
    boxW: 100,
    boxH: 100,
    imgW: 100,
    imgH: 100,
    scale: 1,
    fitScale: 1,
    panX: 0,
    panY: 0,
  };

  it("test_mouseToImageCoords_center_no_pan_no_zoom", () => {
    // Mouse at wrap center → center pixel (50, 50)
    const result = mouseToImageCoords({ ...base, mouseX: 50, mouseY: 50 });
    expect(result).toEqual({ x: 50, y: 50 });
  });

  it("test_mouseToImageCoords_top_left_corner", () => {
    // Mouse at top-left of wrap → pixel (0, 0)
    const result = mouseToImageCoords({ ...base, mouseX: 0, mouseY: 0 });
    expect(result).toEqual({ x: 0, y: 0 });
  });

  it("test_mouseToImageCoords_out_of_bounds_returns_null", () => {
    // Mouse outside the image (negative offset from center, past left edge)
    const result = mouseToImageCoords({ ...base, mouseX: -10, mouseY: 50 });
    expect(result).toBeNull();
  });

  it("test_mouseToImageCoords_fit_scaled_image", () => {
    // 200×200 image fit into 100×100 box → fitScale=0.5
    // Mouse at center (50,50) of box → center pixel (100,100)
    const result = mouseToImageCoords({
      boxW: 100,
      boxH: 100,
      imgW: 200,
      imgH: 200,
      scale: 0.5,
      fitScale: 0.5,
      panX: 0,
      panY: 0,
      mouseX: 50,
      mouseY: 50,
    });
    expect(result).toEqual({ x: 100, y: 100 });
  });

  it("test_mouseToImageCoords_zoomed_in", () => {
    // Zoomed to 2× with no pan; center mouse still gives center pixel
    const result = mouseToImageCoords({
      ...base,
      scale: 2,
      fitScale: 1,
      mouseX: 50,
      mouseY: 50,
    });
    expect(result).toEqual({ x: 50, y: 50 });
  });

  it("test_mouseToImageCoords_zero_fitScale_returns_null", () => {
    // fitScale=0 must not divide by zero, returns null
    const result = mouseToImageCoords({
      ...base,
      fitScale: 0,
      mouseX: 50,
      mouseY: 50,
    });
    expect(result).toBeNull();
  });

  it("test_mouseToImageCoords_panX_shifts_result", () => {
    // panX=10 shifts canvas right by 10px → for the same wrap-relative x,
    // the image x coordinate decreases by 10 pixels.
    const withPan = mouseToImageCoords({
      ...base,
      panX: 10,
      mouseX: 50,
      mouseY: 50,
    });
    const noPan = mouseToImageCoords({
      ...base,
      panX: 0,
      mouseX: 50,
      mouseY: 50,
    });
    expect(withPan).not.toBeNull();
    expect(noPan).not.toBeNull();
    expect(withPan!.x).toBe(noPan!.x - 10);
  });
});

describe("clampedPixel", () => {
  const view = {
    boxW: 640,
    boxH: 480,
    imgW: 640,
    imgH: 480,
    scale: 1,
    fitScale: 1,
    panX: 0,
    panY: 0,
  };
  const at = (x: number, y: number) => clampedPixel(wrapToImage(view, { x, y })!, 640, 480);

  it("takes the pixel under an in-bounds point", () => {
    expect(at(320, 240)).toEqual({ x: 320, y: 240 });
    expect(at(0, 0)).toEqual({ x: 0, y: 0 });
  });

  it("clamps a point past the edge to the last pixel", () => {
    expect(at(9999, 240)).toEqual({ x: 639, y: 240 });
    expect(at(-5, -5)).toEqual({ x: 0, y: 0 });
  });
});

// "1:1" drew a copy downscaled to the fit size and enlarged it, so it never
// showed the camera's pixels (backlog ST-39).
describe("canvasLayout", () => {
  it("keeps every camera pixel and shows the canvas at the fit size", () => {
    const layout = canvasLayout(2448, 2048, 800, 600);
    expect(layout.backing).toEqual({ width: 2448, height: 2048 });
    expect(layout.css.height).toBe(600);
    expect(layout.css.width).toBeCloseTo(717.1875, 6);
  });

  // The zoom transform multiplies the fit size by scale / fitScale. A rounded
  // fit size (717 for 717.19) made 1:1 come out at 2447.4 CSS pixels for a
  // 2448-pixel image, which the browser resamples (Codex review of #178).
  it("sizes the canvas so that 1:1 is exactly the camera's width", () => {
    const { css } = canvasLayout(2448, 2048, 800, 600);
    const fitScale = Math.min(800 / 2448, 600 / 2048, 1);
    expect(css.width * (1 / fitScale)).toBeCloseTo(2448, 9);
  });

  it("shows a small image at its own size", () => {
    expect(canvasLayout(320, 240, 800, 600)).toEqual({
      backing: { width: 320, height: 240 },
      css: { width: 320, height: 240 },
    });
  });
});

describe("imageRendering", () => {
  it("draws enlarged pixels as squares and smooths a reduced image", () => {
    expect(imageRendering(2)).toBe("pixelated");
    expect(imageRendering(1)).toBe("auto");
    expect(imageRendering(0.4)).toBe("auto");
  });
});

// Switching tabs unmounted the viewer, which reconnected its WebSocket and
// reset the zoom. Kept mounted but hidden, it measures 0×0, which must not
// count as a resize, or the zoom resets anyway (backlog ST-39).
describe("observedBoxSize", () => {
  it("follows a resize", () => {
    expect(observedBoxSize({ w: 800, h: 600 }, 1024.4, 700.6)).toEqual({ w: 1024, h: 701 });
  });

  it("keeps the last size while the viewer is hidden", () => {
    const shown = { w: 800, h: 600 };
    expect(observedBoxSize(shown, 0, 0)).toBe(shown);
  });
});

// The overlays keep their geometry in image pixels and draw it through the
// view of the moment; a line kept in wrap pixels stayed on the screen while
// the image zoomed under it.
describe("imageToWrap / wrapToImage", () => {
  // 1000×500 image in a 500×500 box: the fit scale is 0.5 and the image is
  // drawn 500×250, centred.
  const fit = {
    boxW: 500,
    boxH: 500,
    imgW: 1000,
    imgH: 500,
    scale: 0.5,
    fitScale: 0.5,
    panX: 0,
    panY: 0,
  };
  // Zoomed to 200% and panned.
  const zoomed = { ...fit, scale: 2, panX: 10, panY: -20 };

  it("puts the image's corners and centre where the fit view draws them", () => {
    expect(imageToWrap(fit, { x: 0, y: 0 })).toEqual({ x: 0, y: 125 });
    expect(imageToWrap(fit, { x: 1000, y: 500 })).toEqual({ x: 500, y: 375 });
    expect(imageToWrap(fit, { x: 500, y: 250 })).toEqual({ x: 250, y: 250 });
  });

  it("moves an image point with the zoom and the pan", () => {
    // The image centre sits at the box centre plus the pan.
    expect(imageToWrap(zoomed, { x: 500, y: 250 })).toEqual({ x: 260, y: 230 });
    // Two CSS pixels per camera pixel from there.
    expect(imageToWrap(zoomed, { x: 0, y: 0 })).toEqual({ x: -740, y: -270 });
    expect(imageToWrap(zoomed, { x: 510, y: 240 })).toEqual({ x: 280, y: 210 });
  });

  it("inverts the projection at both zoom levels", () => {
    for (const view of [fit, zoomed]) {
      for (const p of [{ x: 0, y: 0 }, { x: 123.5, y: 456.25 }, { x: 1000, y: 500 }]) {
        const wrap = imageToWrap(view, p)!;
        expect(wrapToImage(view, wrap)).toEqual(p);
      }
    }
  });

  it("agrees with the hovered pixel", () => {
    const wrap = imageToWrap(zoomed, { x: 321.5, y: 87.5 })!;
    expect(mouseToImageCoords({ ...zoomed, mouseX: wrap.x, mouseY: wrap.y })).toEqual({
      x: 321,
      y: 87,
    });
  });

  it("has no answer before the first frame", () => {
    const empty = { ...fit, imgW: 0, imgH: 0 };
    expect(imageToWrap(empty, { x: 0, y: 0 })).toBeNull();
    expect(wrapToImage(empty, { x: 0, y: 0 })).toBeNull();
  });
});

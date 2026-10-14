import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { LineOverlay } from "./LineOverlay";
import { RoiOverlay } from "./RoiOverlay";

// A profile line drawn and then zoomed stayed on the same screen pixels while
// the image moved under it: the overlays kept wrap pixels. They keep image
// pixels now, and a new view moves them with the image.
describe("overlays follow the image", () => {
  // 1000×500 image in a 500×500 box, at fit (0.5) and at 200%, panned.
  const fit = { boxW: 500, boxH: 500, imgW: 1000, imgH: 500, scale: 0.5, fitScale: 0.5, panX: 0, panY: 0 };
  const zoomed = { ...fit, scale: 2, panX: 10, panY: -20 };
  const span = { from: { x: 100, y: 50 }, to: { x: 600, y: 300 } };

  it("draws a line on the same image pixels at two zoom levels", () => {
    const { container, rerender } = render(<LineOverlay line={span} view={fit} />);
    const ends = () => {
      const line = container.querySelector("line")!;
      return ["x1", "y1", "x2", "y2"].map((a) => Number(line.getAttribute(a)));
    };
    expect(ends()).toEqual([50, 150, 300, 275]);
    rerender(<LineOverlay line={span} view={zoomed} />);
    expect(ends()).toEqual([-540, -170, 460, 330]);
  });

  it("draws an ROI on the same image pixels at two zoom levels", () => {
    const { container, rerender } = render(<RoiOverlay rect={span} view={fit} />);
    const box = () => {
      const rect = container.querySelector("rect")!;
      return ["x", "y", "width", "height"].map((a) => Number(rect.getAttribute(a)));
    };
    expect(box()).toEqual([50, 150, 250, 125]);
    rerender(<RoiOverlay rect={span} view={zoomed} />);
    expect(box()).toEqual([-540, -170, 1000, 500]);
  });
});

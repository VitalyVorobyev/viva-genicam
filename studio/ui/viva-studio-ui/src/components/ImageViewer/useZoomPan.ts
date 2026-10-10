import { useReducer, useRef, useCallback, useEffect, useLayoutEffect } from "react";
import { fitContain } from "./viewerUtils";
import type { FitContainResult } from "./viewerUtils";
import {
  computeFitScale,
  clampZoom,
  clampPan,
  zoomAroundPoint,
  formatZoomLabel,
  MAX_ZOOM,
  buildTransform,
} from "./zoomPanUtils";

export interface ZoomPanState {
  scale: number;
  panX: number;
  panY: number;
  fitScale: number;
  zoomLabel: string;
  fitContainResult: FitContainResult;
}

interface InternalState {
  scale: number;
  panX: number;
  panY: number;
}

type Action =
  | { type: "SET"; scale: number; panX: number; panY: number }
  | { type: "RESET"; fitScale: number };

const WHEEL_SENSITIVITY = 0.001;

function reducer(state: InternalState, action: Action): InternalState {
  switch (action.type) {
    case "SET":
      return { scale: action.scale, panX: action.panX, panY: action.panY };
    case "RESET":
      return { scale: action.fitScale, panX: 0, panY: 0 };
    default:
      return state;
  }
}

export function useZoomPan(
  imgW: number,
  imgH: number,
  boxW: number,
  boxH: number,
) {
  const fitScale = computeFitScale(imgW, imgH, boxW, boxH);
  const fitContainResult = fitContain(imgW, imgH, boxW, boxH);

  const [internal, dispatch] = useReducer(reducer, {
    scale: fitScale,
    panX: 0,
    panY: 0,
  });

  // Refs that mirror state for use inside stale-closure-safe native event handlers
  const scaleRef = useRef(internal.scale);
  const panXRef = useRef(internal.panX);
  const panYRef = useRef(internal.panY);
  const fitScaleRef = useRef(fitScale);
  // Updated after each render, before the browser can deliver an event.
  useLayoutEffect(() => {
    scaleRef.current = internal.scale;
    panXRef.current = internal.panX;
    panYRef.current = internal.panY;
    fitScaleRef.current = fitScale;
  });

  // Reset when image dimensions change
  const prevImgSizeRef = useRef({ w: imgW, h: imgH });
  useEffect(() => {
    if (
      prevImgSizeRef.current.w !== imgW ||
      prevImgSizeRef.current.h !== imgH
    ) {
      prevImgSizeRef.current = { w: imgW, h: imgH };
      dispatch({ type: "RESET", fitScale });
    }
  }, [imgW, imgH, fitScale]);

  // Re-clamp when box size changes (but keep scale, just clamp pan)
  const prevBoxSizeRef = useRef({ w: boxW, h: boxH });
  useEffect(() => {
    if (
      prevBoxSizeRef.current.w !== boxW ||
      prevBoxSizeRef.current.h !== boxH
    ) {
      prevBoxSizeRef.current = { w: boxW, h: boxH };
      const clamped = clampPan(
        panXRef.current,
        panYRef.current,
        scaleRef.current,
        imgW,
        imgH,
        boxW,
        boxH,
      );
      dispatch({
        type: "SET",
        scale: scaleRef.current,
        panX: clamped.panX,
        panY: clamped.panY,
      });
    }
  }, [boxW, boxH, imgW, imgH]);

  // Drag state
  const isDraggingRef = useRef(false);
  const dragStartRef = useRef({ x: 0, y: 0, panX: 0, panY: 0 });

  const onPointerDown = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      if (e.button !== 0) return;
      e.currentTarget.setPointerCapture(e.pointerId);
      isDraggingRef.current = true;
      dragStartRef.current = {
        x: e.clientX,
        y: e.clientY,
        panX: panXRef.current,
        panY: panYRef.current,
      };
    },
    [],
  );

  const onPointerMove = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      if (!isDraggingRef.current) return;
      const dx = e.clientX - dragStartRef.current.x;
      const dy = e.clientY - dragStartRef.current.y;
      const newPanX = dragStartRef.current.panX + dx;
      const newPanY = dragStartRef.current.panY + dy;
      const clamped = clampPan(
        newPanX,
        newPanY,
        scaleRef.current,
        imgW,
        imgH,
        boxW,
        boxH,
      );
      dispatch({
        type: "SET",
        scale: scaleRef.current,
        panX: clamped.panX,
        panY: clamped.panY,
      });
    },
    [imgW, imgH, boxW, boxH],
  );

  const onPointerUp = useCallback(
    (_e: React.PointerEvent<HTMLDivElement>) => {
      isDraggingRef.current = false;
    },
    [],
  );

  const onDoubleClick = useCallback(() => {
    dispatch({ type: "RESET", fitScale: fitScaleRef.current });
  }, []);

  const zoomTo100 = useCallback(() => {
    const clamped = clampPan(0, 0, 1.0, imgW, imgH, boxW, boxH);
    dispatch({ type: "SET", scale: 1.0, panX: clamped.panX, panY: clamped.panY });
  }, [imgW, imgH, boxW, boxH]);

  /**
   * Returns a stable native WheelEvent handler for attaching via addEventListener.
   * Uses refs for all state access to avoid stale closures.
   */
  const getWheelHandler = useCallback(
    (wrapEl: HTMLDivElement): ((e: WheelEvent) => void) => {
      return (e: WheelEvent) => {
        e.preventDefault();

        const currentScale = scaleRef.current;
        const currentFitScale = fitScaleRef.current;

        const delta = -e.deltaY * WHEEL_SENSITIVITY;
        const nextScale = clampZoom(
          currentScale * (1 + delta),
          currentFitScale,
          MAX_ZOOM,
        );

        if (nextScale === currentScale) return;

        // Mouse position relative to centre of wrap box
        const rect = wrapEl.getBoundingClientRect();
        const mouseX = e.clientX - rect.left - rect.width / 2;
        const mouseY = e.clientY - rect.top - rect.height / 2;

        const { panX: newPanX, panY: newPanY } = zoomAroundPoint(
          currentScale,
          nextScale,
          panXRef.current,
          panYRef.current,
          mouseX,
          mouseY,
          boxW,
          boxH,
        );

        const clamped = clampPan(newPanX, newPanY, nextScale, imgW, imgH, boxW, boxH);

        scaleRef.current = nextScale;
        panXRef.current = clamped.panX;
        panYRef.current = clamped.panY;

        dispatch({
          type: "SET",
          scale: nextScale,
          panX: clamped.panX,
          panY: clamped.panY,
        });
      };
    },
    [imgW, imgH, boxW, boxH],
  );

  const state: ZoomPanState = {
    scale: internal.scale,
    panX: internal.panX,
    panY: internal.panY,
    fitScale,
    zoomLabel: formatZoomLabel(internal.scale, fitScale),
    fitContainResult,
  };

  return {
    state,
    onPointerDown,
    onPointerMove,
    onPointerUp,
    onDoubleClick,
    zoomTo100,
    getWheelHandler,
    isDraggingRef,
    buildTransform: () =>
      buildTransform(internal.panX, internal.panY, internal.scale, fitScale),
  };
}

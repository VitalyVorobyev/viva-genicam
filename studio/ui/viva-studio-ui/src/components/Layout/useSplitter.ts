import { useState, useRef, useCallback, useLayoutEffect } from "react";
import type React from "react";

/**
 * Clamp a splitter position value between min and max (inclusive).
 *
 * Extracted as a pure function to enable unit testing independent of the hook.
 */
export function clampSplitter(value: number, min: number, max: number): number {
  return Math.max(min, Math.min(max, value));
}

/** How far one arrow key moves a splitter, in pixels. */
const KEY_STEP = 16;

/**
 * The size a key press asks a focused splitter for, or `null` for a key it
 * does not handle. Left/right splitters follow the horizontal arrows; a
 * vertical one sizes the pane below it, which ArrowUp grows. The arrow
 * that moves the handle away from the sized pane grows it, so a pane right
 * of its handle (`paneAfter`) grows with ArrowLeft. Home and End jump to the
 * bounds.
 */
export function splitterKeySize(
  key: string,
  size: number,
  orientation: "horizontal" | "vertical",
  { minSize, maxSize }: { minSize: number; maxSize: number },
  paneAfter: boolean = orientation === "vertical",
): number | null {
  const [toStart, toEnd] =
    orientation === "vertical" ? ["ArrowUp", "ArrowDown"] : ["ArrowLeft", "ArrowRight"];
  const [grow, shrink] = paneAfter ? [toStart, toEnd] : [toEnd, toStart];
  switch (key) {
    case grow:
      return clampSplitter(size + KEY_STEP, minSize, maxSize);
    case shrink:
      return clampSplitter(size - KEY_STEP, minSize, maxSize);
    case "Home":
      return minSize;
    case "End":
      return maxSize;
    default:
      return null;
  }
}

function loadSize(key: string, defaultSize: number): number {
  try {
    const raw = localStorage.getItem(key);
    if (!raw) return defaultSize;
    const n = Number(raw);
    return isNaN(n) ? defaultSize : n;
  } catch {
    return defaultSize;
  }
}

function saveSize(key: string, size: number): void {
  try {
    localStorage.setItem(key, String(size));
  } catch {
    // localStorage unavailable — silently ignore
  }
}

interface SplitterOptions {
  storageKey: string;
  defaultSize: number;
  minSize: number;
  maxSize: number;
  /** "horizontal" = left/right split (default), "vertical" = top/bottom split */
  orientation?: "horizontal" | "vertical";
  /**
   * Whether the sized pane lies after the handle (right of it, or below it),
   * so that moving the handle toward the start grows the pane. Defaults to
   * a bottom pane for a vertical split and a left pane for a horizontal one.
   */
  paneAfter?: boolean;
}

interface HandleProps {
  role: "separator";
  "aria-orientation": "vertical" | "horizontal";
  "aria-valuenow": number;
  "aria-valuemin": number;
  "aria-valuemax": number;
  className: string;
  tabIndex: number;
  onPointerDown: (e: React.PointerEvent) => void;
  onKeyDown: (e: React.KeyboardEvent) => void;
}

interface SplitterResult {
  size: number;
  handleProps: HandleProps;
}

/**
 * Drag-to-resize splitter hook.
 *
 * Returns the current pane size and props to spread onto a `<div>` splitter
 * handle element. Attaches `pointermove` / `pointerup` to `document` during
 * drag so fast mouse moves don't lose the drag target. Persists the final
 * position to localStorage on pointer-up.
 *
 * @param storageKey   localStorage key for persistence.
 * @param defaultSize  Initial size when no saved value exists.
 * @param minSize      Minimum allowed size in pixels.
 * @param maxSize      Maximum allowed size in pixels.
 */
export function useSplitter({
  storageKey,
  defaultSize,
  minSize,
  maxSize,
  orientation = "horizontal",
  paneAfter = orientation === "vertical",
}: SplitterOptions): SplitterResult {
  const [size, setSize] = useState<number>(() => {
    const loaded = loadSize(storageKey, defaultSize);
    return clampSplitter(loaded, minSize, maxSize);
  });

  // Keep a ref so closures inside onPointerDown always see the latest size.
  const sizeRef = useRef(size);
  useLayoutEffect(() => {
    sizeRef.current = size;
  }, [size]);

  const dragRef = useRef({ active: false, startPos: 0, startSize: 0 });

  const onPointerDown = useCallback(
    (e: React.PointerEvent) => {
      e.preventDefault();
      dragRef.current = {
        active: true,
        startPos: orientation === "vertical" ? e.clientY : e.clientX,
        startSize: sizeRef.current,
      };

      const handleMove = (ev: PointerEvent) => {
        const currentPos = orientation === "vertical" ? ev.clientY : ev.clientX;
        const delta = currentPos - dragRef.current.startPos;
        // A pane after the handle (bottom, or right) grows as the handle
        // moves toward the start, so subtract delta.
        const next = paneAfter
          ? clampSplitter(dragRef.current.startSize - delta, minSize, maxSize)
          : clampSplitter(dragRef.current.startSize + delta, minSize, maxSize);
        setSize(next);
      };

      const handleUp = () => {
        dragRef.current.active = false;
        document.removeEventListener("pointermove", handleMove);
        document.removeEventListener("pointerup", handleUp);
        // Persist on drag end; sizeRef is current at this point.
        saveSize(storageKey, sizeRef.current);
      };

      document.addEventListener("pointermove", handleMove);
      document.addEventListener("pointerup", handleUp);
    },
    [minSize, maxSize, storageKey, orientation, paneAfter],
  );

  const onKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      const next = splitterKeySize(
        e.key,
        sizeRef.current,
        orientation,
        { minSize, maxSize },
        paneAfter,
      );
      if (next === null) return;
      e.preventDefault();
      setSize(next);
      saveSize(storageKey, next);
    },
    [minSize, maxSize, storageKey, orientation, paneAfter],
  );

  const isVertical = orientation === "vertical";

  const handleProps: HandleProps = {
    role: "separator",
    "aria-orientation": isVertical ? "horizontal" : "vertical",
    "aria-valuenow": size,
    "aria-valuemin": minSize,
    "aria-valuemax": maxSize,
    className: isVertical ? "pane-splitter pane-splitter--horizontal" : "pane-splitter",
    tabIndex: 0,
    onPointerDown,
    onKeyDown,
  };

  return { size, handleProps };
}

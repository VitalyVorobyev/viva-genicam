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
}

interface HandleProps {
  role: "separator";
  "aria-orientation": "vertical" | "horizontal";
  className: string;
  tabIndex: number;
  onPointerDown: (e: React.PointerEvent) => void;
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
        // For vertical (bottom pane): dragging up = larger, so subtract delta
        const next = orientation === "vertical"
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
    [minSize, maxSize, storageKey, orientation],
  );

  const isVertical = orientation === "vertical";

  const handleProps: HandleProps = {
    role: "separator",
    "aria-orientation": isVertical ? "horizontal" : "vertical",
    className: isVertical ? "pane-splitter pane-splitter--horizontal" : "pane-splitter",
    tabIndex: 0,
    onPointerDown,
  };

  return { size, handleProps };
}

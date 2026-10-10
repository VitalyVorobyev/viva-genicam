import { useEffect, useState } from "react";
import { isTauri } from "../tauri";
import { listenTo } from "./events";
import type { ImageMeta } from "./types";

export function useImageMeta() {
  const [imageMeta, setImageMeta] = useState<ImageMeta | null>(null);

  useEffect(() => {
    if (!isTauri()) return;

    let unlistenMeta: (() => void) | null = null;
    let unlistenConn: (() => void) | null = null;
    let cancelled = false;

    listenTo("image-meta-changed", (meta) => {
      setImageMeta(meta);
    }).then((fn) => {
      if (cancelled) fn();
      else unlistenMeta = fn;
    });

    listenTo("connection-state-changed", (state) => {
      if (state.kind === "disconnected" || state.kind === "error") {
        setImageMeta(null);
      }
    }).then((fn) => {
      if (cancelled) fn();
      else unlistenConn = fn;
    });

    return () => {
      cancelled = true;
      unlistenMeta?.();
      unlistenConn?.();
    };
  }, []);

  return { imageMeta };
}

import { useCallback, useEffect, useState } from "react";
import { isTauri } from "../tauri";
import type { FeatureState, NodeValueEntry } from "./types";

/**
 * Project a rich [`FeatureState`] down to the legacy [`NodeValueEntry`] shape
 * so existing consumers that read `value` / `access_mode` / `min` / `max` /
 * `inc` keep working during the UI migration to state-first logic.
 */
function featureStateToEntry(state: FeatureState): NodeValueEntry {
  const entry: NodeValueEntry = {
    value: state.value,
    access_mode: state.access_mode,
  };
  if (state.numeric) {
    entry.min = state.numeric.min;
    entry.max = state.numeric.max;
    if (state.numeric.inc !== undefined) entry.inc = state.numeric.inc;
  }
  return entry;
}

/**
 * Reject numeric values that are clearly nonsensical (NaN / Infinity). We do
 * not reject based on range, because the device's declared range may itself
 * be wrong (see the `viva-genapi` numeric-dispatch handoff). The goal here is
 * only to keep obviously broken payloads out of the UI cache.
 */
function sanitizeValue(value: unknown): number | string | boolean | null {
  if (typeof value === "number") {
    if (!Number.isFinite(value)) {
      return String(value); // "NaN" / "Infinity" render as text, not garbage
    }
  }
  // `null` passes through: a command or write-only node has no value.
  return value as number | string | boolean | null;
}

export function useNodeValues() {
  const [liveValues, setLiveValues] = useState<Map<string, NodeValueEntry>>(new Map());
  const [liveStates, setLiveStates] = useState<Map<string, FeatureState>>(new Map());

  useEffect(() => {
    if (!isTauri()) return;

    let unlisten: (() => void) | null = null;
    let cancelled = false;

    import("@tauri-apps/api/event").then(({ listen }) => {
      if (cancelled) return;
      listen<{
        node_name: string;
        value: number | string | boolean | null;
        access_mode: string;
        /** `FeatureState` payload added in API v2. Missing on older backends. */
        state?: FeatureState;
      }>("node-value-changed", (event) => {
        const { node_name, value, access_mode, state } = event.payload;
        const cleanValue = sanitizeValue(value);
        setLiveValues((prev) => {
          const next = new Map(prev);
          next.set(node_name, { value: cleanValue, access_mode });
          return next;
        });
        if (state) {
          const cleanState: FeatureState = {
            ...state,
            value: sanitizeValue(state.value),
          };
          setLiveStates((prev) => {
            const next = new Map(prev);
            next.set(node_name, cleanState);
            return next;
          });
        }
      }).then((fn) => {
        if (cancelled) {
          fn();
        } else {
          unlisten = fn;
        }
      });
    });

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  // Clear cache when device disconnects
  useEffect(() => {
    if (!isTauri()) return;

    let unlisten: (() => void) | null = null;
    import("@tauri-apps/api/event").then(({ listen }) => {
      listen("connection-state-changed", (e) => {
        const state = e.payload as { kind: string };
        if (state.kind === "disconnected" || state.kind === "error") {
          setLiveValues(new Map());
          setLiveStates(new Map());
        }
      }).then((fn) => {
        unlisten = fn;
      });
    });

    return () => {
      unlisten?.();
    };
  }, []);

  const seedValues = useCallback((bulk: Map<string, NodeValueEntry>) => {
    setLiveValues((prev) => {
      const next = new Map(prev);
      bulk.forEach((v, k) => next.set(k, v));
      return next;
    });
  }, []);

  /**
   * Merge a batch of [`FeatureState`]s into both the state map and the legacy
   * value map (so consumers that have not migrated yet still see the update).
   */
  const seedStates = useCallback((bulk: Map<string, FeatureState>) => {
    setLiveStates((prev) => {
      const next = new Map(prev);
      bulk.forEach((s, k) => next.set(k, s));
      return next;
    });
    setLiveValues((prev) => {
      const next = new Map(prev);
      bulk.forEach((s, k) => next.set(k, featureStateToEntry(s)));
      return next;
    });
  }, []);

  /**
   * Update the state cache for a single node after an Apply or Execute that
   * returned a fresh `FeatureState`. Keeps `liveValues` in sync automatically.
   */
  const mergeState = useCallback((name: string, state: FeatureState) => {
    const cleanState: FeatureState = { ...state, value: sanitizeValue(state.value) };
    setLiveStates((prev) => {
      const next = new Map(prev);
      next.set(name, cleanState);
      return next;
    });
    setLiveValues((prev) => {
      const next = new Map(prev);
      next.set(name, featureStateToEntry(cleanState));
      return next;
    });
  }, []);

  return { liveValues, liveStates, seedValues, seedStates, mergeState };
}

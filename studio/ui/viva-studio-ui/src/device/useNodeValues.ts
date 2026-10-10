import { useCallback, useEffect, useState } from "react";
import { isTauri } from "../tauri";
import { listenTo } from "./events";
import type { FeatureState, JsonValue, NodeValueEntry } from "./types";

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
 * Lift a legacy [`NodeValueEntry`] to the [`FeatureState`] the editors read,
 * for a node the live state map does not hold yet: the connect-time bulk read
 * fills only the legacy map (backlog ST-29). The entry does not say whether
 * the feature is implemented or available, so both are reported `true`.
 */
export function entryToFeatureState(entry: NodeValueEntry, kind: string): FeatureState {
  const state: FeatureState = {
    value: entry.value,
    access_mode: entry.access_mode,
    kind,
    is_implemented: true,
    is_available: true,
  };
  if (entry.min !== undefined && entry.max !== undefined) {
    state.numeric = { min: entry.min, max: entry.max };
    if (entry.inc !== undefined) state.numeric.inc = entry.inc;
  }
  return state;
}

/**
 * Reject numeric values that are clearly nonsensical (NaN / Infinity). We do
 * not reject based on range, because the device's declared range may itself
 * be wrong (see the `viva-genapi` numeric-dispatch handoff). The goal here is
 * only to keep obviously broken payloads out of the UI cache.
 */
function sanitizeValue(value: JsonValue): JsonValue {
  if (typeof value === "number" && !Number.isFinite(value)) {
    return String(value); // "NaN" / "Infinity" render as text, not garbage
  }
  // `null` passes through: a command or write-only node has no value.
  return value;
}

export function useNodeValues() {
  const [liveValues, setLiveValues] = useState<Map<string, NodeValueEntry>>(new Map());
  const [liveStates, setLiveStates] = useState<Map<string, FeatureState>>(new Map());

  useEffect(() => {
    if (!isTauri()) return;

    let unlisten: (() => void) | null = null;
    let cancelled = false;

    listenTo("node-value-changed", ({ node_name, value, access_mode, state }) => {
      const cleanValue = sanitizeValue(value);
      setLiveValues((prev) => {
        const next = new Map(prev);
        next.set(node_name, { value: cleanValue, access_mode });
        return next;
      });
      const cleanState: FeatureState = {
        ...state,
        value: sanitizeValue(state.value),
      };
      setLiveStates((prev) => {
        const next = new Map(prev);
        next.set(node_name, cleanState);
        return next;
      });
    }).then((fn) => {
      if (cancelled) {
        fn();
      } else {
        unlisten = fn;
      }
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
    listenTo("connection-state-changed", (state) => {
      if (state.kind === "disconnected" || state.kind === "error") {
        setLiveValues(new Map());
        setLiveStates(new Map());
      }
    }).then((fn) => {
      unlisten = fn;
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

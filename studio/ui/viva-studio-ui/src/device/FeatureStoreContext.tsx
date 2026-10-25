import { invokeCommand } from "./commands";
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useState,
  useSyncExternalStore,
  type ReactNode,
} from "react";
import { useToast } from "../context/ToastContext";
import { isTauri } from "../tauri";
import { formatErrorMessage } from "./errorMessage";
import { listenTo } from "./events";
import { FeatureStore } from "./featureStore";
import type { FeatureState, JsonValue } from "./types";

const FeatureStoreContext = createContext<FeatureStore | null>(null);

/**
 * NaN and the infinities cannot travel as JSON numbers; the backend sends
 * their text. Keep any number that is not finite out of the cache.
 */
function sanitize(state: FeatureState): FeatureState {
  const { value } = state;
  return typeof value === "number" && !Number.isFinite(value)
    ? { ...state, value: String(value) }
    : state;
}

async function fetchStates(names: string[]): Promise<Record<string, FeatureState>> {
  if (!isTauri()) return {};
  const states = await invokeCommand("query_feature_states_bulk", {
    names,
  });
  return Object.fromEntries(
    Object.entries(states).map(([name, state]) => [name, sanitize(state)]),
  );
}

/**
 * Holds the app's {@link FeatureStore} and keeps it current: it reads only
 * while `connected`, takes every state the backend announces, and re-reads
 * what a write or command invalidated (backlog ST-30).
 */
export function FeatureStoreProvider({
  connected,
  children,
}: {
  connected: boolean;
  children: ReactNode;
}) {
  const { addToast } = useToast();
  const [store] = useState(() => new FeatureStore(fetchStates));

  useEffect(() => {
    store.setErrorHandler((error) => {
      addToast("error", `Reading features failed: ${formatErrorMessage(error)}`);
    });
  }, [store, addToast]);

  useEffect(() => {
    store.setEnabled(connected);
  }, [store, connected]);

  useEffect(() => {
    if (!isTauri()) return;
    let cancelled = false;
    const unlisten: Array<() => void> = [];
    const keep = (fn: () => void) => (cancelled ? fn() : unlisten.push(fn));
    listenTo("feature-state-changed", ({ name, state }) => {
      store.set(name, sanitize(state));
    }).then(keep);
    listenTo("features-invalidated", ({ names }) => {
      store.invalidate(names);
    }).then(keep);
    return () => {
      cancelled = true;
      unlisten.forEach((fn) => fn());
    };
  }, [store]);

  return <FeatureStoreContext.Provider value={store}>{children}</FeatureStoreContext.Provider>;
}

export function useFeatureStore(): FeatureStore {
  const store = useContext(FeatureStoreContext);
  if (!store) throw new Error("useFeatureStore needs a FeatureStoreProvider");
  return store;
}

/**
 * The live state of `name`, read from the camera when first shown and kept
 * current. `undefined` until it arrives, and for no name.
 */
export function useFeature(name: string | null | undefined): FeatureState | undefined {
  const store = useFeatureStore();
  const subscribe = useCallback(
    (onChange: () => void) => (name ? store.subscribe(name, onChange) : () => {}),
    [store, name],
  );
  const read = useCallback(() => (name ? store.get(name) : undefined), [store, name]);
  return useSyncExternalStore(subscribe, read);
}

/**
 * Write one feature. A refusal reaches the user as a toast — the control
 * would otherwise go on showing a value the camera rejected — and the
 * promise says whether the write succeeded, so a caller writing several
 * features can stop at the first failure. The state the camera reports
 * afterwards replaces the cached one.
 */
export function useFeatureWriter(): (name: string, value: JsonValue) => Promise<boolean> {
  const store = useFeatureStore();
  const { addToast } = useToast();
  return useCallback(
    async (name, value) => {
      if (!isTauri()) return false;
      try {
        const state = await invokeCommand("write_node", { nodeName: name, value });
        store.set(name, sanitize(state));
        return true;
      } catch (error) {
        addToast("error", `Writing ${name} failed: ${formatErrorMessage(error)}`);
        return false;
      }
    },
    [store, addToast],
  );
}

/** Execute a command; a refusal reaches the user as a toast. */
export function useCommandRunner(): (name: string) => Promise<boolean> {
  const store = useFeatureStore();
  const { addToast } = useToast();
  return useCallback(
    async (name) => {
      if (!isTauri()) return false;
      try {
        const result = await invokeCommand("execute_command", { nodeName: name });
        if (!result.ok) {
          addToast("error", `${name} failed: ${result.error ?? "refused"}`);
          return false;
        }
        store.setMany(result.affected_states ?? {});
        return true;
      } catch (error) {
        addToast("error", `${name} failed: ${formatErrorMessage(error)}`);
        return false;
      }
    },
    [store, addToast],
  );
}

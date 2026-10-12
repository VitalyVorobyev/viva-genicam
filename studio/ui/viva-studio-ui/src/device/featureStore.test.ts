import { afterEach, describe, expect, it, vi } from "vitest";
import {
  FeatureStore,
  MIN_REFRESH_MS,
  NO_CACHE_REFRESH_MS,
  refreshInterval,
  type FeatureFetcher,
} from "./featureStore";
import type { FeatureState } from "./types";

function state(value: number): FeatureState {
  return {
    value,
    access_mode: "RW",
    kind: "Integer",
    is_implemented: true,
    is_available: true,
  };
}

/** A fetcher that answers every name with `values[name]` and records calls. */
function fetcher(values: Record<string, number>) {
  const calls: string[][] = [];
  const fetch: FeatureFetcher = async (names) => {
    calls.push([...names].sort());
    return Object.fromEntries(
      names.filter((n) => n in values).map((n) => [n, state(values[n])]),
    );
  };
  return { fetch, calls };
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("FeatureStore", () => {
  it("reads what is subscribed to, in one bulk read per tick", async () => {
    const { fetch, calls } = fetcher({ Width: 640, Height: 480, Gain: 3 });
    const store = new FeatureStore(fetch);
    store.setEnabled(true);
    store.subscribe("Width", () => {});
    store.subscribe("Height", () => {});
    await settle();
    expect(calls).toEqual([["Height", "Width"]]);
    expect(store.get("Width")?.value).toBe(640);
    // Nothing on screen asked for Gain.
    expect(store.get("Gain")).toBeUndefined();
  });

  it("tells a subscriber when its feature changes, and only then", async () => {
    const { fetch } = fetcher({ Width: 640 });
    const store = new FeatureStore(fetch);
    store.setEnabled(true);
    const width = vi.fn();
    const gain = vi.fn();
    store.subscribe("Width", width);
    store.subscribe("Gain", gain);
    await settle();
    width.mockClear();
    gain.mockClear();
    store.set("Width", state(320));
    expect(width).toHaveBeenCalledTimes(1);
    expect(gain).not.toHaveBeenCalled();
  });

  it("re-reads invalidated features on screen and forgets the rest", async () => {
    const { fetch, calls } = fetcher({ BalanceRatio: 1, Gamma: 2 });
    const store = new FeatureStore(fetch);
    store.setEnabled(true);
    store.subscribe("BalanceRatio", () => {});
    await settle();
    store.set("Gamma", state(2));
    calls.length = 0;

    store.invalidate(["BalanceRatio", "Gamma"]);
    await settle();
    expect(calls).toEqual([["BalanceRatio"]]);
    expect(store.get("Gamma")).toBeUndefined();
  });

  it("drops a reply that arrives after the cache was cleared", async () => {
    let answer: (states: Record<string, FeatureState>) => void = () => {};
    const store = new FeatureStore(
      () => new Promise((resolve) => (answer = resolve)),
    );
    store.setEnabled(true);
    store.subscribe("Width", () => {});
    await settle();
    store.clear(); // the device went away while the read was out
    answer({ Width: state(640) });
    await settle();
    expect(store.get("Width")).toBeUndefined();
  });

  // Codex on #172: two reads of one feature in flight, answered out of order.
  it("keeps the newest read when an older one answers last", async () => {
    const answers: Array<(states: Record<string, FeatureState>) => void> = [];
    const store = new FeatureStore(
      () => new Promise((resolve) => answers.push(resolve)),
    );
    store.setEnabled(true);
    store.subscribe("Width", () => {});
    await settle(); // read 1 is out
    store.invalidate(["Width"]);
    await settle(); // read 2 is out
    answers[1]!({ Width: state(320) }); // the newer read answers first
    await settle();
    answers[0]!({ Width: state(640) }); // then the older one
    await settle();
    expect(store.get("Width")?.value).toBe(320);
  });

  it("keeps a state the backend announced over a read already in flight", async () => {
    let answer: (states: Record<string, FeatureState>) => void = () => {};
    const store = new FeatureStore(() => new Promise((resolve) => (answer = resolve)));
    store.setEnabled(true);
    store.subscribe("Gain", () => {});
    await settle();
    store.set("Gain", state(6)); // e.g. the reply to a write
    answer({ Gain: state(0) });
    await settle();
    expect(store.get("Gain")?.value).toBe(6);
  });

  it("asks nothing while no device is connected, and reads on connect", async () => {
    const { fetch, calls } = fetcher({ Width: 640 });
    const store = new FeatureStore(fetch);
    store.subscribe("Width", () => {});
    await settle();
    expect(calls).toEqual([]);
    store.setEnabled(true);
    await settle();
    expect(calls).toEqual([["Width"]]);
    expect(store.get("Width")?.value).toBe(640);
  });

  it("loads features nothing shows, for an export", async () => {
    const { fetch } = fetcher({ Width: 640, Gain: 3 });
    const store = new FeatureStore(fetch);
    store.setEnabled(true);
    const all = await store.load(["Width", "Gain"]);
    expect([...all.keys()].sort()).toEqual(["Gain", "Width"]);
  });

  it("reports a failed read", async () => {
    const onError = vi.fn();
    const store = new FeatureStore(async () => {
      throw new Error("timeout");
    });
    store.setErrorHandler(onError);
    store.setEnabled(true);
    store.subscribe("Width", () => {});
    await settle();
    expect(onError).toHaveBeenCalledTimes(1);
  });
});

/** A fetcher that answers with whole states, so a test can give one a
 * polling time or a caching mode. */
function stateFetcher(states: Record<string, FeatureState>) {
  const calls: string[][] = [];
  const fetch: FeatureFetcher = async (names) => {
    calls.push([...names].sort());
    return Object.fromEntries(names.filter((n) => n in states).map((n) => [n, states[n]]));
  };
  return { fetch, calls };
}

describe("FeatureStore polling (GA-05)", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it("reads a feature on screen again at its polling time, and stops when it leaves", async () => {
    vi.useFakeTimers();
    const { fetch, calls } = stateFetcher({
      DeviceTemperature: { ...state(40), kind: "Float", polling_ms: 500, caching: "NoCache" },
      Width: { ...state(640), caching: "WriteThrough" },
    });
    const store = new FeatureStore(fetch);
    store.setEnabled(true);
    const stop = store.subscribe("DeviceTemperature", () => {});
    store.subscribe("Width", () => {});
    await vi.advanceTimersByTimeAsync(0);
    expect(calls).toEqual([["DeviceTemperature", "Width"]]);

    await vi.advanceTimersByTimeAsync(499);
    expect(calls).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(calls).toEqual([["DeviceTemperature", "Width"], ["DeviceTemperature"]]);
    await vi.advanceTimersByTimeAsync(500);
    expect(calls).toHaveLength(3);

    stop();
    await vi.advanceTimersByTimeAsync(2000);
    expect(calls).toHaveLength(3);
  });

  it("reads a NoCache feature without a polling time once a second", async () => {
    vi.useFakeTimers();
    const { fetch, calls } = stateFetcher({
      LineStatus: { ...state(1), kind: "Boolean", caching: "NoCache" },
    });
    const store = new FeatureStore(fetch);
    store.setEnabled(true);
    store.subscribe("LineStatus", () => {});
    await vi.advanceTimersByTimeAsync(0);
    await vi.advanceTimersByTimeAsync(NO_CACHE_REFRESH_MS - 1);
    expect(calls).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(calls).toHaveLength(2);
  });

  it("stops polling when the camera goes away", async () => {
    vi.useFakeTimers();
    const { fetch, calls } = stateFetcher({
      DeviceTemperature: { ...state(40), polling_ms: 500 },
    });
    const store = new FeatureStore(fetch);
    store.setEnabled(true);
    store.subscribe("DeviceTemperature", () => {});
    await vi.advanceTimersByTimeAsync(500);
    expect(calls).toHaveLength(2);
    store.setEnabled(false);
    await vi.advanceTimersByTimeAsync(2000);
    expect(calls).toHaveLength(2);
  });

  it("knows how often to refresh a state", () => {
    expect(refreshInterval({ ...state(1), polling_ms: 500 })).toBe(500);
    expect(refreshInterval({ ...state(1), polling_ms: 5 })).toBe(MIN_REFRESH_MS);
    expect(refreshInterval({ ...state(1), caching: "NoCache" })).toBe(NO_CACHE_REFRESH_MS);
    expect(refreshInterval({ ...state(1), caching: "WriteAround" })).toBeUndefined();
    expect(refreshInterval(state(1))).toBeUndefined();
  });
});

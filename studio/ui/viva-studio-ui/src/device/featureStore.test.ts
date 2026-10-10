import { describe, expect, it, vi } from "vitest";
import { FeatureStore, type FeatureFetcher } from "./featureStore";
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

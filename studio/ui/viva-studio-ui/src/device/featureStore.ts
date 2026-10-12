import type { FeatureState } from "./types";

/**
 * How often a `NoCache` feature that declares no polling time is read again
 * while it is shown, in milliseconds.
 *
 * `NoCache` says the camera changes the value with nothing to announce it; 546
 * of the 883 `NoCache` registers in the vendor corpus declare no
 * `<PollingTime>` to say how fast. Where vendors do say, 500 and 1000 ms are
 * by far the commonest values; the slower of the two keeps a screenful of
 * such features to one read each per second.
 */
export const NO_CACHE_REFRESH_MS = 1000;

/**
 * The shortest interval a feature is polled at, in milliseconds. The shortest
 * `<PollingTime>` in a vendor document of the corpus is 50 ms; faster than
 * that is more than a screen shows, and only floods the control channel.
 */
export const MIN_REFRESH_MS = 50;

/**
 * How often to read a feature again while it is shown: its polling time —
 * the XML's `<PollingTime>`, on the feature or on a register it is read
 * through — or, for a `NoCache` feature without one, {@link NO_CACHE_REFRESH_MS}.
 * `undefined`: it changes only when written, and is read again then.
 */
export function refreshInterval(state: FeatureState): number | undefined {
  const polling = state.polling_ms;
  if (polling != null && polling > 0) return Math.max(MIN_REFRESH_MS, polling);
  if (state.caching === "NoCache") return NO_CACHE_REFRESH_MS;
  return undefined;
}

/** Reads the live state of many features in one round-trip. */
export type FeatureFetcher = (names: string[]) => Promise<Record<string, FeatureState>>;

/**
 * The UI's one cache of live feature state (backlog ST-29).
 *
 * Every control reads a feature through {@link FeatureStore.subscribe}, and
 * the store reads from the camera only what is subscribed to: the features on
 * screen. A camera has hundreds of features and reading each costs several
 * register reads, so fetching all of them at connect — what the UI did before
 * — kept a real camera busy for seconds before anything showed.
 *
 * Requests made in the same tick go out as one bulk read. A state that
 * arrives after {@link FeatureStore.clear} belongs to the previous
 * connection and is dropped, and so is a read of a feature that a newer read
 * or a newer state has already superseded: reads can answer out of order.
 *
 * A feature on screen whose value the camera changes by itself is read again
 * at its {@link refreshInterval} (backlog GA-05); features due together go
 * out as one bulk read. A read that fails stops that feature's polling until
 * it is read again some other way.
 */
export class FeatureStore {
  private readonly states = new Map<string, FeatureState>();
  private readonly listeners = new Map<string, Set<() => void>>();
  private readonly queued = new Set<string>();
  /** Per feature: bumped by each read sent and each state stored. */
  private readonly versions = new Map<string, number>();
  private flushScheduled = false;
  /** Per polled feature on screen: when to read it again (`Date.now()`). */
  private readonly due = new Map<string, number>();
  private pollTimer: ReturnType<typeof setTimeout> | undefined;
  private generation = 0;
  private enabled = false;
  private onError: (error: unknown) => void = () => {};

  constructor(private readonly fetch: FeatureFetcher) {}

  /** Where a failed read is reported. */
  setErrorHandler(onError: (error: unknown) => void): void {
    this.onError = onError;
  }

  get(name: string): FeatureState | undefined {
    return this.states.get(name);
  }

  /**
   * Follow `name`. The first subscriber to a feature the store does not hold
   * yet asks the camera for it. So does the first subscriber to a feature
   * that is polled: what the store kept while nothing showed it is stale,
   * and its polling stopped when it left the screen.
   */
  subscribe(name: string, listener: () => void): () => void {
    let set = this.listeners.get(name);
    if (!set) {
      set = new Set();
      this.listeners.set(name, set);
    }
    const first = set.size === 0;
    set.add(listener);
    const cached = this.states.get(name);
    if (cached === undefined || (first && refreshInterval(cached) !== undefined)) {
      this.request([name]);
    }
    return () => {
      set.delete(listener);
      if (set.size === 0) {
        this.listeners.delete(name);
        if (this.due.delete(name)) this.armPollTimer();
      }
    };
  }

  /** Whether a control is showing `name`. */
  isWatched(name: string): boolean {
    return this.listeners.has(name);
  }

  /**
   * Start or stop talking to the camera. While disabled — no device
   * connected — requests are dropped rather than sent. Either way the cache
   * starts over, and on enabling, the features on screen are read.
   */
  setEnabled(enabled: boolean): void {
    if (enabled === this.enabled) return;
    this.enabled = enabled;
    this.clear();
    if (enabled) this.refresh();
  }

  /**
   * Store states that arrived some other way: a write's reply, an event.
   * A read already in flight for the feature is older, and is dropped.
   */
  set(name: string, state: FeatureState): void {
    this.bump(name);
    this.install(name, state);
  }

  setMany(states: Record<string, FeatureState>): void {
    for (const [name, state] of Object.entries(states)) this.set(name, state);
  }

  /**
   * A write or command may have changed these features. Those on screen are
   * read again; the rest are forgotten, so they are read when next shown.
   */
  invalidate(names: string[]): void {
    const watched: string[] = [];
    for (const name of names) {
      if (this.isWatched(name)) watched.push(name);
      else this.states.delete(name);
    }
    this.request(watched);
  }

  /** Read every feature on screen again. */
  refresh(): void {
    this.request([...this.listeners.keys()]);
  }

  /**
   * Read `names` now, whether shown or not, and resolve with what came back.
   * Used for "Export State", which needs every feature.
   */
  async load(names: string[]): Promise<Map<string, FeatureState>> {
    if (!this.enabled || names.length === 0) return new Map();
    const generation = this.generation;
    const states = await this.fetch(names);
    if (generation !== this.generation) return new Map();
    this.setMany(states);
    return new Map(Object.entries(states));
  }

  /** Forget everything: the device went away or another one connected. */
  clear(): void {
    this.generation += 1;
    this.queued.clear();
    this.due.clear();
    this.armPollTimer();
    const names = [...this.states.keys()];
    this.states.clear();
    for (const name of names) this.notify(name);
  }

  private request(names: string[]): void {
    if (!this.enabled || names.length === 0) return;
    for (const name of names) this.queued.add(name);
    if (this.flushScheduled) return;
    this.flushScheduled = true;
    queueMicrotask(() => {
      this.flushScheduled = false;
      void this.flush();
    });
  }

  private async flush(): Promise<void> {
    const names = [...this.queued];
    this.queued.clear();
    if (names.length === 0) return;
    const generation = this.generation;
    const tickets = new Map(names.map((name) => [name, this.bump(name)]));
    try {
      const states = await this.fetch(names);
      if (generation !== this.generation) return;
      for (const [name, state] of Object.entries(states)) {
        // Only the newest read of a feature, with nothing stored since, lands.
        if (tickets.get(name) === this.versions.get(name)) this.install(name, state);
      }
    } catch (error) {
      if (generation === this.generation) this.onError(error);
    }
  }

  private bump(name: string): number {
    const version = (this.versions.get(name) ?? 0) + 1;
    this.versions.set(name, version);
    return version;
  }

  private install(name: string, state: FeatureState): void {
    this.states.set(name, state);
    this.schedule(name, state);
    this.notify(name);
  }

  /** Poll `name` at its refresh interval while it is on screen. */
  private schedule(name: string, state: FeatureState): void {
    const interval = refreshInterval(state);
    if (interval === undefined || !this.isWatched(name)) {
      if (!this.due.delete(name)) return;
    } else {
      this.due.set(name, Date.now() + interval);
    }
    this.armPollTimer();
  }

  /** One timer, for the feature due first. */
  private armPollTimer(): void {
    if (this.pollTimer !== undefined) {
      clearTimeout(this.pollTimer);
      this.pollTimer = undefined;
    }
    if (!this.enabled || this.due.size === 0) return;
    const next = Math.min(...this.due.values());
    this.pollTimer = setTimeout(() => this.pollDue(), Math.max(0, next - Date.now()));
  }

  private pollDue(): void {
    this.pollTimer = undefined;
    const now = Date.now();
    const names: string[] = [];
    for (const [name, at] of this.due) {
      if (at <= now) names.push(name);
    }
    for (const name of names) this.due.delete(name);
    this.request(names.filter((name) => this.isWatched(name)));
    this.armPollTimer();
  }

  private notify(name: string): void {
    for (const listener of this.listeners.get(name) ?? []) listener();
  }
}

import type { FeatureState } from "./types";

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
 * connection and is dropped.
 */
export class FeatureStore {
  private readonly states = new Map<string, FeatureState>();
  private readonly listeners = new Map<string, Set<() => void>>();
  private readonly queued = new Set<string>();
  private flushScheduled = false;
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
   * yet asks the camera for it.
   */
  subscribe(name: string, listener: () => void): () => void {
    let set = this.listeners.get(name);
    if (!set) {
      set = new Set();
      this.listeners.set(name, set);
    }
    set.add(listener);
    if (!this.states.has(name)) this.request([name]);
    return () => {
      set.delete(listener);
      if (set.size === 0) this.listeners.delete(name);
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

  /** Store states that arrived some other way: a write's reply, an event. */
  set(name: string, state: FeatureState): void {
    this.states.set(name, state);
    this.notify(name);
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
    try {
      const states = await this.fetch(names);
      if (generation === this.generation) this.setMany(states);
    } catch (error) {
      if (generation === this.generation) this.onError(error);
    }
  }

  private notify(name: string): void {
    for (const listener of this.listeners.get(name) ?? []) listener();
  }
}

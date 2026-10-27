import { emit } from "@tauri-apps/api/event";
import { mockIPC } from "@tauri-apps/api/mocks";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { ToastProvider } from "../context/ToastContext";
import { FeatureStoreProvider, useFeature, useFeatureWriter, useCommandRunner } from "./FeatureStoreContext";
import type { FeatureState, FeaturesInvalidated, FeatureStateChanged } from "./types";

function state(value: number): FeatureState {
  return { value, access_mode: "RW", kind: "Float", is_implemented: true, is_available: true };
}

/** The camera; `values` can change behind the UI's back, as a selector write would. */
function camera(values: Record<string, number>) {
  const reads: string[][] = [];
  mockIPC(
    (cmd, args) => {
      if (cmd === "query_feature_states_bulk") {
        const names = (args as { names: string[] }).names;
        reads.push([...names].sort());
        return Object.fromEntries(names.map((n) => [n, state(values[n] ?? 0)]));
      }
      return undefined;
    },
    { shouldMockEvents: true },
  );
  return reads;
}

function Value({ name }: { name: string }) {
  const s = useFeature(name);
  return <span data-testid={name}>{s ? String(s.value) : "…"}</span>;
}

function app(...names: string[]) {
  return (
    <ToastProvider>
      <FeatureStoreProvider connected={true}>
        {names.map((n) => (
          <Value key={n} name={n} />
        ))}
      </FeatureStoreProvider>
    </ToastProvider>
  );
}

describe("FeatureStoreProvider", () => {
  it("immediately clamps cached states and refuses writes and commands after control loss", async () => {
    const writes: string[] = [];
    mockIPC((cmd) => {
      if (cmd === "query_feature_states_bulk") return { Gain: state(6) };
      if (cmd === "write_node" || cmd === "execute_command") writes.push(cmd);
      return undefined;
    }, { shouldMockEvents: true });
    function Controls() {
      const s = useFeature("Gain");
      const write = useFeatureWriter();
      const run = useCommandRunner();
      return <><span data-testid="access">{s?.access_mode}</span><button onClick={() => write("Gain", 10)}>Write</button><button onClick={() => run("DeviceReset")}>Execute</button></>;
    }
    const tree = (readOnly: boolean) => <ToastProvider><FeatureStoreProvider connected readOnly={readOnly}><Controls /></FeatureStoreProvider></ToastProvider>;
    const view = render(tree(false));
    await waitFor(() => expect(screen.getByTestId("access").textContent).toBe("RW"));
    view.rerender(tree(true));
    expect(screen.getByTestId("access").textContent).toBe("RO");
    await userEvent.click(screen.getByRole("button", { name: "Write" }));
    await userEvent.click(screen.getByRole("button", { name: "Execute" }));
    expect(writes).toEqual([]);
  });
  // ST-30: writing BalanceRatioSelector changes what BalanceRatio reads.
  it("re-reads the invalidated features on screen, and only those", async () => {
    const values = { BalanceRatio: 1.0, Gamma: 2.2 };
    const reads = camera(values);
    render(app("BalanceRatio"));
    await waitFor(() => expect(screen.getByTestId("BalanceRatio").textContent).toBe("1"));
    reads.length = 0;

    values.BalanceRatio = 1.5;
    const invalidated: FeaturesInvalidated = {
      cause: "BalanceRatioSelector",
      names: ["BalanceRatio", "Gamma"],
    };
    await act(() => emit("features-invalidated", invalidated));

    await waitFor(() => expect(screen.getByTestId("BalanceRatio").textContent).toBe("1.5"));
    expect(reads).toEqual([["BalanceRatio"]]);
  });

  it("takes a state the backend announces", async () => {
    camera({ Gain: 0 });
    render(app("Gain"));
    await waitFor(() => expect(screen.getByTestId("Gain").textContent).toBe("0"));
    const changed: FeatureStateChanged = { name: "Gain", state: state(6) };
    await act(() => emit("feature-state-changed", changed));
    await waitFor(() => expect(screen.getByTestId("Gain").textContent).toBe("6"));
  });
});

it("isolates a replacement session even if connected never renders false", async () => {
  let finishOld!: (states: Record<string, FeatureState>) => void;
  const old = new Promise<Record<string, FeatureState>>((resolve) => { finishOld = resolve; });
  let reads = 0;
  mockIPC((cmd) => {
    if (cmd === "query_feature_states_bulk") return ++reads === 1 ? old : { Gain: state(6) };
    return undefined;
  });
  const tree = (session: number) => <ToastProvider><FeatureStoreProvider connected sessionKey={session}><Value name="Gain" /></FeatureStoreProvider></ToastProvider>;
  const view = render(tree(1));
  await waitFor(() => expect(reads).toBe(1));
  view.rerender(tree(2));
  await waitFor(() => expect(screen.getByTestId("Gain").textContent).toBe("6"));
  await act(async () => finishOld({ Gain: state(777) }));
  expect(screen.getByTestId("Gain").textContent).toBe("6");
});

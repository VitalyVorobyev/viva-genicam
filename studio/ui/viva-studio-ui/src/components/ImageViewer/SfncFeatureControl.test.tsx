import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { FeatureStoreProvider } from "../../device/FeatureStoreContext";
import { ToastProvider } from "../../context/ToastContext";
import { makeModel } from "../../test/model";
import type { FeatureState } from "../../device/types";
import { SfncFeatureControl, resolveCandidateNode } from "./SfncFeatureControl";

const feature = { node: "ExposureTime", aliases: ["ExposureTimeAbs", "ExposureTimeRaw"], widget: "float_input" };
const model = makeModel({
  ExposureTime: { kind: "Float", display_name: "Exposure" },
  ExposureTimeAbs: { kind: "Float", display_name: "Legacy exposure" },
  ExposureTimeRaw: { kind: "Integer", display_name: "Raw exposure" },
});
const nodes = model.graph.nodes_by_name;
const numeric = (extra: Partial<FeatureState> = {}): FeatureState => ({ value: 42, kind: "Float", access_mode: "RW", is_implemented: true, is_available: true, numeric: { min: 2, max: 100, inc: 2 }, ...extra });

describe("SFNC candidate resolution", () => {
  it("prefers the available canonical feature and retains its read-only constraint", () => {
    const states = { ExposureTime: numeric({ access_mode: "RO" }), ExposureTimeAbs: numeric() };
    expect(resolveCandidateNode([nodes.ExposureTime, nodes.ExposureTimeAbs], (name) => states[name as keyof typeof states])).toBe(nodes.ExposureTime);
  });
  it("falls back when canonical is declared but unimplemented or unavailable", () => {
    for (const state of [numeric({ is_implemented: false }), numeric({ is_available: false })]) {
      expect(resolveCandidateNode([nodes.ExposureTime, nodes.ExposureTimeAbs], (name) => name === "ExposureTime" ? state : numeric())).toBe(nodes.ExposureTimeAbs);
    }
  });
  it("omits controls when no candidate is present or implemented", () => {
    expect(resolveCandidateNode([], () => undefined)).toBeUndefined();
    expect(resolveCandidateNode([nodes.ExposureTime], () => numeric({ is_implemented: false }))).toBeUndefined();
  });
  it("renders Raw's own label, integer range and unit without inventing microseconds", async () => {
    mockIPC((cmd, raw) => cmd === "query_feature_states_bulk" ? Object.fromEntries(((raw as { names: string[] }).names).map((name) => [name, numeric({ kind: "Integer", unit: "ticks" })])) : undefined);
    render(<ToastProvider><FeatureStoreProvider connected><SfncFeatureControl feature={feature} nodes={{ ExposureTimeRaw: nodes.ExposureTimeRaw }} /></FeatureStoreProvider></ToastProvider>);
    const field = await screen.findByRole("spinbutton", { name: "Raw exposure" }) as HTMLInputElement;
    await waitFor(() => expect(field.value).toBe("42"));
    expect(field.min).toBe("2"); expect(field.max).toBe("100"); expect(field.step).toBe("2");
    expect(field.closest(".iv-row")?.textContent).toContain("ticks");
    expect(field.closest(".iv-row")?.textContent).not.toContain("µs");
  });
});

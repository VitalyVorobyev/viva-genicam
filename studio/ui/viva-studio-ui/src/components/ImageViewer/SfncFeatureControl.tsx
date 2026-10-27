import { useCallback, useMemo, useSyncExternalStore, type ReactNode } from "react";
import type { SfncGroup, FeatureState } from "../../device/types";
import type { UiNode } from "../../xml_model/uigraph";
import { useFeatureStore } from "../../device/FeatureStoreContext";
import { FeatureControl } from "../controls/FeatureControl";

type SfncFeature = SfncGroup["features"][number];

/** The first implemented, available candidate; retain a pending or blocked row
 * when none is currently available, so its actual camera restriction is shown. */
export function resolveCandidateNode(
  candidates: readonly UiNode[],
  read: (name: string) => FeatureState | undefined,
): UiNode | undefined {
  return candidates.find((node) => {
    const state = read(node.name);
    return state?.is_implemented && state.is_available;
  }) ?? candidates.find((node) => read(node.name) === undefined)
    ?? candidates.find((node) => read(node.name)?.is_implemented);
}

/** Resolve canonical/legacy names against the one live FeatureStore. One hook
 * follows the candidate set, so XML replacement may change its length safely. */
export function SfncFeatureControl({ feature, nodes, leading }: {
  feature: SfncFeature;
  nodes: Record<string, UiNode>;
  leading?: ReactNode;
}) {
  const store = useFeatureStore();
  const candidates = useMemo(() =>
    [feature.node, ...(feature.aliases ?? [])].map((name) => nodes[name]).filter(Boolean),
  [feature, nodes]);
  const subscribe = useCallback((notify: () => void) => {
    const subscriptions = candidates.map((node) => store.subscribe(node.name, notify));
    return () => subscriptions.forEach((unsubscribe) => unsubscribe());
  }, [store, candidates]);
  const read = useCallback(() => resolveCandidateNode(candidates, (name) => store.get(name)), [store, candidates]);
  const node = useSyncExternalStore(subscribe, read);
  // Raw fallback features retain their own integer semantics, units and range.
  const widget = feature.widget === "float_input" && node?.kind === "Integer" ? "int_input" : feature.widget;
  return <FeatureControl node={node} widget={widget} leading={leading} />;
}

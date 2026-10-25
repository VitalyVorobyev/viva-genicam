import { invokeCommand } from "./commands";
import { useEffect, useState } from "react";
import { isTauri } from "../tauri";
import type { SfncGroup } from "./types";

export function useSfncGroups(): SfncGroup[] {
  const [groups, setGroups] = useState<SfncGroup[]>([]);

  useEffect(() => {
    if (!isTauri()) return;
    let cancelled = false;
    invokeCommand("get_sfnc_groups")
      .then((g) => {
        if (!cancelled) setGroups(g);
      })
      .catch((e) => console.error("get_sfnc_groups failed:", e));
    return () => {
      cancelled = true;
    };
  }, []);

  return groups;
}

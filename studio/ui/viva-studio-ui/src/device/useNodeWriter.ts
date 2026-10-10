import { useCallback } from "react";
import { useToast } from "../context/ToastContext";
import { isTauri } from "../tauri";
import { formatErrorMessage } from "./errorMessage";

/**
 * Write one feature from a control outside the Feature Browser. A refusal
 * reaches the user as a toast — the control would otherwise go on showing a
 * value the camera rejected — and the returned promise says whether the write
 * succeeded, so a caller writing several features can stop at the first
 * failure.
 */
export function useNodeWriter(): (
  nodeName: string,
  value: number | string | boolean,
) => Promise<boolean> {
  const { addToast } = useToast();
  return useCallback(
    async (nodeName, value) => {
      if (!isTauri()) return false;
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        await invoke("write_node", { nodeName, value });
        return true;
      } catch (err) {
        addToast("error", `Writing ${nodeName} failed: ${formatErrorMessage(err)}`);
        return false;
      }
    },
    [addToast],
  );
}

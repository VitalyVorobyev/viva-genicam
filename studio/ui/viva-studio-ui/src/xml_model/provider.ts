import type { ParseXmlResponse } from "./uigraph";
import type { NodeValue } from "./values";
import type { CommandResult, FeatureState } from "../device/types";

export interface XmlModelProvider {
  parseXml(xml: string): Promise<ParseXmlResponse>;
  listFixtures?(): Promise<string[]>;
  loadFixture?(name: string): Promise<ParseXmlResponse>;
  getCurrentModel?(): Promise<ParseXmlResponse | null>;
  /**
   * Apply a value to the device and return the refreshed `FeatureState` that
   * results. Callers MUST reconcile their draft form state to
   * `result.value` — devices routinely clamp or round writes, and that is the
   * authoritative post-write state.
   */
  applyNodeValue?(nodeName: string, value: NodeValue): Promise<FeatureState>;
  /**
   * Execute a Command node. The returned [`CommandResult`] carries `ok`/error
   * plus `affected_states` — a map of nodes whose value changed as a side
   * effect (e.g. `AcquisitionStatus` after `AcquisitionStart`). Callers
   * surface `error` to the user and cache `affected_states` as live state.
   */
  executeCommand?(nodeName: string): Promise<CommandResult>;
}

// Tauri provider bridges the UI to native Rust parsing. It also exposes fixtures
// and can fetch the last loaded model from the Rust-side state.
export class TauriProvider implements XmlModelProvider {
  async parseXml(xml: string): Promise<ParseXmlResponse> {
    return await invokeNative<ParseXmlResponse>("parse_xml", { xml });
  }

  async listFixtures(): Promise<string[]> {
    return await invokeNative<string[]>("list_fixtures");
  }

  async loadFixture(name: string): Promise<ParseXmlResponse> {
    return await invokeNative<ParseXmlResponse>("load_fixture", { name });
  }

  async getCurrentModel(): Promise<ParseXmlResponse | null> {
    return await invokeNative<ParseXmlResponse | null>("get_current_model");
  }

  async applyNodeValue(nodeName: string, value: NodeValue): Promise<FeatureState> {
    return await invokeNative<FeatureState>("write_node", {
      nodeName,
      value: nodeValueToJson(value),
    });
  }

  async executeCommand(nodeName: string): Promise<CommandResult> {
    return await invokeNative<CommandResult>("execute_command", { nodeName });
  }
}

async function invokeNative<T>(command: string, payload?: Record<string, unknown>) {
  const { invoke } = await import("@tauri-apps/api/core");
  return (await invoke(command, payload)) as T;
}

function nodeValueToJson(value: NodeValue): unknown {
  if (value === null) return null;
  if (typeof value === "object" && "enumName" in value) return value.enumName;
  return value;
}

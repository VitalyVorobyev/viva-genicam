import { useState } from "react";
import { useToast } from "../../context/ToastContext";
import { invokeCommand } from "../../device/commands";
import { formatErrorMessage } from "../../device/errorMessage";
import { useCameraReadOnly } from "../../device/FeatureStoreContext";
import type { ConfigurationLoaded, ConfigurationSaveReport } from "../../device/types";
import { isTauri } from "../../tauri";

const FILTERS = [{ name: "Camera configuration", extensions: ["txt", "pfs"] }];

type Outcome =
  | { kind: "saved"; report: ConfigurationSaveReport }
  | { kind: "loaded"; path: string; report: ConfigurationLoaded };

/** The file the user picks to save to; `null` if they cancelled. */
async function pickSavePath(): Promise<string | null> {
  const { save } = await import("@tauri-apps/plugin-dialog");
  return save({ defaultPath: "camera-configuration.txt", filters: FILTERS });
}

/** The file the user picks to load; `null` if they cancelled. */
async function pickOpenPath(): Promise<string | null> {
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({ multiple: false, directory: false, filters: FILTERS });
  return typeof picked === "string" ? picked : null;
}

/**
 * Save the camera's configuration — every feature its XML marks
 * `<Streamable>`, under each selector entry — to a GenApi feature file, and
 * load one back (backlog ST-50). Loading reports every value the camera
 * refused, and a file saved from another model; every feature on screen is
 * read again afterwards.
 */
export function ConfigurationPanel({ connected, acquiring }: { connected: boolean; acquiring: boolean }) {
  const { addToast } = useToast();
  const readOnly = useCameraReadOnly();
  const [busy, setBusy] = useState(false);
  const [outcome, setOutcome] = useState<Outcome | null>(null);

  if (!connected) return null;

  const save = async () => {
    const path = await pickSavePath();
    if (!path) return;
    setBusy(true);
    try {
      const report = await invokeCommand("save_camera_configuration", { path });
      setOutcome({ kind: "saved", report });
      addToast("success", `Saved ${report.values} values`);
    } catch (error) {
      addToast("error", `Saving the configuration failed: ${formatErrorMessage(error)}`);
    } finally {
      setBusy(false);
    }
  };

  const load = async () => {
    const path = await pickOpenPath();
    if (!path) return;
    setBusy(true);
    try {
      const report = await invokeCommand("load_camera_configuration", { path });
      setOutcome({ kind: "loaded", path, report });
      const complete = report.failures.length === 0 && report.warnings.length === 0;
      addToast(complete ? "success" : "info", `Restored ${report.written} values`);
    } catch (error) {
      addToast("error", `Loading the configuration failed: ${formatErrorMessage(error)}`);
    } finally {
      setBusy(false);
    }
  };

  const disabled = busy || readOnly || !isTauri();
  return (
    <section className="tool-panel" aria-label="Camera configuration">
      <div className="tool-panel__header">
        <span className="diagnostics-tab__title">Configuration</span>
        <div className="report-panel__actions">
          <button type="button" className="btn btn--sm" disabled={disabled} onClick={() => void save()}>
            {busy ? "Working…" : "Save to file…"}
          </button>
          <button type="button" className="btn--ghost" disabled={disabled} onClick={() => void load()}>
            Load from file…
          </button>
        </div>
      </div>
      {readOnly ? (
        <p className="device-panel__note">
          Read-only session: saving sets selectors to read each entry, and loading writes features.
        </p>
      ) : (
        acquiring && (
          <p className="device-panel__note">
            The camera is acquiring: features it locks while streaming are left out of a save.
            Stop acquisition first to save them.
          </p>
        )
      )}
      {outcome?.kind === "saved" && <Saved report={outcome.report} />}
      {outcome?.kind === "loaded" && <Loaded path={outcome.path} report={outcome.report} />}
    </section>
  );
}

function Saved({ report }: { report: ConfigurationSaveReport }) {
  return (
    <>
      <p className="tool-panel__result" role="status">
        Saved {report.values} values to {report.path}.
        {report.omitted.length > 0 && ` ${report.omitted.length} left out.`}
      </p>
      {report.omitted.length > 0 && (
        <details className="tool-panel__details">
          <summary>Left out, and why</summary>
          <ul className="tool-panel__list" aria-label="Features left out">
            {report.omitted.map((omission, index) => (
              <li key={`${omission.feature}:${index}`}>
                {omission.feature}: {omission.reason}
              </li>
            ))}
          </ul>
        </details>
      )}
    </>
  );
}

function Loaded({ path, report }: { path: string; report: ConfigurationLoaded }) {
  return (
    <>
      <p className="tool-panel__result" role="status">
        Restored {report.written} values from {path}
        {report.written_on_retry > 0 && ` (${report.written_on_retry} on a second pass)`}.
        {report.failures.length > 0 && ` ${report.failures.length} could not be written.`}
      </p>
      {report.warnings.length > 0 && (
        <ul className="tool-panel__list tool-panel__list--warning" role="alert" aria-label="Configuration warnings">
          {report.warnings.map((warning) => (
            <li key={warning}>{warning}</li>
          ))}
        </ul>
      )}
      {report.failures.length > 0 && (
        <ul className="tool-panel__list tool-panel__list--warning" aria-label="Values not written">
          {report.failures.map((failure, index) => (
            <li key={`${failure}:${index}`}>{failure}</li>
          ))}
        </ul>
      )}
    </>
  );
}

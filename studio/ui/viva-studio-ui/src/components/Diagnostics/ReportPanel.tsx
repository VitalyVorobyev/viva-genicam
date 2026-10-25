import { invokeCommand } from "../../device/commands";
import { useState } from "react";
import { useToast } from "../../context/ToastContext";
import { formatErrorMessage } from "../../device/errorMessage";
import { isTauri } from "../../tauri";

const ISSUES_URL = "https://github.com/VitalyVorobyev/viva-genicam/issues";

/** Save `text` where the user chooses; `false` if they cancelled. */
async function saveText(text: string, suggestedName: string): Promise<boolean> {
  const { save } = await import("@tauri-apps/plugin-dialog");
  const { writeFile } = await import("@tauri-apps/plugin-fs");
  const path = await save({
    defaultPath: suggestedName,
    filters: [{ name: "Text", extensions: ["txt"] }],
  });
  if (!path) return false;
  await writeFile(path, new TextEncoder().encode(text));
  return true;
}

/**
 * "Generate report": the plain-text bundle `viva-camctl report` writes —
 * host, interfaces, discovery, the camera's registers, the features the
 * library dropped and its GenApi XML — for the user to attach to an issue.
 * It is what we ask every reporter for (backlog ST-41).
 */
export function ReportPanel() {
  const { addToast } = useToast();
  const [includeXml, setIncludeXml] = useState(true);
  const [busy, setBusy] = useState(false);

  const generate = async (then: "save" | "copy") => {
    setBusy(true);
    try {
      const report = await invokeCommand("generate_report", { includeXml });
      if (then === "copy") {
        await navigator.clipboard.writeText(report);
        addToast("success", "Report copied — paste it into an issue");
      } else if (await saveText(report, "viva-report.txt")) {
        addToast("success", `Report saved — attach it to an issue at ${ISSUES_URL}`);
      }
    } catch (e) {
      addToast("error", `Report failed: ${formatErrorMessage(e)}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="report-panel" aria-label="Diagnostic report">
      <div className="report-panel__text">
        <span className="diagnostics-tab__title">Diagnostic report</span>
        <p className="report-panel__hint">
          Everything a bug report needs about this computer and the connected
          camera, in one text file. Nothing is sent anywhere.
        </p>
      </div>
      <label className="report-panel__option">
        <input
          type="checkbox"
          checked={includeXml}
          onChange={(e) => setIncludeXml(e.target.checked)}
        />
        Include the camera&apos;s GenApi XML
      </label>
      <div className="report-panel__actions">
        <button
          type="button"
          className="btn btn--sm"
          disabled={busy || !isTauri()}
          onClick={() => void generate("save")}
        >
          {busy ? "Writing…" : "Save report…"}
        </button>
        <button
          type="button"
          className="btn--ghost"
          disabled={busy || !isTauri()}
          onClick={() => void generate("copy")}
        >
          Copy
        </button>
      </div>
    </section>
  );
}

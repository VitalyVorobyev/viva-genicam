import { useCallback, useEffect, useState } from "react";
import { useToast } from "../../../context/ToastContext";
import { invokeCommand } from "../../../device/commands";
import { formatErrorMessage } from "../../../device/errorMessage";
import type { FeatureState } from "../../../device/types";
import { isTauri } from "../../../tauri";
import {
  MAX_DISPLAY_BYTES,
  hexRows,
  parseHexBytes,
  registerLength,
  toHexText,
} from "./registerHex";

interface RegisterEditorProps {
  name: string;
  /**
   * The register's live state: its length, and an access mode already capped
   * to read-only for a read-only session.
   */
  liveState: FeatureState;
}

type Read =
  | { kind: "loading" }
  | { kind: "ready"; bytes: number[] }
  | { kind: "failed"; reason: string };

/**
 * A `<Register>` node: its bytes as a hex dump, and an editor that writes it
 * whole (backlog ST-53). The bytes are not part of the feature's live state —
 * a register can be 100 000 bytes — so they are read here, on Refresh, and
 * after each write; what the write invalidated is re-read by the store.
 */
export function RegisterEditor({ name, liveState }: RegisterEditorProps) {
  const { addToast } = useToast();
  const length = registerLength(liveState.value);
  const access = liveState.access_mode;
  const readable = access === "RO" || access === "RW";
  const writable = (access === "WO" || access === "RW") && liveState.is_available;
  const [read, setRead] = useState<Read>({ kind: "loading" });
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    if (!isTauri()) return;
    try {
      const bytes = await invokeCommand("read_register", { nodeName: name });
      setRead({ kind: "ready", bytes });
    } catch (error) {
      setRead({ kind: "failed", reason: formatErrorMessage(error) });
    }
  }, [name]);

  useEffect(() => {
    if (!readable) return;
    let cancelled = false;
    if (isTauri()) {
      invokeCommand("read_register", { nodeName: name }).then(
        (bytes) => !cancelled && setRead({ kind: "ready", bytes }),
        (error) => !cancelled && setRead({ kind: "failed", reason: formatErrorMessage(error) }),
      );
    }
    return () => {
      cancelled = true;
    };
  }, [name, readable]);

  const parsed = length === null ? null : parseHexBytes(draft, length);
  const canWrite = writable && parsed !== null && parsed.ok && !busy;

  const write = async () => {
    if (!parsed?.ok) return;
    setBusy(true);
    try {
      await invokeCommand("write_register", { nodeName: name, data: parsed.bytes });
      addToast("success", `Wrote ${parsed.bytes.length} bytes to ${name}`);
      if (readable) await refresh();
    } catch (error) {
      addToast("error", `Writing ${name} failed: ${formatErrorMessage(error)}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="editor register-editor">
      <div className="register-editor__meta">
        <span>{length === null ? "Length unknown" : `${length} bytes`}</span>
        {readable && (
          <button
            type="button"
            className="btn--ghost"
            disabled={busy}
            onClick={() => void refresh()}
          >
            Refresh
          </button>
        )}
        {readable && read.kind === "ready" && writable && (
          <button type="button" className="btn--ghost" onClick={() => setDraft(toHexText(read.bytes))}>
            Copy to editor
          </button>
        )}
      </div>
      {!readable && (
        <p className="editor__hint">
          {access === "WO" ? "Write-only register: its bytes cannot be read." : "The register cannot be read now."}
        </p>
      )}
      {readable && read.kind === "loading" && <p className="editor__hint">Reading…</p>}
      {readable && read.kind === "failed" && (
        <p className="editor__hint register-editor__error" role="alert">
          {read.reason}
        </p>
      )}
      {readable && read.kind === "ready" && <HexView bytes={read.bytes} />}

      {writable ? (
        <>
          <label className="editor__label" htmlFor={`register-${name}`}>
            New bytes (hex)
          </label>
          <textarea
            id={`register-${name}`}
            className="editor__input register-editor__input"
            spellCheck={false}
            rows={Math.min(8, Math.max(2, Math.ceil((length ?? 16) / 16)))}
            value={draft}
            placeholder={length === null ? "" : `${length} bytes, e.g. 00 01 02 …`}
            onChange={(e) => setDraft(e.target.value)}
          />
          {draft.trim() !== "" && parsed && !parsed.ok && (
            <p className="editor__hint register-editor__error">{parsed.error}</p>
          )}
          <button
            type="button"
            className="btn editor__action"
            disabled={!canWrite}
            onClick={() => void write()}
          >
            Write
          </button>
        </>
      ) : (
        <p className="editor__hint">
          {liveState.is_available ? "Read-only now: the register cannot be written." : "Not available in the camera's current state."}
        </p>
      )}
    </div>
  );
}

function HexView({ bytes }: { bytes: number[] }) {
  if (bytes.length === 0) return <p className="editor__hint">The register is empty.</p>;
  const rows = hexRows(bytes);
  return (
    <>
      <pre className="register-editor__dump" aria-label="Register bytes">
        {rows.map((row) => `${row.offset}  ${row.hex.join(" ").padEnd(47)}  ${row.ascii}`).join("\n")}
      </pre>
      {bytes.length > MAX_DISPLAY_BYTES && (
        <p className="editor__hint">
          Showing the first {MAX_DISPLAY_BYTES} of {bytes.length} bytes; Copy to editor takes them all.
        </p>
      )}
    </>
  );
}

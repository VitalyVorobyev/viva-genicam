import { useEffect, useState } from "react";
import { useToast } from "../../context/ToastContext";
import { invokeCommand } from "../../device/commands";
import { formatErrorMessage } from "../../device/errorMessage";
import { useFeature } from "../../device/FeatureStoreContext";
import type { ActionCommandResult, FeatureState } from "../../device/types";
import { isTauri } from "../../tauri";

/** How long acknowledgements are collected unless the user says otherwise. */
const DEFAULT_TIMEOUT_MS = "200";

const HEX_32 = /^(0x)?[0-9a-f]{1,8}$/i;
const TICKS = /^(0x[0-9a-f]{1,16}|[0-9]{1,20})$/i;
const IPV4 = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/;

function isIpv4(text: string): boolean {
  const match = IPV4.exec(text.trim());
  return match !== null && match.slice(1).every((part) => Number(part) <= 255);
}

/** A key the camera reports, as the form shows one; `undefined` without. */
function keyOf(state: FeatureState | undefined): string | undefined {
  if (typeof state?.value !== "number") return undefined;
  return `0x${(state.value >>> 0).toString(16).toUpperCase().padStart(8, "0")}`;
}

type Destination =
  | { kind: "loading" }
  | { kind: "ready"; address: string }
  | { kind: "unavailable"; reason: string };

/**
 * A GigE Vision action command, as `viva-camctl action` sends it (backlog
 * ST-54). It is addressed by keys, not by camera: every device the address
 * reaches compares the keys and group mask with its own, and those that
 * match act. Embedded mode only.
 */
export function ActionCommandPanel() {
  const [open, setOpen] = useState(false);
  return (
    <section className="tool-panel" aria-label="Action command">
      <div className="tool-panel__header">
        <span className="diagnostics-tab__title">Action command</span>
        <button
          type="button"
          className="btn--ghost"
          aria-expanded={open}
          onClick={() => setOpen((o) => !o)}
        >
          {open ? "Close" : "Send an action command…"}
        </button>
      </div>
      {open && <ActionForm />}
    </section>
  );
}

function ActionForm() {
  const [destination, setDestination] = useState<Destination>({ kind: "loading" });

  useEffect(() => {
    if (!isTauri()) return;
    let cancelled = false;
    invokeCommand("default_action_destination").then(
      (address) => !cancelled && setDestination({ kind: "ready", address }),
      (error) => !cancelled && setDestination({ kind: "unavailable", reason: formatErrorMessage(error) }),
    );
    return () => {
      cancelled = true;
    };
  }, []);

  if (destination.kind === "loading") return null;
  if (destination.kind === "unavailable") {
    return <p className="device-panel__note">{destination.reason}</p>;
  }
  return <ActionFields defaultDestination={destination.address} />;
}

function ActionFields({ defaultDestination }: { defaultDestination: string }) {
  const { addToast } = useToast();
  // What the connected camera holds now, if it says: a key is often
  // write-only, and the group key and mask are those `ActionSelector` selects.
  const deviceKeyState = useFeature("ActionDeviceKey");
  const groupKeyState = useFeature("ActionGroupKey");
  const groupMaskState = useFeature("ActionGroupMask");
  // `undefined` until the user edits a field: until then it shows the
  // camera's value, or the default.
  const [deviceKey, setDeviceKey] = useState<string>();
  const [groupKey, setGroupKey] = useState<string>();
  const [groupMask, setGroupMask] = useState<string>();
  const [scheduledTime, setScheduledTime] = useState("");
  const [destination, setDestination] = useState<string>();
  const [timeoutMs, setTimeoutMs] = useState(DEFAULT_TIMEOUT_MS);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<ActionCommandResult | null>(null);

  const values = {
    deviceKey: deviceKey ?? keyOf(deviceKeyState) ?? "",
    groupKey: groupKey ?? keyOf(groupKeyState) ?? "",
    groupMask: groupMask ?? keyOf(groupMaskState) ?? "",
    destination: destination ?? defaultDestination,
  };
  const timeout = Number(timeoutMs);
  const valid =
    HEX_32.test(values.deviceKey.trim()) &&
    HEX_32.test(values.groupKey.trim()) &&
    HEX_32.test(values.groupMask.trim()) &&
    (scheduledTime.trim() === "" || TICKS.test(scheduledTime.trim())) &&
    isIpv4(values.destination) &&
    /^\d+$/.test(timeoutMs.trim()) &&
    timeout <= 10_000;
  const writeOnlyKey = deviceKeyState?.is_implemented === true && deviceKeyState.value === null;

  const send = async () => {
    setBusy(true);
    try {
      const sent = await invokeCommand("send_action_command", {
        destination: values.destination.trim(),
        deviceKey: values.deviceKey.trim(),
        groupKey: values.groupKey.trim(),
        groupMask: values.groupMask.trim(),
        scheduledTime: scheduledTime.trim() === "" ? null : scheduledTime.trim(),
        timeoutMs: timeout,
      });
      setResult(sent);
    } catch (error) {
      addToast("error", `Action command failed: ${formatErrorMessage(error)}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <p className="device-panel__note" role="note">
        An action command is broadcast: every device the address reaches whose keys and group
        mask match acts on it, not only the connected camera.
      </p>
      <form
        className="action-form"
        onSubmit={(e) => {
          e.preventDefault();
          if (valid && !busy) void send();
        }}
      >
        <Field label="Device key (hex)" value={values.deviceKey} onChange={setDeviceKey} placeholder="0x00000000" />
        <Field label="Group key (hex)" value={values.groupKey} onChange={setGroupKey} placeholder="0x00000000" />
        <Field label="Group mask (hex)" value={values.groupMask} onChange={setGroupMask} placeholder="0xFFFFFFFF" />
        <Field
          label="Scheduled time (ticks)"
          value={scheduledTime}
          onChange={setScheduledTime}
          placeholder="now"
        />
        <Field label="Destination" value={values.destination} onChange={setDestination} placeholder="255.255.255.255" />
        <Field label="Timeout (ms)" value={timeoutMs} onChange={setTimeoutMs} placeholder={DEFAULT_TIMEOUT_MS} />
        <span />
        <button type="submit" className="btn btn--sm editor__action" disabled={!valid || busy}>
          {busy ? "Sending…" : "Send"}
        </button>
      </form>
      {writeOnlyKey && deviceKey === undefined && (
        <p className="device-panel__note">
          ActionDeviceKey is write-only on this camera: enter the key it was given.
        </p>
      )}
      {result && <Result result={result} />}
    </>
  );
}

function Field({
  label,
  value,
  onChange,
  placeholder,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  placeholder: string;
}) {
  return (
    <label style={{ display: "contents" }}>
      <span>{label}</span>
      <input
        type="text"
        className="iv-text-input"
        spellCheck={false}
        value={value}
        placeholder={placeholder}
        onChange={(e) => onChange(e.target.value)}
      />
    </label>
  );
}

function Result({ result }: { result: ActionCommandResult }) {
  return (
    <>
      <p className="tool-panel__result" role="status">
        Sent to {result.destination}: acknowledged by {result.acks} device{result.acks === 1 ? "" : "s"}.
      </p>
      {result.refusals.length > 0 && (
        <ul className="tool-panel__list tool-panel__list--warning" aria-label="Refusals">
          {result.refusals.map((refusal) => (
            <li key={refusal.source}>
              {refusal.source} refused: {refusal.status}
            </li>
          ))}
        </ul>
      )}
    </>
  );
}

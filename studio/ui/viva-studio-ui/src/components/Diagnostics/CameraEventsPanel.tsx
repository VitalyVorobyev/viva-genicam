import { useEffect, useState } from "react";
import { useToast } from "../../context/ToastContext";
import { useCameraEvents } from "../../device/CameraEventsContext";
import { invokeCommand } from "../../device/commands";
import { formatErrorMessage } from "../../device/errorMessage";
import { useCameraReadOnly } from "../../device/FeatureStoreContext";
import type { CameraEvent } from "../../device/types";
import { isTauri } from "../../tauri";

type Choices =
  | { kind: "loading" }
  | { kind: "ready"; entries: string[] }
  | { kind: "unavailable"; reason: string };

/**
 * The camera's own events — ExposureEnd and the rest, GigE Vision and USB3
 * Vision alike — each with its device timestamp and the features its XML
 * reads from it (backlog ST-51). Starting changes the camera's event
 * settings; stopping, disconnecting and closing Studio put them back, and
 * what could not be put back is shown here.
 */
export function CameraEventsPanel({ connected }: { connected: boolean }) {
  if (!connected) return null;
  return <EventsPanel />;
}

function EventsPanel() {
  const { addToast } = useToast();
  const readOnly = useCameraReadOnly();
  const [choices, setChoices] = useState<Choices>({ kind: "loading" });
  const { status, setStatus, received, clear, chosen, setChosen } = useCameraEvents();
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!isTauri()) return;
    let cancelled = false;
    invokeCommand("list_camera_events").then(
      (entries) => !cancelled && setChoices({ kind: "ready", entries }),
      (error) => !cancelled && setChoices({ kind: "unavailable", reason: formatErrorMessage(error) }),
    );
    return () => {
      cancelled = true;
    };
  }, []);

  const run = async (start: boolean) => {
    setBusy(true);
    try {
      setStatus(
        start
          ? await invokeCommand("start_camera_events", { events: chosen })
          : await invokeCommand("stop_camera_events"),
      );
    } catch (error) {
      addToast("error", `${start ? "Starting" : "Stopping"} events failed: ${formatErrorMessage(error)}`);
    } finally {
      setBusy(false);
    }
  };

  const toggle = (entry: string, on: boolean) =>
    setChosen((list) => (on ? [...list, entry] : list.filter((e) => e !== entry)));

  return (
    <section className="events-panel" aria-label="Camera events">
      <div className="events-panel__header">
        <span className="diagnostics-tab__title">Camera events</span>
        <span className="events-panel__state" role="status">
          {status.active ? `Monitoring ${status.events.join(", ")}` : "Not monitoring"}
        </span>
      </div>
      {choices.kind === "unavailable" && (
        <p className="device-panel__note">Events: {choices.reason}</p>
      )}
      {choices.kind === "ready" && choices.entries.length === 0 && (
        <p className="device-panel__note">
          This camera declares no EventSelector, so it has no events to turn on.
        </p>
      )}
      {choices.kind === "ready" && choices.entries.length > 0 && (
        <>
          <fieldset className="events-panel__choices" disabled={status.active || busy}>
            <legend className="events-panel__legend">Events to monitor</legend>
            {choices.entries.map((entry) => (
              <label key={entry} className="report-panel__option">
                <input
                  type="checkbox"
                  checked={chosen.includes(entry)}
                  onChange={(e) => toggle(entry, e.target.checked)}
                />
                {entry}
              </label>
            ))}
          </fieldset>
          <div className="report-panel__actions">
            <button
              type="button"
              className="btn btn--sm"
              disabled={busy || readOnly || status.active || chosen.length === 0}
              onClick={() => void run(true)}
            >
              Start
            </button>
            <button
              type="button"
              className="btn--ghost"
              disabled={busy || !status.active}
              onClick={() => void run(false)}
            >
              Stop
            </button>
            <button
              type="button"
              className="btn--ghost"
              disabled={received.length === 0}
              onClick={clear}
            >
              Clear
            </button>
            {received.length > 0 && <span className="diagnostics-tab__count">{received.length}</span>}
          </div>
          {readOnly && (
            <p className="device-panel__note">Read-only session: event settings cannot be changed.</p>
          )}
        </>
      )}
      {status.warnings.length > 0 && (
        <ul className="events-panel__warnings" role="alert" aria-label="Event settings warnings">
          {status.warnings.map((warning) => (
            <li key={warning}>{warning}</li>
          ))}
        </ul>
      )}
      {received.length > 0 && (
        <ol className="events-panel__list" aria-label="Received camera events">
          {received.map((event) => (
            <EventRow key={event.key} event={event} />
          ))}
        </ol>
      )}
    </section>
  );
}

function EventRow({ event }: { event: CameraEvent }) {
  return (
    <li className="events-panel__event">
      <details>
        <summary>
          <span className="events-panel__name">{event.name ?? "Unnamed event"}</span>
          <span className="events-panel__id">{event.event_id}</span>
          <span className="events-panel__time" title="Raw device clock ticks; these are not seconds.">
            ticks {event.ts_dev}
          </span>
          <span
            className="events-panel__time"
            title="Device time mapped to Unix nanoseconds by clock calibration. Never the event's receipt time."
          >
            {event.ts_host_ns === null ? "host time not calibrated" : `host ${event.ts_host_ns} ns`}
          </span>
          {event.block_id !== null && <span className="events-panel__time">block {event.block_id}</span>}
        </summary>
        <dl className="device-panel__info events-panel__features">
          {event.stream_channel !== null && (
            <>
              <dt>Stream channel</dt>
              <dd>{event.stream_channel}</dd>
            </>
          )}
          {event.features.length === 0 && (
            <>
              <dt>Features</dt>
              <dd>The camera&apos;s XML reads no features from this event.</dd>
            </>
          )}
          {event.features.map((feature, index) => (
            <FeatureRow key={`${feature.name}:${index}`} name={feature.name} value={feature.value} error={feature.error} />
          ))}
        </dl>
      </details>
    </li>
  );
}

function FeatureRow({ name, value, error }: { name: string; value: string | null; error: string | null }) {
  return (
    <>
      <dt>{name}</dt>
      <dd className={error ? "events-panel__error" : undefined}>{error ? `Error: ${error}` : value ?? "Unavailable"}</dd>
    </>
  );
}

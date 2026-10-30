import { useCallback, useEffect, useRef } from "react";
import { useAppLog } from "../../context/AppLogContext";
import { ActionCommandPanel } from "./ActionCommandPanel";
import { CameraEventsPanel } from "./CameraEventsPanel";
import { ConfigurationPanel } from "./ConfigurationPanel";
import { DevicePanel } from "./DevicePanel";
import { ReportPanel } from "./ReportPanel";

// DiagnosticsTab renders the camera, its configuration file, its events,
// action commands, the report, and Studio's own timestamped activity log from
// AppLogContext.
// The log auto-scrolls to the newest entry (top, since entries are newest-first).
export function DiagnosticsTab({ connected, acquiring = false }: { connected: boolean; acquiring?: boolean }) {
  const { entries, clear } = useAppLog();
  const listRef = useRef<HTMLDivElement | null>(null);

  // Scroll to top whenever a new entry is added (newest is at top).
  useEffect(() => {
    if (listRef.current) {
      listRef.current.scrollTop = 0;
    }
  }, [entries.length]);

  const formatTime = useCallback((date: Date): string => {
    const hh = String(date.getHours()).padStart(2, "0");
    const mm = String(date.getMinutes()).padStart(2, "0");
    const ss = String(date.getSeconds()).padStart(2, "0");
    const ms = String(date.getMilliseconds()).padStart(3, "0");
    return `${hh}:${mm}:${ss}.${ms}`;
  }, []);

  return (
    <div className="diagnostics-tab">
      <DevicePanel connected={connected} />
      <ConfigurationPanel connected={connected} acquiring={acquiring} />
      <CameraEventsPanel connected={connected} />
      <ActionCommandPanel />
      <ReportPanel />
      <div className="diagnostics-tab__toolbar">
        <span className="diagnostics-tab__title">Activity Log</span>
        <div className="diagnostics-tab__actions">
          {entries.length > 0 && (
            <span className="diagnostics-tab__count">{entries.length}</span>
          )}
          <button
            type="button"
            className="btn--ghost"
            onClick={clear}
            disabled={entries.length === 0}
            title="Clear log"
          >
            Clear
          </button>
        </div>
      </div>

      <div className="diagnostics-tab__log" ref={listRef}>
        {entries.length === 0 ? (
          <div className="diagnostics-tab__empty">
            No activity yet. Entries appear here when you connect to devices,
            apply node values, or start acquisition.
          </div>
        ) : (
          entries.map((entry) => (
            <div key={entry.id} className="log-entry">
              <span className="log-entry__ts">{formatTime(entry.ts)}</span>
              <span className={`log-entry__level log-entry__level--${entry.level}`}>
                {entry.level}
              </span>
              <div className="log-entry__body">
                <div className="log-entry__message">{entry.message}</div>
                {entry.node && (
                  <div className="log-entry__node">{entry.node}</div>
                )}
              </div>
            </div>
          ))
        )}
      </div>
    </div>
  );
}

import { useCallback, useEffect, useRef } from "react";
import { useAppLog } from "../../context/AppLogContext";
import { DevicePanel } from "./DevicePanel";
import { ReportPanel } from "./ReportPanel";

// DiagnosticsTab renders the timestamped event log from AppLogContext.
// The log auto-scrolls to the newest entry (top, since entries are newest-first).
export function DiagnosticsTab({ connected }: { connected: boolean }) {
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
      <ReportPanel />
      <div className="diagnostics-tab__toolbar">
        <span className="diagnostics-tab__title">Event Log</span>
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
            No events yet. Events appear here when you connect to devices, apply
            node values, or start acquisition.
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

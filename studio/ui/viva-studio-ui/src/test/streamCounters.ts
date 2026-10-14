import type { StreamCounters } from "../device/types";

/** A stream's counters, all 0 but `overrides`. */
export function counters(overrides: Partial<StreamCounters> = {}): StreamCounters {
  return {
    frames_complete: 0,
    frames_incomplete: 0,
    frames_missed: 0,
    frames_backpressure: 0,
    frames_unsupported: 0,
    packets_received: 0,
    packets_missing: 0,
    packets_duplicate: 0,
    packets_ignored: 0,
    resend_requested: 0,
    resend_recovered: 0,
    resend_unavailable: 0,
    transfer_errors: 0,
    bytes: 0,
    avg_mbps: 0,
    avg_latency_ms: null,
    elapsed_ms: 0,
    ...overrides,
  };
}

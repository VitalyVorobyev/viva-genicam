# ADR-0036: Frame metadata travels with its pixels

**Status:** Accepted
**Date:** 2026-10-28

## Context

Studio exposed image geometry and aggregate acquisition counters, but not the
frame's device clock or chunk values. A separate latest-value metadata topic
can be delivered before or after another frame and cannot establish which
image its values describe. Native analysis already requires one immutable
capture (ADR-0034). High-rate acquisition must keep draining frames even when
the viewer skips them.

## Decision

- Zenoh API 8 image packets use frame-header version 3: a 24-byte fixed prefix,
  bounded JSON frame metadata, then image pixels. Geometry, stride, transport
  block ID, timestamps and chunk values describe that same frame. Event-vision
  framing is unchanged. Services and Studio must be upgraded together.
- Device ticks and frame/chunk identifiers are decimal strings. Calibrated
  host Unix nanoseconds are an optional signed decimal string; missing
  calibration is never replaced by reception time.
- Shared wire types limit JSON to 64 KiB, chunk/feature lists to 128 entries
  and descriptive text to 1,024 Unicode characters. Truncation and read errors
  are visible. Malformed remote envelopes are rejected before exposing pixels.
- Plain frames derive metadata directly from their frame fields with no new
  camera lock or feature reads. Chunk mode remains an explicit camera setting.
  Services evaluate chunk features only on frames carrying chunks; embedded
  Studio evaluates them only after preview admission. Chunk-enabled remote
  streams incur GenApi evaluation and a camera mutex on each chunk frame.
- The local preview carries metadata and BMP in one bounded `VFRM` message.
  Source pixels, BMP and metadata are retained together for native capture.
  The UI exposes metadata only after the matching preview has painted and
  rejects late decoding from a previous camera/session. Frozen captures keep
  their own metadata until Resume.
- Chunk controls reuse the standard feature cache, ranges and access ceiling.
  Connecting, recovering or starting acquisition does not enable chunks or
  replay chunk configuration.

## Consequences

**Positive:** Live and frozen metadata cannot drift onto unrelated pixels;
large timestamps remain exact in JavaScript. Both transports and both Studio
modes share one wire model and the existing GenApi interpretation of chunks.

**Negative:** The image wire format changes. JSON adds a small per-frame cost,
and evaluating chunk features adds work for explicitly chunk-enabled streams.
Metadata beyond the documented bounds is omitted with an indication rather
than retaining unbounded camera-supplied data.

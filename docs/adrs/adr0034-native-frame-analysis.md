# ADR-0034: Native frame analysis from one immutable capture

**Status:** Accepted
**Date:** 2026-10-25

## Context

Studio's live BMP preview reduces camera samples to eight bits. Using those
preview bytes for a pixel inspector, histogram, line profile or snapshot loses
the camera's original values (ST-39). Fetching raw pixels separately while the
preview keeps advancing can also measure a different frame from the one shown.

## Decision

- Keep the existing BMP WebSocket unchanged. Both embedded and remote modes
  retain one source corresponding to a displayed frame, scoped to that
  acquisition's server. The embedded path carries the frame's row stride.
- Add an on-demand `/native` route beside the WebSocket, only on loopback-bound
  listeners. Renderer-origin checks prevent other browser origins from reading
  captures; they are not authentication of arbitrary local programs.
- Return a versioned binary envelope: `VRAW`, little-endian u32 metadata length,
  UTF-8 JSON metadata, then BMP bytes, canonical sample bytes, and PNG bytes.
  Metadata identifies geometry, source PFNC name, significant bits, channels,
  Bayer-mosaic status, and each segment's byte count. Pixel arrays stay binary.
- Convert only on request, on the blocking pool. `viva-pfnc` owns decoding;
  TypeScript reads canonical u8 or little-endian u16 samples, never PFNC packing.
  Bayer inspection uses the original sensor mosaic; the PNG follows the
  library's established full-depth export, including demosaicing and sBIT.
- **Freeze for analysis** displays the envelope's own BMP and uses its samples
  for all measurements. **Resume** returns to live preview. Saving a frozen
  capture uses its included PNG; a live snapshot requests one capture and saves
  that PNG without claiming it is the frame currently on screen.
- Limit captures to 16 Mi pixels, 128 MiB combined retained source bytes and
  256 MiB response bytes, with checked length arithmetic. One conversion/response
  permit bounds concurrent work; malformed or unsupported sources are refused
  without disabling live preview. Changing sessions clears captured state.
- Do not revise the Zenoh wire in this change. Its image header lacks row
  stride, so remote native capture is limited to unambiguous complete tight
  rows with the exact advertised geometry length. Ambiguous packed rows and
  extra payload/padding are refused for native analysis; embedded mode remains
  able to use explicit row stride and converter-supported partial final groups.

## Consequences

**Positive:** displayed pixels, measurements and saved data come from one
immutable frame. The same PFNC and PNG implementations serve Rust and Studio,
and the live-preview protocol remains compatible. No additional full-depth
conversion runs per incoming frame.

**Negative:** accurate analysis is explicitly frozen, not a continuously
updating plot. Retaining the latest source costs one bounded frame; the remote
path may copy its payload. Bayer PNG export represents demosaiced colour,
whereas the inspector measures sensor samples. Remote padding requires a future
versioned per-frame layout contract (SR-26, alongside ST-52), rather than inference from
payload size. Real camera validation remains a separate gate after the ongoing
endurance run; pure synthetic captures do not establish hardware coverage.

## 2026-10-25 amendment: explicit remote row layout (SR-26)

API 7 introduced a version-2, 20-byte image header with a per-frame row stride.
[ADR-0036](adr0036-frame-metadata-with-pixels.md) supersedes that envelope with
API 8 / version 3, retaining the stride and adding inline frame metadata.
Zero means continuous packing; nonzero stride includes padding after every
row, including the final row. Services preserve the padded image, and both
preview and native capture use the declared stride through the shared PFNC
converter. This replaces the initial remote tight-row restriction above.
Version-1 image headers are rejected, so services and clients update together.
The wire layout is specified in [the Zenoh contract](../studio/zenoh-api.md).
That change did not carry frame timestamps or decoded chunks; ADR-0036 adds
both while retaining the matching-source capture rule.

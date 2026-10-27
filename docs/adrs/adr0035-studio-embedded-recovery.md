# ADR-0035: Embedded Studio recovery preserves identity, not acquisition

**Status:** Accepted
**Date:** 2026-10-27

## Context

The remote backend already replaces a lost camera session and refreshes the
frontend model. The embedded backend did not observe control loss or terminal
stream errors, so a disconnected camera could leave a frozen viewer. Recovery
also races with explicit disconnect, switching cameras and application exit.

A camera may control equipment as well as an image sensor. Reconnecting it
does not establish that resuming acquisition or replaying its old settings is
appropriate. The maintainer explicitly chose manual Start after recovery.

## Decision

- One embedded lifecycle coordinator serializes session transitions. A new
  user operation invalidates the previous generation and cancels its recovery;
  an obsolete task cannot publish a camera, model or connection state.
- GigE reachability and control ownership are separate observations. A bounded
  uncached control-channel read checks an owned session, and a bootstrap read
  checks an already read-only session. The facade's read-only
  `refresh_control_state` updates ownership and its watcher, including an
  exclusive controller's `ACCESS_DENIED`, without attempting a claim. Camera
  I/O stays on the blocking pool.
- An unreachable GigE camera is reopened by MAC through the facade's existing
  bounded recovery recipe, with 30 seconds of patience. Replacement identity,
  XML, model and selected interface belong to one new session. An unknown-MAC
  session cannot establish device identity for automatic recovery.
- Recovery writes no saved configuration and never resumes acquisition. The
  reopened controller stops any acquisition left running during the outage
  before it is published. The user inspects the refreshed controls and starts
  explicitly.
- A reachable camera whose control was lost remains read-only. Studio offers
  explicit reconnection rather than automatically competing for control.
- USB stream disconnection tears down its reader and offers manual reconnect;
  automatic matching of a replacement USB device is not part of this decision.
- Normal Stop and finite acquisition completion are not device loss.

## Consequences

**Positive:** Recovery follows the library's identity and retry rules, and the
same frontend replacement path serves both operating modes. Cancellation
prevents a delayed reconnect from undoing the user's choice.

**Negative:** Acquisition requires another click after recovery, and camera
settings changed by a reset remain changed. Idle GigE reachability checks add
bounded control-channel reads. USB recovery remains manual.

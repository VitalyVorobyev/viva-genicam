# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Changes that are merged but not yet released are listed under **Unreleased**
below. Each release series has its own file in [`changelog/`](changelog/); a
release opens with what to read before upgrading.

## [Unreleased]

Read before upgrading:
- **Zenoh API version 9.** Camera events need a version 9 service and Studio;
  upgrade `viva-service`, `viva-service-u3v` and Viva Studio together. Images
  are unchanged.
- **`viva-service`:** a `StreamSource` implements `open_events`.

### Added

- **Restorable event setup.** `Camera::configure_events_restorable` turns
  events on and returns an `EventConfiguration`; `Camera::restore_events` puts
  back exactly the notifications, selectors and message channel it changed,
  and its `EventRestoreReport` names whatever could not be restored — a lost
  camera, lost control, a refused write — instead of passing it off as clean.
  `Camera::event_name` names an event by its `EventSelector` entry
  ([#PR]).
- **Viva Studio: camera events.** The Diagnostics tab's Camera events panel
  turns on the events you choose and lists each as it arrives, with its device
  timestamp and the values the camera's XML reads from it; Stop, disconnecting
  and closing Studio restore the camera's event settings, and a setting that
  could not be restored is shown ([#PR]).
- **Camera events over Zenoh.** `viva-service` and `viva-service-u3v` start and
  stop event monitoring on `events/control`, publish each event as a
  `CameraEvent` on `events` and the monitoring status on `events/status`
  (API version 9, [#PR]).

### Changed

- **A USB3 Vision event stream restores the EventEnable word it found** when
  it closes, rather than clearing it, and `U3vEventStream::shutdown` waits for
  its reader and reports a restoration that failed ([#PR]).
- **Enabling an event that cannot be enabled reports the GenApi error**, such
  as the entry `EventSelector` does not have, rather than a generic
  "no event selection" ([#PR]).

[#PR]: https://github.com/VitalyVorobyev/viva-genicam/pull/PR

## Releases

| Series | Releases | File |
|--------|----------|------|
| 0.11 | 0.11.0 (2026-10-30) | [changelog/0.11.md](changelog/0.11.md) |
| 0.10 | 0.10.0 (2026-10-17) | [changelog/0.10.md](changelog/0.10.md) |
| 0.9 | 0.9.0 (2026-10-13) | [changelog/0.9.md](changelog/0.9.md) |
| 0.8 | 0.8.0 (2026-10-10) | [changelog/0.8.md](changelog/0.8.md) |
| 0.7 | 0.7.0 (2026-09-30) | [changelog/0.7.md](changelog/0.7.md) |
| 0.6 | 0.6.0 (2026-09-24) | [changelog/0.6.md](changelog/0.6.md) |
| 0.5 | 0.5.0 (2026-08-26) | [changelog/0.5.md](changelog/0.5.md) |
| 0.4 | 0.4.1 (2026-08-03), 0.4.0 (2026-07-31) | [changelog/0.4.md](changelog/0.4.md) |
| 0.3 | 0.3.1 (2026-07-31), 0.3.0 (2026-07-30) | [changelog/0.3.md](changelog/0.3.md) |
| 0.2 | 0.2.8 (2026-07-28), 0.2.7 (2026-07-28), 0.2.6 (2026-07-25), 0.2.5 (2026-04-15), 0.2.4 (2026-04-13), 0.2.3 (2026-04-12), 0.2.2 (2026-04-12), 0.2.1 (2026-04-12), 0.2.0 (2026-04-11) | [changelog/0.2.md](changelog/0.2.md) |
| 0.1 | 0.1.0 (2026-04-10) | [changelog/0.1.md](changelog/0.1.md) |

[Unreleased]: https://github.com/VitalyVorobyev/viva-genicam/compare/v0.11.0...HEAD

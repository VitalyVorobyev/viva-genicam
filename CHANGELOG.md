# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Changes that are merged but not yet released are listed under **Unreleased**
below. Each release series has its own file in [`changelog/`](changelog/); a
release opens with what to read before upgrading.

## [Unreleased]

### Read before upgrading

- **Commands write the value their XML declares.** `<pCommandValue>` was
  ignored, so every such command wrote `1` (19 of 38 corpus documents). On a
  Hikrobot MV-CS050 with `UserSet2` selected, `UserSetSave` overwrote user
  set 1; Point Grey `UserSetSave` expects a key such as `0x80000000`. ([#193](https://github.com/VitalyVorobyev/viva-genicam/pull/193))
- **Writes are checked against the limits the camera declares**, including
  `<pMin>`/`<pMax>`/`<pInc>`, before being forwarded through `<pValue>` — most
  `Width`/`Height`/`Offset` features were never checked. A value outside them
  now fails with `Range` before it reaches the camera. ([#193](https://github.com/VitalyVorobyev/viva-genicam/pull/193))

### Added

- `Camera::execute_and_wait`, `Camera::is_done` and `NodeMap::is_done`
  (GenICam `IsDone`), so a caller can wait for a command such as `UserSetSave`
  to finish instead of sleeping. `viva-camctl execute` and Python's
  `Camera.execute` wait by default (`--no-wait`/`--wait-ms`;
  `wait=`/`timeout=`); a camera that has not reported completion in time is a
  warning there, not an error, since the command was sent. ([#193](https://github.com/VitalyVorobyev/viva-genicam/pull/193))

### Changed

- A command checks its own `pIsImplemented`/`pIsAvailable`/`pIsLocked` before
  writing, on every addressing path. ([#193](https://github.com/VitalyVorobyev/viva-genicam/pull/193))
- `<pSelected>` is read as the standard defines it, on the selector: writing a
  selector refreshes the features it selects. The non-standard per-node
  `<Selected>` addressing is removed. ([#193](https://github.com/VitalyVorobyev/viva-genicam/pull/193))
- `feature_state` reports dynamic limits, a delegating feature's inherited
  ones, and a converter's range in the order its `<Slope>` gives. ([#193](https://github.com/VitalyVorobyev/viva-genicam/pull/193))
- XML model (consumed by Viva Studio): `Integer`/`Float` bounds are `Option`;
  `Integer` gains `p_inc` and `selected`, `Float` gains `p_min`/`p_max`;
  `Command::command_value` is a `CommandValueSrc`; `Addressing::BySelector`,
  `selectors` and `selected_if` are gone. ([#193](https://github.com/VitalyVorobyev/viva-genicam/pull/193))
- Viva Studio's Linux bundles (AppImage and deb) are built on Ubuntu 24.04, so
  they need glibc 2.39 or newer — Ubuntu 24.04, Debian 13, Fedora 40 or later.
  The Ubuntu 22.04 build image retires in April 2027.
  ([#192](https://github.com/VitalyVorobyev/viva-genicam/pull/192))

### Fixed

- Writes to a floating `<Integer>` (a `<Value>` with no register) always
  failed; 319 corpus features write through one, among them AVT's
  `TriggerSelector` and Sony's `UserSetSelector`. ([#193](https://github.com/VitalyVorobyev/viva-genicam/pull/193))

## Releases

| Series | Releases | File |
|--------|----------|------|
| 0.8 | 0.8.0 (2026-10-10) | [changelog/0.8.md](changelog/0.8.md) |
| 0.7 | 0.7.0 (2026-09-30) | [changelog/0.7.md](changelog/0.7.md) |
| 0.6 | 0.6.0 (2026-09-24) | [changelog/0.6.md](changelog/0.6.md) |
| 0.5 | 0.5.0 (2026-08-26) | [changelog/0.5.md](changelog/0.5.md) |
| 0.4 | 0.4.1 (2026-08-03), 0.4.0 (2026-07-31) | [changelog/0.4.md](changelog/0.4.md) |
| 0.3 | 0.3.1 (2026-07-31), 0.3.0 (2026-07-30) | [changelog/0.3.md](changelog/0.3.md) |
| 0.2 | 0.2.8 (2026-07-28), 0.2.7 (2026-07-28), 0.2.6 (2026-07-25), 0.2.5 (2026-04-15), 0.2.4 (2026-04-13), 0.2.3 (2026-04-12), 0.2.2 (2026-04-12), 0.2.1 (2026-04-12), 0.2.0 (2026-04-11) | [changelog/0.2.md](changelog/0.2.md) |
| 0.1 | 0.1.0 (2026-04-10) | [changelog/0.1.md](changelog/0.1.md) |

[Unreleased]: https://github.com/VitalyVorobyev/viva-genicam/compare/v0.8.0...HEAD

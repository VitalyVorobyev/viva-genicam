# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Changes that are merged but not yet released are listed under **Unreleased**
below. Each release series has its own file in [`changelog/`](changelog/); a
release opens with what to read before upgrading.

## [Unreleased]

### Viva Studio — not part of any published crate

- A profile line or ROI drawn on the image stays on the same image pixels when
  the view is zoomed or panned; it used to stay on the same screen pixels
  ([#209](https://github.com/VitalyVorobyev/viva-genicam/pull/209)).
- The Image tab's control panel is an inspector: a label on the left and one
  aligned control column, number fields with the unit inside and no sliders.
  Arrow keys step a value by the camera's increment, and a typed integer
  snaps to it ([#209](https://github.com/VitalyVorobyev/viva-genicam/pull/209)).
- A control that cannot be changed shows a lock, and says why in its tooltip,
  the same way for every kind ([#209](https://github.com/VitalyVorobyev/viva-genicam/pull/209)).
- The control panel can be resized; its section headers have larger chevrons
  and icons that look the same on every OS; a rail icon opens the panel at
  its section. The frame counter moves to the status bar ([#209](https://github.com/VitalyVorobyev/viva-genicam/pull/209)).
- The Feature Browser's categories column can be resized, and the search box
  shows one clear button instead of two ([#209](https://github.com/VitalyVorobyev/viva-genicam/pull/209)).
- Studio's app icon is the viva-genicam mark ([#209](https://github.com/VitalyVorobyev/viva-genicam/pull/209)).

## Releases

| Series | Releases | File |
|--------|----------|------|
| 0.9 | 0.9.0 (2026-10-13) | [changelog/0.9.md](changelog/0.9.md) |
| 0.8 | 0.8.0 (2026-10-10) | [changelog/0.8.md](changelog/0.8.md) |
| 0.7 | 0.7.0 (2026-09-30) | [changelog/0.7.md](changelog/0.7.md) |
| 0.6 | 0.6.0 (2026-09-24) | [changelog/0.6.md](changelog/0.6.md) |
| 0.5 | 0.5.0 (2026-08-26) | [changelog/0.5.md](changelog/0.5.md) |
| 0.4 | 0.4.1 (2026-08-03), 0.4.0 (2026-07-31) | [changelog/0.4.md](changelog/0.4.md) |
| 0.3 | 0.3.1 (2026-07-31), 0.3.0 (2026-07-30) | [changelog/0.3.md](changelog/0.3.md) |
| 0.2 | 0.2.8 (2026-07-28), 0.2.7 (2026-07-28), 0.2.6 (2026-07-25), 0.2.5 (2026-04-15), 0.2.4 (2026-04-13), 0.2.3 (2026-04-12), 0.2.2 (2026-04-12), 0.2.1 (2026-04-12), 0.2.0 (2026-04-11) | [changelog/0.2.md](changelog/0.2.md) |
| 0.1 | 0.1.0 (2026-04-10) | [changelog/0.1.md](changelog/0.1.md) |

[Unreleased]: https://github.com/VitalyVorobyev/viva-genicam/compare/v0.9.0...HEAD

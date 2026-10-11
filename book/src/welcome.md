# Welcome & Goals

**viva-genicam** provides *pure Rust* building blocks for the GenICam ecosystem supporting **GigE Vision** and **USB3 Vision**, with first-class support for Windows, Linux, and macOS.

## Who is this book for?

People **using** viva-genicam: building a camera application in Rust or
Python, running `viva-camctl`, or using Viva Studio. Working on viva-genicam
itself? The developer documentation lives in the repository — see
[Developing viva-genicam](developers.md).

## What works today
- **GigE Vision**: GVCP discovery; GVSP streaming on a receive thread per stream, tolerant of reordered, repeated and extended-ID packets, with every frame's fate counted; chunk and event features read through the camera's own XML, action commands, FORCEIP, persistent IP configuration. Lost packets are requested again from cameras that support packet resend — so far tested against the fake camera only.
- **USB3 Vision**: device discovery, GenCP register I/O, bulk-endpoint streaming, async frame iterator — see [Status](#status).
- **GenApi**: NodeMap with the standard node types (Integer, Float, Enumeration, Boolean, Command, Category, String, Register, SwissKnife, Converter, IntConverter), pValue delegation, selectors, device, chunk and event ports, runtime access predicates (`pIsLocked`, `pIsAvailable`, `pIsImplemented`), node metadata and visibility filtering.
- **CLI** (`viva-camctl`): discovery, feature get/set, command execution, streaming (including raw event-vision capture), events, chunks, benchmarks, IP configuration, XML dump, and a diagnostic report bundle.
- **Service bridge**: expose cameras over Zenoh for Viva Studio, the desktop app in `studio/`.

## Status

viva-genicam is pre-1.0: the API still changes between releases.

- **GigE Vision**: users have run discovery, control and streaming against real
  cameras from several vendors, on Linux, Windows and macOS.
- **USB3 Vision has never run on a real camera.** It follows the GenCP
  standard and what independent implementations agree on, and is tested
  against a fake built from those sources. If you have a USB3 Vision camera, a
  report from `viva-camctl report --usb 0` is the most useful thing you can
  send ([Reporting a camera we can't open](reporting.md)).
- **Viva Studio is experimental**: it works, and it has been driven against
  very little hardware.
- **There is no camera in CI.** The tests run against built-in fake cameras
  and a corpus of real vendor GenApi XML; hardware confirmations come from
  users with their own devices.

**The full, current list of known gaps is
[issue #191](https://github.com/VitalyVorobyev/viva-genicam/issues/191)**,
updated as fixes land. If your camera hits something that is not on it,
[tell us](reporting.md).

## How this book is organized
- **Getting started**: the Quick Start runs a first discovery; the Primer and
  *How the library fits together* give the big picture, errors and logging
  included.
- **Using the library**: tutorials and networking.
- **Python** and **Viva Studio** each have a part of their own.
- **Help**: FAQ & Troubleshooting, and how to report a camera we cannot open.
- **Reference**: the crates, the API docs, a glossary.

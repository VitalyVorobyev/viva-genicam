# Welcome & Goals

**viva-genicam** provides *pure Rust* building blocks for the GenICam ecosystem supporting **GigE Vision** and **USB3 Vision**, with first-class support for Windows, Linux, and macOS.

## Who is this book for?

People **using** viva-genicam: building a camera application in Rust or
Python, running `viva-camctl`, or using Viva Studio. Working on viva-genicam
itself? The developer documentation lives in the repository — see
[Developing viva-genicam](developers.md).

## What works today
- **GigE Vision**: GVCP discovery, broadcast or of one address, flagging a camera configured for another subnet; control privilege given back when a camera is closed, and read-only monitors that watch a camera — and its multicast stream — while another application controls it; GVSP streaming on a receive thread per stream, tolerant of reordered, repeated and extended-ID packets, with every frame's fate counted; chunk and event features read through the camera's own XML, action commands, FORCEIP, persistent IP configuration. Lost packets are requested again from cameras that support packet resend — so far tested against the fake camera only.
- **USB3 Vision**: discovery without opening the device, GenCP register I/O, streaming with several USB transfers in flight that recovers from a failed transfer and ends with an error of its own when the camera is unplugged, chunks, events, frame timestamps on the host's clock, and a warning when a camera runs below SuperSpeed — see [Status](#status).
- **Camera API**: features read and written by type; helpers for exposure, gain, triggers, region of interest, binning, frame rate and user sets that accept the older names cameras still use (`ExposureTimeAbs`, `GainRaw`, ...); acquisition locks the transport parameters; a camera's configuration saved to a file and restored, selectors and dependencies included; a blocking API for programs without an async runtime.
- **GenApi**: NodeMap with the standard node types (Integer, Float, Enumeration, Boolean, Command, Category, String, Register, SwissKnife, Converter, IntConverter), pValue delegation, selectors, device, chunk and event ports, runtime access predicates (`pIsLocked`, `pIsAvailable`, `pIsImplemented`), node metadata and visibility filtering; values cached only as the XML's `<Cachable>` allows and read again after their `<PollingTime>`, with a query for what changed since a viewer last looked.
- **CLI** (`viva-camctl`): discovery; the feature tree, a feature's full description and every value; get/set, command execution, configuration save and load; raw register access and action commands; streaming (including raw event-vision capture), events, chunks, benchmarks, IP configuration, XML dump, and a diagnostic report bundle — see [the command line](camctl.md).
- **GenTL producer**: `viva_gentl.cti` lets GenTL consumers such as Harvesters use GigE Vision cameras through viva — see [Use viva from other tools](gentl.md).
- **Service bridge**: expose cameras over Zenoh for Viva Studio, the desktop app in `studio/`.

## Status

viva-genicam is pre-1.0: the API still changes between releases.

- **GigE Vision**: users have run discovery, control and streaming against real
  cameras from several vendors, on Linux, Windows and macOS.
- **USB3 Vision has run on one camera**: a MATRIX VISION mvBlueFOX3-2004G, on
  macOS (Apple silicon), over a USB 2 (High Speed) link. Discovery, control,
  streaming, chunks, events and the host clock were checked against it. Not
  yet tried: Linux, Windows, a SuperSpeed link, any other vendor's camera,
  and a transfer that fails in the middle of a stream. Pulling the cable while
  it streams ends the stream as an unplug, checked on macOS. If you have a USB3 Vision
  camera, a report from `viva-camctl report --usb 0` is the most useful thing
  you can send ([Reporting a camera we can't open](reporting.md)).
- **Viva Studio is experimental**: it works, and it has been driven against
  very little hardware.
- **The GenTL producer is new**: Harvesters drives it on every change, against
  the fake camera; no real camera and no other GenTL consumer has been tried
  with it yet ([Use viva from other tools](gentl.md)).
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

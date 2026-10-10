# Welcome & Goals

**viva-genicam** provides *pure Rust* building blocks for the GenICam ecosystem supporting **GigE Vision** and **USB3 Vision**, with first-class support for Windows, Linux, and macOS.

## Who is this book for?

People **using** viva-genicam: building a camera application in Rust or
Python, running `viva-camctl`, or using Viva Studio. Working on viva-genicam
itself? The developer documentation lives in the repository — see
[Developing viva-genicam](developers.md).

## What works today
- **GigE Vision**: GVCP discovery, GVSP streaming with frame reassembly, events, action commands, chunk parsing, FORCEIP, persistent IP configuration.
- **USB3 Vision**: device discovery, GenCP register I/O, bulk-endpoint streaming, async frame iterator. Built to the GenCP standard and to what independent implementations agree on, but **not yet run on a real camera**: if you have one, a report from `viva-camctl report --usb 0` is the most useful thing you can send ([Reporting a problem](reporting.md)).
- **GenApi**: NodeMap with the standard node types (Integer, Float, Enumeration, Boolean, Command, Category, String, Register, SwissKnife, Converter, IntConverter), pValue delegation, selectors, runtime access predicates (`pIsLocked`, `pIsAvailable`, `pIsImplemented`), node metadata and visibility filtering.
- **CLI** (`viva-camctl`): discovery, feature get/set, command execution, streaming (including raw event-vision capture), events, chunks, benchmarks, IP configuration, XML dump, and a diagnostic report bundle.
- **Service bridge**: expose cameras over Zenoh for Viva Studio, the desktop app in `studio/`.

## What does not work yet

Worth knowing before you build on it:

- **Packet resend is not wired in**, so the `resends` statistic is always zero;
  watch `drops` instead. See
  [Streaming → What the statistics do and do not tell you](tutorials/streaming.md#43-what-the-statistics-do-and-do-not-tell-you).
- **The Python bindings are narrower than the Rust API** — no chunks, events
  or action commands. [Python bindings](python.md) lists the gaps.
- **Viva Studio is experimental.** It works, and it has been driven against very
  little hardware.

> **Status.** The protocol implementations follow the published EMVA
> specifications and are tested against built-in fake cameras and a corpus of
> dozens of real vendor GenApi XML descriptions. Users have run discovery,
> control and streaming against real cameras from several vendors on Linux,
> Windows and macOS. There is no camera in CI, though -- hardware confirmations
> come from users with their own devices -- so bug reports and compatibility
> feedback remain the main way this gets better.

## How this book is organized
- **Getting started**: the Quick Start runs a first discovery; the Primer and
  *How the library fits together* give the big picture.
- **Using the library**: tutorials, networking, errors and logging.
- **Python** and **Viva Studio** each have a part of their own.
- **Help**: FAQ & Troubleshooting, and how to report a camera we cannot open.
- **Reference**: the crates, the API docs, a glossary.

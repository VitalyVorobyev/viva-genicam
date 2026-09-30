# Welcome & Goals

**viva-genicam** provides *pure Rust* building blocks for the GenICam ecosystem supporting **GigE Vision** and **USB3 Vision**, with first-class support for Windows, Linux, and macOS.

## Who is this book for?
- **End-users** building camera applications who want a practical high-level API and copy-pasteable examples.
- **Contributors** extending transports, GenApi features, and streaming -- who need a clear mental model of crates and internal boundaries.

## What works today
- **GigE Vision**: GVCP discovery, GVSP streaming with frame reassembly, events, action commands, chunk parsing, FORCEIP, persistent IP configuration.
- **USB3 Vision**: device discovery, GenCP register I/O, bulk-endpoint streaming, async frame iterator.
- **GenApi**: NodeMap with the standard node types (Integer, Float, Enumeration, Boolean, Command, Category, String, Register, SwissKnife, Converter, IntConverter), pValue delegation, selectors, runtime access predicates (`pIsLocked`, `pIsAvailable`, `pIsImplemented`), node metadata and visibility filtering.
- **CLI** (`viva-camctl`): discovery, feature get/set, command execution, streaming (including raw event-vision capture), events, chunks, benchmarks, IP configuration, XML dump, and a diagnostic report bundle.
- **Service bridge**: expose cameras over Zenoh for Viva Studio, the desktop app in `studio/`.

## What does not work yet

Worth knowing before you build on it:

- **Packet resend is not wired in**, so the `resends` statistic is always zero;
  watch `drops` instead. See
  [Streaming → What the statistics do and do not tell you](tutorials/streaming.md#43-what-the-statistics-do-and-do-not-tell-you).
- **The Python bindings are narrower than the Rust API** — no chunks, events,
  time sync or action commands. [Python bindings](python.md) lists the gaps.
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
- Start with **Quick Start** to build, test, and run the first discovery.
- Read the **Primer** and **Architecture** to get the big picture.
- Use **Crates** and **Tutorials** for hands-on tasks.
- See **Networking** and **FAQ & Troubleshooting** when packets don’t behave.

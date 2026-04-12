# Welcome & Goals

**viva-genicam** provides *pure Rust* building blocks for the GenICam ecosystem supporting **GigE Vision** and **USB3 Vision**, with first-class support for Windows, Linux, and macOS.

## Who is this book for?
- **End-users** building camera applications who want a practical high-level API and copy-pasteable examples.
- **Contributors** extending transports, GenApi features, and streaming -- who need a clear mental model of crates and internal boundaries.

## What works today
- **GigE Vision**: GVCP discovery, GVSP streaming with resend and reassembly, events, action commands, chunk parsing, FORCEIP, persistent IP configuration.
- **USB3 Vision**: device discovery, GenCP register I/O, bulk-endpoint streaming, async frame iterator.
- **GenApi**: NodeMap with all standard node types (Integer, Float, Enum, Boolean, Command, Category, String, SwissKnife, Converter), pValue delegation, selectors, node metadata and visibility filtering.
- **CLI** (`viva-camctl`): discovery, feature get/set, streaming, events, chunks, benchmarks, IP configuration.
- **Service bridge**: expose cameras over Zenoh for [genicam-studio](https://github.com/VitalyVorobyev/genicam-studio).

> The protocol implementations follow the published EMVA specifications and are validated against built-in fake camera simulators (190+ automated tests). Testing against physical cameras from different manufacturers is ongoing -- bug reports and compatibility feedback are welcome.

## How this book is organized
- Start with **Quick Start** to build, test, and run the first discovery.
- Read the **Primer** and **Architecture** to get the big picture.
- Use **Crate Guides** and **Tutorials** for hands-on tasks.
- See **Networking** and **Troubleshooting** when packets don’t behave.

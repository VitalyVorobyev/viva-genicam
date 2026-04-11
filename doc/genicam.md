# GenICam Standards: A Practical Introduction

GenICam (Generic Interface for Cameras) is a set of EMVA standards that give industrial cameras a uniform control interface regardless of the physical connection. The same code can discover a GigE Vision camera over Ethernet and a USB3 Vision camera over USB -- the features, names, and access patterns are identical.

This document covers what you need to know to use genicam-rs effectively.

## The problem GenICam solves

Every camera model has hundreds of hardware registers controlling exposure, gain, pixel format, I/O lines, and more. Without a standard, each vendor invents its own register layout and SDK. GenICam eliminates this by requiring every camera to carry a self-description (an XML file) that tells the host software exactly how to read and write its features.

## Key standards

### GenApi -- the feature model

Each camera stores an XML document describing its features: names, types, register addresses, value ranges, dependencies, and formulas. A GenApi library parses this XML into a **node map** -- an in-memory graph of typed feature nodes.

Common node types:

| Type | Example | Description |
|------|---------|-------------|
| Integer | `Width`, `Height` | Integer value with min/max/increment |
| Float | `ExposureTime`, `Gain` | Floating-point value with unit |
| Enumeration | `PixelFormat`, `TriggerMode` | Choice from named entries |
| Boolean | `ChunkModeActive` | On/off toggle |
| Command | `AcquisitionStart` | Trigger an action |
| Category | `ImageFormatControl` | Groups related features |

Features can reference each other. A `SwissKnife` node computes values from formulas. A `Converter` applies linear or polynomial transforms. `pValue` delegation lets a high-level feature (e.g. `ExposureTime`) read from a raw register node transparently.

**In genicam-rs:** `viva-genapi-xml` parses the XML, `viva-genapi` builds the node map and evaluates features.

### GenCP -- the control protocol

GenCP defines how read/write commands are sent to a camera over any transport. It specifies a simple packet format with four operations:

- `ReadRegister` / `WriteRegister` -- access 32-bit registers
- `ReadMem` / `WriteMem` -- access arbitrary memory blocks

Both GigE Vision and USB3 Vision use GenCP for their control channels.

**In genicam-rs:** `viva-gencp` provides transport-agnostic encode/decode for GenCP packets.

### GVCP / GVSP -- GigE Vision protocols

GigE Vision adds two protocols on top of GenCP:

- **GVCP** (Control Protocol) -- UDP-based device discovery, register access, event delivery, and action commands. Cameras listen on port 3956.
- **GVSP** (Streaming Protocol) -- UDP-based image transfer with packet reassembly and resend support.

**In genicam-rs:** `viva-gige` implements both protocols.

### USB3 Vision

USB3 Vision cameras use USB bulk endpoints: one pair for GenCP control, another for image data. Device discovery uses standard USB enumeration with U3V class descriptors. Bootstrap registers (ABRM, SBRM, SIRM) configure the device.

**In genicam-rs:** `viva-u3v` implements the transport layer.

### SFNC -- standard feature names

The Standard Features Naming Convention ensures that common features use the same name across all cameras. `ExposureTime` is always `ExposureTime`, not `Exposure` or `ShutterTime`.

**In genicam-rs:** `viva-sfnc` provides these names as string constants.

### PFNC -- standard pixel formats

The Pixel Format Naming Convention assigns numeric codes to pixel formats. `Mono8` is `0x01080001`, `RGB8` is `0x02180014`, etc.

**In genicam-rs:** `viva-pfnc` provides the `PixelFormat` enum with code-to-name conversion.

## How it fits together

```
Application
    |
    v
viva-genicam (facade)     -- Camera<T>, discovery, streaming, events
    |
    +-- viva-genapi        -- NodeMap, feature evaluation
    |     +-- viva-genapi-xml  -- XML parsing
    |
    +-- viva-gige          -- GigE Vision (GVCP + GVSP)
    |     +-- viva-gencp   -- GenCP packets
    |
    +-- viva-u3v           -- USB3 Vision
    |     +-- viva-gencp   -- GenCP packets (shared)
    |
    +-- viva-sfnc          -- Feature name constants
    +-- viva-pfnc          -- Pixel format tables
```

1. **Discover** -- find cameras on the network (GVCP broadcast) or USB bus
2. **Connect** -- claim control, fetch the GenICam XML, build the node map
3. **Configure** -- read and write features via the node map (which translates to register I/O)
4. **Stream** -- receive image frames (GVSP over UDP or USB bulk transfers)

## Further reading

- [EMVA GenICam website](https://www.emva.org/standards-technology/genicam/) -- official standard documents
- [genicam-rs book](https://vitalyvorobyev.github.io/genicam-rs/) -- tutorials and architecture guide
- [genicam-rs API reference](https://docs.rs/viva-genicam) -- Rust API documentation

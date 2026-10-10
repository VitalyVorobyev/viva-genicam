# How the library fits together

viva-genicam is a stack of crates, each one a layer of the GenICam standards.
Most applications use only the top one, `viva-genicam`; the layers below are
there when you need to reach past it.

## The layers

```
  your application          viva-camctl (CLI)      Python (viva-genicam on PyPI)
          \                        |                      /
           +---------------- viva-genicam ---------------+     discovery, Camera, streams, events
                          /                  \
                 viva-genapi                  transports
           NodeMap: features by name          viva-gige (GigE Vision: GVCP control, GVSP streaming)
                       |                      viva-u3v  (USB3 Vision: GenCP over USB bulk endpoints)
               viva-genapi-xml                         \
           the camera's GenApi XML                    viva-gencp (GenCP / GVCP message primitives)
```

- **`viva-genicam`** — start here. It discovers cameras, opens them, fetches
  their XML, and gives you a `Camera` whose features you read and write by
  name, plus frame streams, events, chunks and time mapping.
- **`viva-genapi`** — the GenApi engine: it turns a feature name such as
  `ExposureTime` into register reads and writes, evaluating formulas,
  selectors and access rules on the way.
- **`viva-genapi-xml`** — parses a camera's GenApi XML, from the camera or from
  a file.
- **`viva-gige`** and **`viva-u3v`** — the two transports.
- **`viva-gencp`** — the control-protocol message formats both build on.

Around the stack: `viva-pfnc` and `viva-sfnc` name pixel formats and standard
features; `viva-service` and `viva-service-u3v` serve cameras over
[Zenoh](https://zenoh.io) for [Viva Studio](studio.md) and other clients;
`viva-fake-gige` and `viva-fake-u3v` are cameras that run in your process
([Testing without hardware](tutorials/fake-camera.md)). The
[Crates](crates/README.md) page says which of them you are likely to need.

## What happens when you open a camera

1. **Discovery** finds cameras: a GVCP broadcast on each network interface, or
   USB enumeration.
2. **Connecting** opens the control channel. For GigE Vision this also takes
   control privilege and starts a heartbeat that keeps it.
3. **The GenApi XML** is read from the camera — its address is in the
   camera's bootstrap registers — and parsed.
4. **The NodeMap** is built from it. A declaration the library cannot build is
   listed in `NodeMap::skipped()` instead of failing the camera.
5. **Feature access** goes through the NodeMap: a direct feature reads and
   writes its registers, a computed one evaluates its formula over others.
6. **Streaming** configures the camera's stream channel and receives frames —
   GVSP packets reassembled over UDP, or USB3 Vision blocks over a bulk
   endpoint — with their timestamps and chunk data.

## Blocking and async

Discovery, stream setup and frame reception are async (Tokio):
`FrameStream::next_frame` is an `async fn`.

Feature access is **synchronous**: `Camera::get` and `Camera::set` block until
the camera answers. That keeps the GenApi engine independent of any runtime,
and it is safe to call them inside `#[tokio::main]` — the GigE transport
bridges onto its async client itself. In a GUI or a server, call them where a
short block is acceptable, such as a blocking-pool task. The reasoning is in
[ADR-0014](https://github.com/VitalyVorobyev/viva-genicam/blob/main/docs/adrs/adr0014-sync-registerio-async-adapters.md).

## Errors and logs

Each layer has its own error type — `GigeError`, `GenCpError`, `XmlError`,
`GenApiError` — and `GenicamError` wraps them at the top
([Error Handling & Logging](errors-logging.md)). Everything logs through
`tracing`; `RUST_LOG=debug` shows the protocol traffic.

## Platforms

Linux, macOS and Windows. Network setup — firewalls, interface selection,
jumbo frames — is in [Networking](networking.md). On a host with several
network interfaces, name the one your camera is on.

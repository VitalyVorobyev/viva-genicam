# Quick Start

From nothing to a list of the cameras on your network.

## Install what you need

- **A Rust application**: add the facade crate and an async runtime. Rust 1.88
  or newer.

  ```bash
  cargo add viva-genicam
  cargo add tokio --features full
  ```

- **The command line**: `viva-camctl` comes with the Python package
  (`pip install viva-genicam`), and in the `viva-genicam-<version>-<platform>`
  archives on each [GitHub release](https://github.com/VitalyVorobyev/viva-genicam/releases),
  next to `viva-service`.
- **Python**: `pip install viva-genicam` — see [Install & hello-camera](python/install.md).
- **A desktop app**: [Viva Studio](studio.md).

For a GigE Vision camera, the host must allow **UDP broadcast** on the network
interface the camera is on; [Networking](networking.md) covers firewalls,
addressing and jumbo frames.

## Find your cameras

From the command line:

```bash
viva-camctl list
# Only on one network interface, named by its address or its OS name:
viva-camctl list --iface 192.168.0.5
```

From Rust, discovery is one call; the snippet comes from the
[`list_cameras`](https://github.com/VitalyVorobyev/viva-genicam/blob/main/crates/viva-genicam/examples/list_cameras.rs)
example:

```rust,ignore
{{#include ../../crates/viva-genicam/examples/list_cameras.rs:discover}}
```

Then connect to one and read or write a feature by name
([`get_set_feature`](https://github.com/VitalyVorobyev/viva-genicam/blob/main/crates/viva-genicam/examples/get_set_feature.rs)):

```rust,ignore
{{#include ../../crates/viva-genicam/examples/get_set_feature.rs:get_set}}
```

From a clone of the repository the examples run as
`cargo run -p viva-genicam --example list_cameras`, and the CLI as
`cargo run -p viva-camctl -- list`.

## Control path: read / write & XML

```bash
# Read a feature by name
viva-camctl get --ip 192.168.0.10 --name ExposureTime

# Set a feature value
viva-camctl set --ip 192.168.0.10 --name ExposureTime --value 5000

# Dump the camera's GenApi XML
viva-camctl xml --ip 192.168.0.10 --out camera.xml
```

## When something does not work

```bash
# Collect everything a bug report needs, in one file
viva-camctl report --ip 192.168.0.10 --out viva-report.txt
```

The report lists the network interfaces the library can see, the camera's
reply to discovery, its bootstrap registers, its GenApi XML, and any feature
the camera has that this library could not build. Neither `report` nor `xml`
needs the camera to open successfully — that is what they are for — so both
still produce output when nothing else does. Attach the file to an
[issue](https://github.com/VitalyVorobyev/viva-genicam/issues/new/choose).

## Streaming

```bash
# Receive a GVSP stream and save 2 frames to the current directory.
# By default the camera's GevSCPSPacketSize is kept; --auto sizes packets from
# the NIC MTU, --packet-size N sets an explicit ceiling.
viva-camctl stream --ip 192.168.0.10 --iface 192.168.0.5 --auto --save 2
```

See [Streaming → Packet size and MTU](tutorials/streaming.md#41-packet-size-and-mtu).

## Windows specifics

The firewall prompt, NIC power saving and jumbo frames are the usual obstacles;
see [Networking → Windows](networking.md#22-windows).

## Next steps

* Read the [Primer](primer.md) for the concepts behind discovery, control, and streaming.
* Jump to the [Discovery tutorial](tutorials/discovery.md) for a step‑by‑step walkthrough with troubleshooting tips.
* No camera yet? [Testing without hardware](tutorials/fake-camera.md) runs everything against an in-process fake.

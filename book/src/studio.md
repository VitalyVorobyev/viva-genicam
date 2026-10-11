# Viva Studio

Viva Studio is the desktop application that comes with viva-genicam: find a
camera, browse and change its features, watch the live image, and save the
diagnostic report we ask for when something goes wrong. It runs the library
itself, so it is also the quickest way to try viva-genicam on your camera.

> **Experimental.** Studio discovers cameras, reads and writes features and
> streams live images, but it is the least-exercised part of the project: its
> hardware reports come from a few users and camera models
> ([Status](welcome.md#status)). If what Studio shows disagrees with your
> camera, please [report it](reporting.md).

## Install

Each release from 0.8.0 attaches Viva Studio to its
[GitHub release](https://github.com/VitalyVorobyev/viva-genicam/releases),
under the same version as the library, as `viva-studio-<tag>-<target>.<ext>`:

| OS | File | Install |
|---|---|---|
| macOS, Apple Silicon | `.dmg` | Open it and drag Viva Studio to Applications |
| Linux x86_64 | `.deb` or `.AppImage` | `sudo apt install ./viva-studio-….deb`, or `chmod +x` the AppImage and run it |
| Windows x86_64 | `.msi` | Run the installer |

The Linux builds need glibc 2.39 (Ubuntu 24.04) or newer and WebKitGTK 4.1,
which the `.deb` pulls in. Running the AppImage needs FUSE 2 (`libfuse2t64` on
Ubuntu 24.04). `SHA256SUMS.txt` on the release lists every file's checksum.
There is no Intel Mac build.

### First launch

The bundles are **not signed** — signing needs paid certificates from Apple and
Microsoft — so the first launch asks you to confirm:

- **macOS** refuses to open the app the first time. Open System Settings →
  Privacy & Security and choose *Open Anyway* for Viva Studio, or clear the
  download's quarantine flag:

  ```bash
  xattr -dr com.apple.quarantine "/Applications/Viva Studio.app"
  ```

- **Windows** SmartScreen shows "Windows protected your PC": choose *More info*,
  then *Run anyway*. If Windows Firewall then asks whether Viva Studio may use
  the network, allow the network your camera is on: GigE Vision discovery and
  streaming arrive as inbound UDP.

To build Studio from source instead, see
[`studio/README.md`](https://github.com/VitalyVorobyev/viva-genicam/blob/main/studio/README.md).

## Connect to a camera

Studio lists the cameras it finds in the device menu at the top right. Pick one
to connect.

- **GigE Vision** cameras are found by broadcast on every network interface.
  If yours does not appear, it is almost always the network:
  [Networking](networking.md) covers addressing, firewalls and jumbo frames.
- A GigE camera on the wrong subnet can be given a temporary address from the
  device menu with **Set IP…** (FORCEIP), and a permanent one from the
  **Camera** panel of the Diagnostics tab.
- **USB3 Vision** cameras are found by USB enumeration. On Linux your user
  needs permission to open the device (a udev rule).

Studio talks to the camera directly. It can also run against cameras that
`viva-service` exposes over the network ([remote mode](#remote-mode)).

## Features

The **Features** tab shows the camera's feature tree as its GenApi XML
describes it: categories on the left, features on the right, with search and
an editor for each feature type. A feature the camera currently refuses — read
only, locked by another feature, or unavailable — is shown as such and cannot
be edited. **Load XML…** opens a GenApi XML file with no camera attached, for
browsing a camera's description offline.

The sidebar groups the features most sessions need — acquisition control,
exposure and gain, image format, trigger, user sets, transport layer, auto
functions and colour processing — showing only those your camera has, and the
packet size of a GigE stream.

## Images

The **Image** tab shows the live stream once you press **Start Acq**. The
toolbar zooms to fit or to exactly one screen pixel per camera pixel, and
opens a histogram, a line profile and an ROI tool. **Save snapshot** saves the
image as shown, as an 8-bit PNG. The header shows the frame rate and, once any
are lost, how many frames were dropped — the first sign of a network that
cannot keep up.

## Diagnostics and reports

The **Diagnostics** tab shows the camera's identity, its IP configuration, the
declarations in its XML that the library could not turn into features, and a
log of what Studio did.

**Generate report** writes the same report as
`viva-camctl report` ([Reporting a camera we can't open](reporting.md)):
your system, what discovery heard, the camera's identity and, by default, its
XML. Save it or copy it, and attach it to an issue. It is the most useful thing
you can send us.

## Remote mode

Instead of opening cameras itself, Studio can show cameras that `viva-service`
(GigE Vision) or `viva-service-u3v` (USB3 Vision) serve over
[Zenoh](https://zenoh.io): for a camera on another machine, or several viewers
of one camera. `viva-service` is in the `viva-genicam-<version>-<platform>`
archive on each [GitHub release](https://github.com/VitalyVorobyev/viva-genicam/releases);
`viva-service-u3v` runs from a clone of the repository.

Both sides need a Zenoh configuration, and neither loads one by itself. The
repository's
[`studio/config/`](https://github.com/VitalyVorobyev/viva-genicam/tree/main/studio/config)
holds a pair for one machine: the service listens on `tcp/127.0.0.1:7447`
(`zenoh-local.json5`) and Studio connects there (`zenoh-studio.json5`). For a
service on another machine, put that machine's address in both.

1. Start the service with its configuration, naming the network interface the
   cameras are on:

   ```bash
   viva-service --iface eth0 --zenoh-config /path/to/zenoh-local.json5
   ```

2. Start Studio with the `ZENOH_CONFIG` environment variable set to the
   **absolute** path of its configuration:

   ```bash
   # macOS
   open -a "Viva Studio" --env ZENOH_CONFIG=/path/to/zenoh-studio.json5
   # Linux, the AppImage
   ZENOH_CONFIG=/path/to/zenoh-studio.json5 ./viva-studio-….AppImage
   ```

   On Windows, set `ZENOH_CONFIG` as a user environment variable, then start
   Studio; remove the variable to go back to direct mode.

The chip in Studio's header reads **Remote** or **Embedded** (direct mode).
Without `ZENOH_CONFIG` Studio runs in direct mode; a `ZENOH_CONFIG` it cannot
load is reported as an error, not ignored.

### Trying it without a camera

From a clone of the repository, a fake GigE Vision camera on loopback stands in
for a real one ([Testing without hardware](tutorials/fake-camera.md)):

```bash
# Terminal 1: the fake camera
cargo run -p viva-fake-gige

# Terminal 2: the service, on the loopback interface (lo0 on macOS, lo on Linux)
cargo run -p viva-service -- --iface lo0 --zenoh-config studio/config/zenoh-local.json5

# Terminal 3: Studio in remote mode, as in step 2, with an absolute path
open -a "Viva Studio" --env ZENOH_CONFIG=$(pwd)/studio/config/zenoh-studio.json5
```

Studio lists the fake camera, shows its features, and streams a gradient image.
For USB3 Vision the service hosts a fake of its own, which replaces the first
two terminals:

```bash
cargo run -p viva-service-u3v -- --fake --zenoh-config studio/config/zenoh-local.json5
```

A fake camera appears only in remote mode: direct mode does not look for
cameras on loopback, so without `ZENOH_CONFIG` the device list stays empty.

The services and their wire protocol are described in the repository's
[`docs/studio/zenoh-api.md`](https://github.com/VitalyVorobyev/viva-genicam/blob/main/docs/studio/zenoh-api.md).

## Not there yet

- Signed and notarised bundles, and automatic updates.
- Recording and playback.
- More than one camera at a time.
- The camera's own 10-, 12- and 16-bit pixel values: the inspector, histogram
  and snapshots work on the 8-bit image shown.

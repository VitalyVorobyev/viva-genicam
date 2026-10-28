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
to connect with **Control** access (the default). The separate **Monitor** action
opens an embedded GigE Vision camera read-only, including while another
application controls it. Direct monitor access is unavailable for USB cameras
and in remote mode, where the service owns camera access.

- **GigE Vision** cameras are found by broadcast on every network interface.
  If yours does not appear, it is almost always the network:
  [Networking](networking.md) covers addressing, firewalls and jumbo frames.
- A GigE camera on the wrong subnet can be given a temporary address from the
  device menu with **Set IP…** (FORCEIP), and a permanent one from the
  **Camera** panel of the Diagnostics tab. Discovery requests broadcast
  acknowledgements, so devices on another subnet can still appear with a
  **Different subnet** label. The label does not prohibit a connection through
  a configured route.
- **USB3 Vision** cameras are found by USB enumeration. On Linux your user
  needs permission to open the device (a udev rule).

Studio talks to the camera directly. It can also run against cameras that
`viva-service` exposes over the network ([remote mode](#remote-mode)).

### Connection loss and recovery

In direct mode, Studio watches the GigE control connection even while no image
is being acquired. If the camera disappears, the viewer closes its old stream
and Studio looks for the same camera by MAC address for up to 30 seconds. The
camera may return with another IP address. Its XML and feature controls are
loaded again; configuration values are not automatically written back.

**Acquisition remains stopped after recovery.** Check the current settings and
press **Start Acq** when ready. **Cancel reconnect** abandons the search, and
connecting another camera also cancels it. If recovery expires, reconnect
from the device menu.

When the transport reports lost control while the camera is still reachable,
Studio keeps the session read-only and displays **Camera control lost**. It
does not repeatedly try to reclaim control. Use **Reconnect with control**
when the other controller has finished. On a camera that does not identify its
controller, a takeover may become visible only after a refused write. A disconnected USB3 Vision camera
requires manual reconnection from the device menu.

### Watching another application's stream

In a direct GigE **Monitor** session, feature controls, commands and persistent
IP changes are read-only. **Watch stream** joins the multicast destination
already configured by the controller; it does not start acquisition or change
packet size, timing, destination or trigger settings. A stream sent by unicast
to another host cannot be watched this way. Configure multicast and start
acquisition in the controlling application first.

**Stop watching**, Disconnect and closing Studio only end this viewer's
reception. The controller and camera keep acquiring. After a monitor connection
is recovered, it remains a monitor and its viewer stays stopped until you
choose **Watch stream** again. Packet-size controls are disabled because the
controller owns those settings. A lost-control banner also offers **Open as
monitor** for a direct GigE camera; this is always an explicit choice.

### macOS local-network access

On macOS 15 or later, allow Viva Studio access to the local network when macOS
asks. This permission is separate from opening an unsigned download. To check
it later, open System Settings → Privacy & Security → Local Network and find
Viva Studio. Close and reopen Studio after changing the permission.

Check this setting when Studio cannot contact a GigE camera or a remote service,
even if `viva-camctl` works from Terminal: macOS treats command-line tools and
apps differently. A network error such as `Broken pipe` alone does not prove
that permission is the cause; [report it](reporting.md) if access is allowed and
the connection still fails. Apple's [local-network privacy guide](https://developer.apple.com/documentation/technotes/tn3179-understanding-local-network-privacy)
describes the distinction.

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

Expand **Image preview** in the Features tab for a compact, fit-to-view canvas
while changing settings. It starts collapsed and remembers your preference.
Acquisition still starts from **Start Acq**; with the preview expanded, Start
keeps the Features tab visible. The canvas shares the Image tab's preview
connection, and only the visible canvas decodes frames.

## Images

The Image sidebar puts acquisition rate, exposure and gain above the statistics
for quick adjustment. Controls use the camera's declared limits, units and
access restrictions, including older names such as `ExposureTimeAbs` and
`AcquisitionFrameRateAbs`. A raw gain or exposure value keeps its camera units;
it is not converted to dB or microseconds. After automatic remote reconnection,
Studio refreshes the feature graph and controls from the replacement session.

The **Image** tab shows the live stream once you press **Start Acq**. The
toolbar zooms to fit or to exactly one screen pixel per camera pixel, and
opens a histogram, a line profile and an ROI tool. **Freeze for native pixel
analysis** holds one frame for the pixel inspector, histogram and line profile.
They use the camera's original sample values, including 10-, 12- and 16-bit
formats. **Resume live preview** returns to the stream; freezing the viewer
does not stop acquisition. The header shows the frame rate and, once any
are lost, how many frames were lost — the first sign of a network that
cannot keep up.

**Save snapshot** saves a PNG at the source bit depth. While frozen, it saves
the frame being inspected; while live, it requests one recent frame. Bayer
inspection measures the sensor mosaic, while the PNG contains demosaiced
colour. Formats without a supported native conversion report an error and
leave the live preview available.

Native capture supports images up to 16 Mi pixels. Remote mode carries each
frame's row stride, so padded rows and supported odd-width packed images use
the same conversion as embedded mode. Update the service and Studio together:
this requires service API version 7 and frame-header version 2; older frame
headers are rejected.

The **Statistics** section of the Image tab counts every frame the camera
sent: received complete, incomplete, missed, dropped by the host, and for GigE
Vision the packets and resends behind them, or for USB3 Vision the transfers.
These are frames *received*. Live preview targets 30 frames a second (15 in
remote mode), decoding only in the visible Image tab or expanded Features
preview, so the Image status bar's
"display" rate and the **Displayed** count stay low at a high camera rate; that
is the preview, not a loss. The counters keep their last values after **Stop**.

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

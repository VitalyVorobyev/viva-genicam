# Viva Studio

Viva Studio is the desktop application that comes with viva-genicam: find a
camera, browse and change its features, watch the live image, and save the
diagnostic report we ask for when something goes wrong. It runs the library
itself, so it is also the quickest way to try viva-genicam on your camera.

> **Experimental.** Studio discovers cameras, reads and writes features and
> streams live images, but it is the least-exercised part of the project: it
> has no camera in CI, and its hardware reports come from a few users and
> camera models. USB3 Vision has not been tried on a real camera at all. If
> what Studio shows disagrees with your camera, please
> [report it](reporting.md).

## Install

Each release from 0.8.0 attaches Viva Studio to its
[GitHub release](https://github.com/VitalyVorobyev/viva-genicam/releases),
under the same version as the library, as `viva-studio-<tag>-<target>.<ext>`:

| OS | File | Install |
|---|---|---|
| macOS, Apple Silicon | `.dmg` | Open it and drag Viva Studio to Applications |
| Linux x86_64 | `.deb` or `.AppImage` | `sudo apt install ./viva-studio-….deb`, or `chmod +x` the AppImage and run it |
| Windows x86_64 | `.msi` | Run the installer |

The Linux builds need glibc 2.35 (Ubuntu 22.04) or newer and WebKitGTK 4.1,
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
of one camera. Start Studio with the `ZENOH_CONFIG` environment variable set to
an absolute path to a Zenoh configuration file; without it Studio runs in its
default, direct mode. The services and their wire protocol are described in
the repository's
[`docs/studio/zenoh-api.md`](https://github.com/VitalyVorobyev/viva-genicam/blob/main/docs/studio/zenoh-api.md).

## Not there yet

- Signed and notarised bundles, and automatic updates.
- Recording and playback.
- More than one camera at a time.
- The camera's own 10-, 12- and 16-bit pixel values: the inspector, histogram
  and snapshots work on the 8-bit image shown.

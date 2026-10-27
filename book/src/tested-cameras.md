# Cameras that have run viva-genicam

Each camera below has run the library, as an issue or pull request shows; the
link is the evidence, and nothing here goes beyond it. *Reported* is what the
person with the camera said they saw. *Inferred* is what we concluded from
what they sent us: an XML file, a log, a capture. A fix that was never run
again on the camera says so.

What does not work yet, on any camera, is
[issue #191](https://github.com/VitalyVorobyev/viva-genicam/issues/191). If
your camera is missing here, or does something this page does not say,
[tell us](reporting.md): a report from your camera is how this list grows.

## GigE Vision

### Anonymous six-sensor GigE device

GigE Vision 1.2; the manufacturer, model and unique identifiers are private.
Maintainer test on macOS/Apple silicon,
1 Gb/s Ethernet, MTU 1500. One stream contains six side-by-side Mono8 images.
The initial run exposed reversed controller IP/port register contents;
the transport accepts that layout only when both values match its socket.
Short runs delivered six full 720 × 540 images at 30 FPS without reported loss.
Each sensor's test pattern was checked in its own image tile. C GenTL and
Harvesters each received 1000 complete frames. Faster minimum-ROI runs and
Python delivery showed host backpressure drops; this is not a loss-free
maximum-FPS or endurance claim. Events, chunks and metric 3D are not
advertised by this interface. The fixes and tests are recorded in [#224](https://github.com/VitalyVorobyev/viva-genicam/pull/224).
The 2026-10-27 follow-up also verified Studio quick controls, compact preview,
near-cap display pacing and remote controls after a software reset. Preloading
NumPy avoided the measured Python startup drops. A larger delivery queue then
passed short production CLI checks at the minimum ROI and full six-sensor ROI;
this is not endurance qualification.
Detailed scope and restoration are in the
[bench record](https://github.com/VitalyVorobyev/viva-genicam/blob/main/docs/hardware-six-sensor-2026-10-26.md).

### Allied Vision Manta G-125B

Firmware 00.01.44.28514, GigE Vision 1.2; macOS on Apple silicon, through a
USB Ethernet adapter at MTU 1500. Run by the maintainer, so everything here is
what was seen on the camera, with verbose logs. The 2026-10-25 run could not capture packets because access to the host’s BPF devices was denied.

- Discovery, including of the camera configured for another subnet, FORCEIP
  and persistent IP configuration
  ([#215](https://github.com/VitalyVorobyev/viva-genicam/pull/215)).
- Control and exclusive access, a second application refused while one holds
  control, a read-only monitor receiving another application's multicast
  stream, commands, a saved configuration loaded back to the same 98 values,
  chunks in its GigE Vision 1.x layout, and events
  ([#216](https://github.com/VitalyVorobyev/viva-genicam/pull/216)).
- Streaming in every pixel format it offers, a 300 s stream at 30.9 fps with
  no frame lost, packet resend, packet pacing, multicast with a monitor,
  acquisition modes and software triggers, event timing, action commands
  (which it does not implement) and its clock
  ([#217](https://github.com/VitalyVorobyev/viva-genicam/pull/217)).
- A two-hour endurance run: 222,375 complete frames, one incomplete frame, no missed frames or delivery backpressure; resident memory stayed bounded and 15 files remained open. A separate ten-minute run delivered 18,531 complete frames without loss. These are measurements, not a loss-free eight-hour qualification.
- On 2026-10-25, all 24 hardware tests passed again. The C GenTL consumer and Harvesters each acquired ten complete 1292 × 964 Mono8 frames; Python discovery, feature reads, command execution and three NumPy-backed image captures passed.
- The service published 92 image messages in three seconds and resumed acquisition after an expired heartbeat and temporary IP change. A native Studio debug bundle discovered this camera but failed to claim control with `Broken pipe`; its cause remains unconfirmed. The eight-hour follow-up started on 2026-10-25 and is still pending.
- The recovery and surface checks above are recorded in [#218](https://github.com/VitalyVorobyev/viva-genicam/pull/218); its eight-hour endurance result is pending.
- The hardware smoke suite, `crates/viva-genicam/tests/gige_hardware.rs`,
  passes on it (#216, #217).

### JAI FS-3200T-10GE-NNC

Windows 11 and Linux, link-local (APIPA) addressing; run by a user. Some
threads call it a Vieworks FS3200T: the reporter wrote "JAI / Vieworks
FS-3200T-10GE", and discovery reports JAI Corporation.

- Reported: discovery on Windows after the fix in
  [#64](https://github.com/VitalyVorobyev/viva-genicam/pull/64), and FORCEIP,
  with a capture we parsed and found to match
  ([#57](https://github.com/VitalyVorobyev/viva-genicam/issues/57)).
- Reported: on Linux, discovery, the XML download, 1 065 features loaded and
  streaming once the firewall let GVSP in
  ([#57](https://github.com/VitalyVorobyev/viva-genicam/issues/57#issuecomment-5127958912)).
- Reported: 15 s of streaming on Windows through `viva-camctl`, 46 frames and
  none dropped ([#72](https://github.com/VitalyVorobyev/viva-genicam/pull/72));
  a later run on `main`, 31 frames and none dropped, which we identified as
  this camera from its MAC address
  ([#70](https://github.com/VitalyVorobyev/viva-genicam/issues/70#issuecomment-5137443985)).
- Reported: live frames in Viva Studio's embedded mode
  ([#87](https://github.com/VitalyVorobyev/viva-genicam/pull/87)), and
  `viva-service` discovering and connecting on Windows by address and by
  adapter GUID
  ([#109](https://github.com/VitalyVorobyev/viva-genicam/issues/109#issuecomment-5172977468)).
- Reported: the automatic packet size gave no frames in Studio, and the cause
  was a switch on the path
  ([#112](https://github.com/VitalyVorobyev/viva-genicam/issues/112)). The
  fixes that followed have not been run on the camera; reads of its event
  registers refused with `BAD_ALIGNMENT` are still open there.

### JAI GOX-2450SC-5GE

Windows 11, Viva Studio; run by a user. Reported: discovery, connection and
its features in Studio. Acquisition failed on a host-side defect, since
fixed, and has not been reported on this camera since
([#70](https://github.com/VitalyVorobyev/viva-genicam/issues/70)).

### Micro-Epsilon scanCONTROL 8500-50

A GigE Vision laser profile scanner, firmware 3.0.0; run by a contributor.

- Reported: discovery with `viva-camctl`
  ([#95](https://github.com/VitalyVorobyev/viva-genicam/pull/95)).
- Reported: streaming its `Coord3D_ABC32f` profiles through the library,
  755 568 frames with none dropped
  ([#93](https://github.com/VitalyVorobyev/viva-genicam/pull/93#issuecomment-5162882332)).
- Reported: how it answers control-channel reads while a command is pending
  (`PENDING_ACK`) and when its 5 s heartbeat expires, and, on the published
  0.8.0 crates, a 5.1 MB file download
  ([#163](https://github.com/VitalyVorobyev/viva-genicam/issues/163#issuecomment-6118461248)).

### LUCID Triton2 EVS TRT009S-E

An event-vision camera (Sony IMX636); run by a contributor, on Linux as we
read from its interface name.

- Reported: control through the library
  ([#138](https://github.com/VitalyVorobyev/viva-genicam/issues/138)), and
  streaming its EVT event data with `viva-camctl stream --raw-out`
  ([#154](https://github.com/VitalyVorobyev/viva-genicam/pull/154)).
- The EVT decoder's tests use bytes captured from this camera
  ([#139](https://github.com/VitalyVorobyev/viva-genicam/pull/139)).

### Basler a2A4096-44g5m

NixOS (Linux), the 0.9.0 crates; run by a user
([#210](https://github.com/VitalyVorobyev/viva-genicam/issues/210)).

- Reported: discovery, connection, writing `PixelFormat` and
  `AcquisitionMode`, and one 4096 × 3000 `Mono8` image, saved through the
  reporter's own workaround for the stream port.
- Reported: `TriggerSelector` failed to resolve on 0.9.0 and worked on `main`;
  `TriggerMode` still could not be read. The fix in 0.10.0 was checked against
  the camera's XML only.

### Hikrobot MV-CS050-10GC

Arch Linux; run by a user. Reported: discovery, and a connection — XML fetched
and its node map built — after three fixes to how its XML is read
([#35](https://github.com/VitalyVorobyev/viva-genicam/issues/35#issuecomment-5120594600)).
Streaming was not reported.

### FLIR Blackfly S BFS-PGE-31S4C-C

macOS, link-local addressing, the Python package; run by a user.

- Reported: the camera connects on 0.3.0 after it had failed to open
  ([#45](https://github.com/VitalyVorobyev/viva-genicam/issues/45#issuecomment-5129635053)).
- Reported: discovery and opening with `viva-camctl` on 0.4.1
  ([#120](https://github.com/VitalyVorobyev/viva-genicam/issues/120)). Writing
  `ExposureTime` was refused as unavailable; the fix in 0.5.0 rests on its XML
  and has not been run on the camera.

### FLIR Blackfly (model not stated)

Run by a user. Reported: one of its PTP registers sometimes failed to read on
0.5.0, and the fix worked on the camera
([#140](https://github.com/VitalyVorobyev/viva-genicam/issues/140#issuecomment-5838690394)).

### The Imaging Source DMK 33GP2000e

Ubuntu 24.04; run by a user. Reported: discovery and the XML download on
0.4.1, then a failure to parse the XML
([#122](https://github.com/VitalyVorobyev/viva-genicam/issues/122)); the fix in
0.5.0 has not been reported run on the camera. On 0.6.0, a capture of a "dmk
camera" showed every packet-size write sent as `WRITEREG`
([#136](https://github.com/VitalyVorobyev/viva-genicam/issues/136#issuecomment-5845319837)).

### Cameras whose model was not given

- Viva Studio's embedded mode on Ubuntu, against an unnamed camera: no more
  refused reads on 0.6.0
  ([#135](https://github.com/VitalyVorobyev/viva-genicam/issues/135#issuecomment-5844760153)).
- Several identical cameras told apart by serial on 0.6.0
  ([#137](https://github.com/VitalyVorobyev/viva-genicam/issues/137#issuecomment-5844744873)).

## USB3 Vision

### MATRIX VISION mvBlueFOX3-2004G

macOS on Apple silicon over a USB 2 (High Speed) link behind a hub, and
Windows; run by the maintainer.

- Bootstrap registers, streaming, chunks, events, the host clock, the cable
  pulled mid-stream, `viva-service-u3v` rescanning while it holds the camera,
  and the Python package
  ([#205](https://github.com/VitalyVorobyev/viva-genicam/pull/205)).
- String and `<pLength>` features, enumeration entries it does not offer, a
  saved configuration loaded back, and the USB3 Vision hardware suite
  ([#206](https://github.com/VitalyVorobyev/viva-genicam/pull/206)).
- On Windows, after WinUSB was installed on the camera's first interface,
  acquisition and Viva Studio
  ([#208](https://github.com/VitalyVorobyev/viva-genicam/pull/208)).

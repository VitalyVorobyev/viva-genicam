# QUISS RTVision six-sensor bench, 2026-10-26

Maintainer-operated hardware on macOS/Apple silicon, through `en10` at
1000baseT full duplex, MTU 1500. The user confirmed an isolated bench,
secured laser paths and disabled robot/motion before acquisition. These are
short functional measurements, not certification or an endurance run.

## Device and preservation

Discovery identified QUISS `RTVision_6L_DCAM_v2`, firmware `2.0.0.0`,
serial `SN64501926`, MAC `8C:1F:64:50:19:26`, GigE Vision 1.2.
The device exposes one stream channel containing six side-by-side Mono8
images. At its original settings each sensor contributes 324 × 56 pixels,
producing a 1944 × 56 frame and 108,864-byte payload. Full sensor geometry
is 720 × 540 each, producing a 4320 × 540 frame. Only Mono8 is available;
the other names in its PixelFormat enumeration are unavailable. No
`Coord3D`, `Scan3d`, chunk controls or event message channel are advertised.
This device does not establish the pending metric 3D wire-format cases.

Original volatile and persistent address: `192.168.0.25/24`, gateway zero.
The host remained `192.168.2.10/24`; normal discovery missed the device, but
`list --allow-broadcast-ack` found it. Testing used volatile FORCEIP to
`192.168.2.27/24`, with the same gateway. No persistent network settings,
user sets, firmware or host NIC settings were written. The timestamp-reset
case intentionally resets the free-running device clock; configuration equality
does not imply restoration of that clock or other counters.

The XML declares no streamable features: `save` succeeds with **zero values**.
That file cannot restore this device's settings. Instead the bench first saved an
explicit 34-feature inventory, extended it to 50 before broader testing,
restored every recorded entry after each batch and read each back for exact
textual equality. Post-reboot restoration uses the saved pre-reboot inventory,
not the device's startup defaults. The inventory includes sensor
geometry/offsets, test patterns, exposure/gain, acquisition/trigger selection,
sequential mode and stream destination/packet settings; the extended inventory
also covers LUT/test-pattern selection, trigger delay, laser timing, file-operation
metadata and interface/channel selectors. The Python trigger
case additionally saved/restored the FrameStart trigger bank before returning
to the original selector. AcquisitionStop was issued explicitly after the
initial failure. Persistent address registers and packet flags were checked
again in the post-run report.

Returning to the original subnet repeatedly timed out waiting for FORCEIP's
ACK, although subsequent broadcast discovery found the **same MAC at the
original IP, mask and gateway**. Record the command failure and the successful
state verification separately; do not call the ACK successful.

Raw evidence, vendor XML and images are ignored local files under
`.local-evidence/rtvision-2026-10-26/` (initial failures and protocol probe)
and `.local-evidence/rtvision-fixed-2026-10-26/` (patched binaries and reruns).
No vendor XML or private images are published with this record.

## Control ownership correction

Initial streaming delivered complete frames but the library declared control
lost and could not send AcquisitionStop. A raw GVCP probe then claimed CCP
and read its value plus both primary-application registers every 450 ms for
12 samples. With local port 51383, CCP stayed 2 while `0x0A04` contained
`0xC0A8020A` (our IP) and `0x0A14` contained 51383. A second session used
port 63644 and read the corresponding new value. Reads kept control alive;
this is not evidence that a write heartbeat is required.

The device reverses the primary-application IP/port contents relative to the
standard layout used by this library and corroborated by
[Wireshark's register definitions](https://github.com/wireshark/wireshark/blob/master/epan/dissectors/packet-gvcp.c).
The transport now also accepts the reversed layout **only when both full
values match this socket's IP and port**. Standard constants remain unchanged;
no automatic reclaim or ownership write is added. Scripted UDP regressions
cover both layouts, other IPs/ports, malformed swapped ports and devices
without this capability.

## Short acquisition and delivery measurements

Each row is one short run. FPS below is complete frames divided by the
requested duration, not the report's moving-average FPS. It is not a
statistically established sustainable ceiling. All rows reported zero
incomplete/missed frames, missing packets and requested resends.

| Configuration | Seconds | Complete | Backpressure drops | Result |
|---|---:|---:|---:|---|
| Original 1944 × 56, sequential, 100 µs exposure, requested 1000 FPS | 4 | 3049 | 0 | Passed; 762.25 complete FPS |
| Minimum 24 × 16 aggregate, sequential, requested 100 FPS | 3 | 301 | 0 | Passed |
| Minimum ROI, sequential, requested 5000 FPS | 5 | 4550 | 0 | Passed short acquisition; 910 complete FPS |
| Six full 720 × 540 sensors, requested 30 FPS | 5 | 150 | 0 | Passed; 30 complete FPS, 559.872 Mb/s image payload |
| Six full 720 × 540 sensors, requested 5000 FPS | 5 | 252 | 0 | Passed short acquisition; 50.4 complete FPS, 940.58496 Mb/s image payload |
| Minimum ROI, simultaneous, 5 µs exposure, requested 5000 FPS | 5 | 13370 | 31 | Failed loss-free delivery; 2674 complete FPS |
| Same, requested 2000 FPS | 5 | 10002 | 6 | Failed loss-free delivery |
| Same, requested 2500 FPS | 5 | 12497 | 13 | Failed loss-free delivery |

The faster cases expose complete-frame eviction in the bounded four-frame
handoff, rather than packet loss. They do not prove a new allocation
bottleneck. Follow-up is SR-28; the allocation strategy remains unchanged.
`MaxAcquisitionFrameRate` read 0.07 at the original settings, inconsistent with
observed acquisition. It was not used as a physical frame-rate limit.

## Feature and consumer checks

| Case | Result and scope |
|---|---|
| Discovery and XML | Passed: direct/broadcast discovery on the temporary subnet; 334 nodes built, none dropped; 305 readable values read without feature failures. The extracted vendor XML validates against GenApi 1.1. |
| Control and monitor | Passed: control held/released on drop; monitor alongside controller; exclusive CCP = 1, with another application's read refused. |
| Competing controller | The initial harness assumed writable Width, but Width is RO here. The corrected check writes the original heartbeat timeout through each application's own socket. Passed: a second claim and raw write were refused; the holder still wrote; a write while nobody held control was also refused. |
| Six sensor identities in the image | Passed: each sensor's test pattern enabled separately; exactly its corresponding 324-pixel tile equals the expected horizontal ramp. All six patterns together also produced six matching tiles. This is not optical calibration validation. |
| Writable features and enumeration entries | Passed after repairing the harness's text-only assumption for byte registers: 51 features accepted their original values; 55 offered enum entries accepted, one unavailable entry refused. This covers the current selector/state combinations, not all possible combinations. Non-volatile names were excluded. |
| Standard helpers | Passed: ExposureTime 100 → 300 → 1234 (requested 1234.5) → 100 µs; gain 5 → 6 → 5 dB. No binning feature advertised. Generic Width is read-only; sensor-specific ROI features were tested explicitly. |
| Software trigger through Python | Passed: 10 commands produced 10 consecutive complete frames with no reported loss; FrameStart selector bank restored. |
| Python and NumPy | Feature access and idle ownership beyond the 3 s timeout passed. 3020 frames converted to uint8 arrays of shape (56, 1944) in 4 s, but 30 backpressure drops occurred: functional conversion passed, loss-free delivery failed. |
| C GenTL consumer | Passed 1000 complete 1944 × 56 Mono8 frames of 108864 bytes; feature/port access exercised. Initial harness rejected the camera's compressed XML; fixed harness preserves binary ZIP bytes instead of treating them as text. |
| Harvesters | Passed 1000 complete frames, geometry/format/payload checked against the camera's remote nodemap. |
| GenTL remote XML container | Passed: saved ZIP CRC/member validation, decompressed XML byte-identical to the direct CLI XML; GenApi 1.1 schema validation passed. ZIP CRC/schema validation is external to the C harness. |
| Acquisition restarts | Initial 100-cycle high-rate run failed the contiguous delivered-ID assertion despite zero incomplete/missed counters; the delivery-backpressure counter was absent from its printout. The test now prints it. A separate 100-cycle rerun at 30 FPS passed with exactly 200 complete delivered frames and zero incomplete/missed/backpressure frames; block IDs continued from 1 through 200. Do not reinterpret the initial run as loss-free. |
| Explicit packet resend | Passed: packets 2–3 from a recent block returned status 0; old block 1 returned header-only `0x800C` for both packets. One stream channel only; SR-25's nonzero-channel question remains open. |
| SingleFrame | Passed: exactly one complete frame in the bounded one-second CLI run. Continuous was restored afterward. MultiFrame is not offered. |
| Packet sizes | Passed: preserve/current and automatic mode; requested 1000 rounded by the device to 1004; an oversized 9000 request probed down to 1500 on this link. No reported loss in the three-second explicit-size runs. |
| Packet delay | At requested 1000 FPS, three-second runs with 0/10/50 µs packet delay delivered 2288/1974/754 complete frames. The 10 µs case also dropped three complete frames to backpressure; the others reported zero loss. This shows throughput reduction, not a wire-level spacing measurement. |
| Multicast | Passed: controller 601 complete frames in six seconds; monitor 299 during its three-second joining window, with zero reported loss counters. No claim of comparing identical start/end block ranges. |
| Clock helpers | Passed short calibration/mapping (three frames mapped within 39 ms of the host check); reset returned about 0.769 ms after a pre-reset clock of 2098.203 s. Hour-scale drift remains untested. |
| Fresh-session reopen | Passed: MAC/XML matched, a new local endpoint was opened and a complete 1944 × 56 frame arrived. Zero streamable values were restored; this is not proof of effective configuration-file restoration on this XML. |
| Events, chunks, other pixel formats, metric 3D | Unsupported by the advertised interface on this unit. Unicast immediate and scheduled action commands both returned NOT_IMPLEMENTED. Broadcast actions were not sent onto the host's other network. |
| Studio embedded | Passed short acquisition: 59,695 complete frames in UI elapsed 1:18; zero incomplete/missed/backpressure/missing/resend counters. Freeze/histogram and PNG export worked; PNG verified 1944 × 56, 8-bit grayscale. Histogram range 0–2 describes a nearly black scene, not optical calibration. |
| Service + Studio remote | Passed short acquisition: 12,957 complete frames in UI elapsed 0:17, zero reported loss counters; remote native freeze/histogram worked. Duplicate bulk-state replies/warnings exposed overlapping queryables; exact endpoint ownership is corrected and regression-tested. |
| Software reboot and service replacement | See recovery details below. This is distinct from physical power/cable tests. |
| Induced-loss stress, hour-scale drift, physical disconnect/power-cycle, hardware trigger wiring, vendor configuration import, persistent user-set/file operations | Untested on RTVision in this batch. The Manta's results do not substitute for these device-specific cases. |
| Endurance | Untested; no long test launched. |

The generic hardware suite was run selectively because its streamable-feature
backup is empty on this device. Enumeration round trips and original-value writes ran with the explicit inventory.
UserSetLoad/Save and FileOperationExecute were not executed: this batch does not
back up every stored user-set or file.

## Software reboot and remote recovery

Two DeviceReset commands were executed through Studio's feature browser while
service acquisition was active. The device returned at its persistent address;
MAC-based broadcast discovery then found it and volatile FORCEIP returned it
to the temporary test subnet. An initial assessment of a service stall was
**withdrawn**: that run logged only warnings, and a stack sample showed active
discovery, so it could not establish that reconnection failed.

The repeat used explicit debug logging. The log records frame-loop exit,
retirement of the old connection, disconnected status at 08:07:32 UTC and a
fresh connection/connected status at 08:07:35 UTC. Studio then started acquisition
through that replacement session and received **52,962 complete frames** in UI
elapsed 1:09, with zero incomplete/missed/backpressure/missing/resend counters.
The old session's unsuccessful stop/control probes during reboot are retained
as expected failure-path evidence; they are not clean-stop successes. The
service subsequently shut down with exit 0.

**ST-61 remains open:** after automatic replacement, the remote image controls
for acquisition mode/rate, exposure and gain disappeared, although streaming
and native freeze continued to work. Disconnecting and reconnecting the viewer
restored all those controls. Do not call this full Studio recovery parity.
The service regression now also reboots the fake during active acquisition,
requires replacement XML and a new-sized frame, and checks shutdown.

Both GUI variants were current-code Tauri debug bundles with the local-network
usage description, not release/notarization tests. Preparation manifests record
source patches and binary/signature hashes; no clean-commit build is asserted.
All viewers and service processes were stopped before final restoration.

## Code validation

Final root formatting, clippy (all targets, warnings denied), all-feature
Rustdoc with warnings denied, both wasm checks and **1,070 workspace tests**
passed. The three scripted controller-layout tests and both service regressions
passed; hardware-only tests were run separately and selectively as described
above. Python's **106 tests** and stub check passed with the rebuilt binding.
The independent fake-camera C/Harvesters GenTL harness and this device's XML
parser/evaluator corpus checks also passed. Studio debug bundles were built and
exercised on hardware; no Studio public types or UI source changed in this PR.
Release packaging, notarization and distribution were not exercised here.

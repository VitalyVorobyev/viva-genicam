# GigE Vision hardware test plan

How to exercise every feature of the library, the CLI, the services, the
Python package, the GenTL producer and Viva Studio against one real GigE Vision
camera. CI has no camera, so this run is the only check that some of these
features get. It is written for any camera. Many rows in
[roadmap.md → Waiting on hardware](roadmap.md#waiting-on-hardware) are settled
by one area of it, and the **Settles** column names them.

The cases a test can judge are also a suite,
`crates/viva-genicam/tests/gige_hardware.rs`: run it first, and by hand the
cases it does not cover. USB3 Vision has its own,
`crates/viva-genicam/tests/u3v_hardware.rs`. Both are described in
[CONTRIBUTING → Tests](../CONTRIBUTING.md#tests). Studio's UI steps are in
[studio/manual-e2e-test.md](studio/manual-e2e-test.md); this plan refers to them
rather than repeating them.

## Before you start

- **Identify the camera.** Take a report and its XML:
  ```bash
  viva-camctl --iface <NIC> list
  viva-camctl report --ip <CAM> --iface <NIC> --out viva-report.txt
  viva-camctl xml    --ip <CAM> --iface <NIC> --out camera.xml
  ```
  The report's GVCP Capability decides which areas apply: pending ACK, packet
  resend, events and event data, action, heartbeat disable, the CCP
  application's port and IP, and switchover. So do the GigE Vision version and
  the stream channel count. An area the camera cannot exercise is recorded as
  not applicable, citing the report, not skipped silently.
- **Add the XML to the corpus** unless it is byte-identical to a corpus file
  (CLAUDE.md, *Vendor XML corpus*).
- **Record the link.** The NIC, its driver, the link speed and the MTU range the
  OS allows. On macOS, `networksetup -listValidMTURange <NIC>` shows the range;
  no jumbo frames means area E's large-packet cases are not applicable.
- **Save what you will change.** Run `viva-camctl save --ip <CAM> --out
  before.txt` and write down the persistent IP configuration. Put both back at
  the end. Every step below that writes non-volatile state says so.
- **Evidence.** Run every command with `-vv`, with a capture alongside:
  `sudo tcpdump -i <NIC> -w <area>-<case>.pcap udp`. The backlog rows ask for
  exactly these logs and captures.
- **One process on UDP 3956.** The fake-camera test suites bind it, so run none
  of them while testing hardware.

## Areas

Each case states what passes. **NV** marks a case that writes non-volatile
state.

### A. Discovery and IP configuration

| Case | Passes when | Settles |
|---|---|---|
| `list`, `--iface <NIC> list`, `list --ip <CAM>`, `list --allow-broadcast-ack` | Each finds the camera once, with the report's identity | |
| A camera on another subnet (its persistent IP outside the host's): `list`, then `list --allow-broadcast-ack` | The second finds it through the camera's broadcast ACK | TC-40 |
| `set-ip --mac <MAC> --ip <IP> --force`, with the MAC from the camera's label or a capture, when discovery finds nothing | The camera answers at the new address | |
| `set-ip --force` to an address on another subnet, then back | The camera answers at the new address, then at the old one; the ACK arrives on the NIC the camera answered on | |
| **NV** `set-ip` (persistent), power-cycle, `report` | The persistent address is in use after the restart; then restore the saved configuration | |
| Studio's *Set IP…* | [manual-e2e §6e](studio/manual-e2e-test.md#6e-a-camera-on-another-subnet-real-hardware) | |

### B. Control channel and session

| Case | Passes when | Settles |
|---|---|---|
| Open with `Access::Control`, then `Access::Monitor`; drop each | CCP reads as expected while held, and is released on drop | |
| Exclusive access (`--access exclusive`, `Access::Exclusive`), then a second process reads | CCP reads `0x1`; the second reader's answer is recorded | |
| Two applications: one holds control, a second `viva-camctl set` | The second is refused, and the first keeps control | TC-38 |
| A write from an application without control while nobody holds it (`gige_hardware` prints the answer) | The camera's answer is recorded | TC-38 |
| SIGSTOP the holder past `GevHeartbeatTimeout`, let a second take control, then SIGCONT and write | The first reports `ControlState::Lost`, at its next write at the latest on a camera that does not name its controller; the log shows what CCP read | TC-38 |
| A slow command (`UserSetSave`, `UserSetLoad`) with `-vv` and a capture | Any `PENDING_ACK` is captured, and its timeout field decoded both ways | TC-12, TC-24 |
| `VIVA_GIGE_FORCE_READMEM=1` with area C's sweep | Same values as with READREG | |

### C. GenApi across the whole node map

| Case | Passes when | Settles |
|---|---|---|
| `viva-camctl dump` | Every readable node reads; no node is skipped that `NodeMap::skipped()` does not list | |
| Write each RW node's own value back (every selector entry) | Each write succeeds and reads back the same | |
| Every enumeration's available entries, set and read back; an unavailable entry | Each is accepted; the unavailable one is refused | |
| `execute` each safe command, with and without `--no-wait` | `is_done` turns true before the timeout; a camera whose registers never clear is recorded | |
| `set` then `get` of a `WriteAround` feature, `-vv` (exposure and gain are `NoCache` on some cameras) | The read-back value is recorded: the new one or the old | |
| A register narrower than a word, written | The camera's answer to the widened WRITEMEM is recorded | TC-39 |
| Register nodes through `get_register`/`set_register` | Bytes match the capture | |

### D. Standard-feature helpers

| Case | Passes when | Settles |
|---|---|---|
| `exposure_time`/`set_exposure_time`, `gain`, `frame_rate` | They resolve the names the camera implements (`ExposureTimeAbs` on older SFNC), and the value changes the image | API-19 |
| `set_roi` growing and shrinking, `set_binning`, decimation | Geometry and `PayloadSize` follow, with no out-of-range write on the way | |
| `enable_trigger`/`trigger_software`/`disable_trigger` | One frame per software trigger | |
| `user_sets`, `load_user_set`; **NV** `save_user_set`, `set_default_user_set`, power-cycle | The loaded set's values appear; the saved set survives the restart; then restore | |

### E. Streaming

| Case | Passes when | Settles |
|---|---|---|
| `stream --save 1` in every pixel format the camera offers | Each saved frame looks right, and its row stride matches the leader | |
| An odd width in a packed format (`Mono12Packed`, `…12p`) | The capture's line or image padding is recorded and the frame decodes | PX-05 |
| Packet size Preserve, `--auto`, `--packet-size N`, and one above the path MTU | Each streams complete frames; the probe bisects down from the oversized one | |
| `--packet-delay-ns`, and `StreamBytesPerSecond` lowered | Throughput drops as set, with no loss. A camera may honour `GevSCPD` only in a bandwidth mode of its own (the Manta G-125B: `BandwidthControlMode` `SCPD`); record which | |
| Induced loss (a small `rcvbuf_bytes`, the highest frame rate) | `resend_requested` and `resend_recovered` both climb; the capture shows the request's source port | SR-24 |
| A resend for a block the camera no longer holds | The camera's answer (status, per packet or per range, or silence) is captured | SR-25 |
| AcquisitionStop/Start 100 times in one stream | No frame is lost to the block-id heuristics; the capture shows the ids across a restart | SR-23 |
| `--mode multicast`, plus a second process with `StreamOptions::monitor()` | Both receive every frame | |
| `bench --duration-s 300` | No incomplete frames on a clean link | |

### F. Acquisition modes and triggers

| Case | Passes when |
|---|---|
| Continuous, SingleFrame, MultiFrame (`AcquisitionFrameCount`), and a vendor's recorder mode (whose record trigger may accept only a hardware line) | Exactly the expected number of frames |
| Software trigger at a steady rate below the maximum | One frame per trigger, none lost |
| `TriggerDelay*`, with a hardware trigger where the camera applies the delay to those alone (the Manta G-125B's `TriggerDelayAbs` did not delay a software trigger) | Frame timestamps shift by the delay |
| Hardware trigger lines | Needs wiring; otherwise not applicable |

### G. Chunks

| Case | Passes when | Settles |
|---|---|---|
| `viva-camctl chunks --enable` (`--selectors ""` on a camera without `ChunkSelector`) with a capture of one frame | Raw chunks parse (`Frame::chunks`); `chunk_features` reads the values if the XML declares a chunk port, and the report says when it does not; the leader's payload type is recorded | TC-37 |

### H. Events

| Case | Passes when | Settles |
|---|---|---|
| `viva-camctl events --enable <each event the XML offers>` | Each arrives once per occurrence; event timestamps fall within the frame's exposure | TC-37 |
| An event that asks for an ACK | The ACK appears in the capture | |
| Event-data features read through `event_features` | No `BAD_ALIGNMENT`; values match the capture | TC-21 |
| Overflow (events faster than they are read) | The camera's overflow event, or its absence, is recorded | |

### I. Action commands

| Case | Passes when |
|---|---|
| `viva-camctl action`, broadcast and unicast, then scheduled | A camera with the action capability acknowledges and triggers; without it, its answer (silence or a status) is recorded |

### J. Time

| Case | Passes when |
|---|---|
| `time_calibrate`, `map_dev_ts` over an hour of streaming | Mapped frame times stay within the fit's residual; drift is recorded |
| `time_reset` | The device timestamp restarts near zero |

### K. Configuration files

| Case | Passes when | Settles |
|---|---|---|
| `save`, change features, `load`, `save` again, diff | The two saves match; the `LoadReport` lists no failure | |
| A file the vendor's tool saved, loaded with `load` | Loads, or the differences from ADR-0028's layout are recorded | GA-46 |

### L. Faults and endurance

| Case | Passes when |
|---|---|
| `soak_bench --duration 8h` at the highest frame rate | No incomplete frames on a clean link; resident memory and open files stay flat |
| Pull the cable mid-stream, then plug it back in | The stream ends as `Disconnected` within the heartbeat timeout; the library's and `viva-service`'s recovery are recorded |
| Power-cycle the camera mid-stream | As above |
| Two NICs up (the camera's and another) | Discovery, `--iface` and the stream use the camera's NIC |

### M. Every surface

| Case | Passes when |
|---|---|
| Every `viva-camctl` subcommand once | Each succeeds or fails with a message that names the cause |
| Python: `examples/` scripts, `stream` into NumPy | Same values as the CLI |
| GenTL: `scripts/build-cti.sh`, then a consumer on the camera | Frames and feature access through the producer |
| `viva-service --iface <NIC>` and Studio in remote mode | The [manual-e2e](studio/manual-e2e-test.md) checklist passes |
| Studio in embedded mode | The [manual-e2e](studio/manual-e2e-test.md) checklist passes with the real camera, including its real-hardware sections |

## Results

The [2026-10-25 bench record](hardware-2026-10-25.md) separates completed checks from unavailable or pending cases.

A run's evidence (logs, captures, values) goes into the backlog rows it
settles, and a defect it finds becomes a new row marked *Seen on hardware*.

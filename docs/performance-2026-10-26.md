# Acquisition and frame-delivery measurements, 2026-10-26

This report describes the socket optimization in #222 with its original
four-frame queue. The subsequent [delivery follow-up](hardware-rtvision-2026-10-26.md#production-delivery-default-2026-10-27)
validates the later sixteen-frame default; it does not change the measurements
below.

The target is Mono8 at 720 × 540 over 1 Gb/s Ethernet, plus maximum frame rate
with a small ROI. The receiver change reduces socket-wait overhead on Unix;
frame allocation, assembly rules and the four-frame delivery queue stay as they are.
Windows retains readiness polling and a nonblocking socket.

## Method

Apple M4 Pro, macOS 27.0.1, Rust release builds. Baseline library source is
`496f555`, whose receive engine is unchanged through `fcf4e1a`. Both versions
use the same `stream_perf` example, with sender and receiver in separate
processes. `/usr/bin/time -lp` measures receiver user + system CPU time;
sender CPU is excluded. No builds, other test suites or profilers ran during
timed measurements. Run order was baseline 10 s, drain pilot 10 s, timed-read
pilot 10 s, baseline repeat 10 s, final timed read 20 s, final baseline 20 s.
These are short measurements on one host, not a statistical confidence interval.

The sender generates existing scripted GVSP datagrams with MTU 1500, a standard
16-bit block ID that wraps, and a deterministic Mono8 pattern. The consumer
checks geometry, completeness, length and three pixel probes per frame. It does
not convert or render images. Frame pacing and optional 12.5 µs packet gaps are
approximate software timing; loopback does not reproduce NIC, wire or switch
behaviour. At 300 FPS, 720 × 540 Mono8 is 933.12 Mb/s of image payload alone.

Elapsed receive FPS uses the first and last received frames, excluding timeout
idle time after loss. Earlier failed pilot cases used timeout-inclusive FPS;
those FPS values are excluded here. CPU time in failed cases still includes
that idle timeout and is not treated as comparable loss-free throughput.

## Synthetic results

The final 20 s runs are below. CPU values are seconds of receiver CPU, not wall
time or percent of all machine cores. Backpressure means the oldest pending
frame was evicted from the fixed-capacity delivery queue. A row with any loss
is a failed integrity case, even if its average rate is close to the request.

| Mono8 size | Requested FPS | Packet pacing | Baseline CPU s | Candidate CPU s | Baseline/candidate backpressure | Baseline/candidate pass |
|---|---:|---|---:|---:|---:|---|
| 720 × 540 | 60 | burst | 4.07 | 3.71 | 0/0 | Passed/Passed |
| 720 × 540 | 300 | burst | 12.30 | 11.88 | 0/0 | Passed/Passed |
| 720 × 540 | 280 | 12.5 µs gap | 6.88 | 6.16 | 0/0 | Passed/Passed |
| 64 × 64 | 2,000 | burst | 5.05 | 4.46 | 0/1 | Passed/Failed |
| 64 × 64 | 10,000 | burst | 3.20 | 2.75 | 0/0 | Passed/Passed |
| 64 × 64 | 20,000 | burst | 6.72 | 5.67 | 1/0 | Failed/Passed |
| 1 × 1 | 10,000 | burst | 2.21 | 1.85 | 0/0 | Passed/Passed |
| 1 × 1 | 50,000 | burst | 12.49 | 11.54 | 52/84 | Failed/Failed |

All final cases had zero reported missing packets, incomplete frames, missed
block IDs, and bad sampled payloads; failures came from delivery backpressure.
For cases that passed on both sides, CPU fell 8.8% at 720 × 540/60 FPS, 3.4% at
720 × 540/300 FPS, 10.5% with paced 720 × 540/280 FPS, 14.1% at 64 × 64/10 kFPS,
and 16.3% at 1 × 1/10 kFPS. The burst difference is small and varies between
runs. The candidate delivered all 400,000 frames at 64 × 64/20 kFPS, but the
baseline lost one; this does not establish a universal sustainable ceiling.
The 2 kFPS and 50 kFPS cases showed occasional backpressure across the pilots
and final runs. Queue capacity and host scheduling make average FPS alone an
insufficient guarantee. No loss-free 50 kFPS claim is made.

## Profile and decisions

A separate macOS `sample` of the baseline at 720 × 540/300 FPS showed receiver
wall stacks mostly in readiness polling and `recvfrom`; the consumer mostly
waited in `kevent`. These samples include sleeping time and are not CPU
percentages. Allocation stacks were sparse, so the profile does not justify
SR-27's proposed pool. The earlier bounded-RSS endurance data also does not
establish an allocation leak.

A first nonblocking drain-loop pilot improved small frames but regressed the
720 × 540 cases; it was discarded. The selected change uses Unix receive and
send timeouts, avoiding a separate readiness wait on every datagram. Both
operations remain bounded by OS timeout handling; if timeout setup fails, the
engine falls back to readiness polling. Timers, configuration changes, source
filtering and cancellation are still checked per datagram, including under
continuous foreign traffic. Resend batches may contain several individually
bounded writes; this is not a hard real-time shutdown guarantee.

POSIX specifies blocking receive and timeout behaviour ([recv](https://pubs.opengroup.org/onlinepubs/9799919799/functions/recv.html)).
Rust documents platform-specific timeout errors ([UdpSocket](https://doc.rust-lang.org/stable/std/net/struct.UdpSocket.html#method.set_read_timeout)).
Winsock documents an expired blocking receive differently, so Windows keeps
its existing polling implementation ([socket options](https://learn.microsoft.com/en-us/windows/win32/winsock/sol-socket-socket-options)).
Windows and Linux performance remain unmeasured in this report.

No buffer pool, deferred counters, larger queue, or generic conditional wake
was introduced. The event queue permits multiple consumers, so waking only
on empty-to-nonempty could strand a waiting event consumer.

## Reproduce the synthetic case

Build once, then run these in two terminals. Wait for `READY` before sending:

```sh
cargo build --release --locked -p viva-genicam --example stream_perf
/usr/bin/time -lp target/release/examples/stream_perf receive 127.0.0.1:55000 720 540 6000 25 target/stream-perf.json
```

```sh
target/release/examples/stream_perf send 127.0.0.1:55000 720 540 300 6000 0
```

The time invocation above is macOS-specific. For paced packets replace the final
`0` with `12.5`; choose a frame rate whose packet train fits within each frame.
Compare identical harnesses and release settings; do not run builds or other
suites during measurement. Raw local evidence, binary hashes, drivers, profiles
and per-case JSON live outside the build tree, linked locally from ignored
`.local-evidence/performance-2026-10-26/`. No vendor XML or private captures are distributed.

## Manta G-125B over 1 Gb/s Ethernet

Camera 192.168.2.25, MAC 00:0f:31:4f:2b:17, interface en10, MTU 1500.
Each case runs 30 s, Mono8, minimum exposure 21 µs, zero offsets, binning and
decimation 1. The requested rate is the camera's reported
`AcquisitionFrameRateLimit` after setting the ROI. `StreamBytesPerSecond`
remains 115 MB/s. This is a sensor-limited camera: the 720 × 540 case does not
saturate 1 Gb/s.

| ROI | Camera limit FPS | Baseline/candidate complete frames | Baseline/candidate CLI CPU s | Result |
|---|---:|---:|---:|---|
| 720 × 540 | 48.936 | 1,468/1,468 | 1.62/1.37 | Passed |
| 64 × 64 | 142.288 | 4,269/4,269 | 1.56/1.44 | Passed |
| 1 × 1 | 190.367 | 5,711/5,711 | 1.26/1.11 | Passed |

All six unbinned runs reported zero incomplete, missed and backpressure frames,
missing packets and resend requests. CLI CPU includes control/XML setup and
shutdown as well as the 30 s stream; these single hardware trials are not a
precise standalone receive-thread measurement. Saved configuration restoration
passed with byte-identical values after both baseline and candidate batches.

An additional candidate batch used maximum declared binning (8 horizontal,
14 vertical). Its 1 × 1 and 64 × 64 cases still reported limits of 190.367 and
142.288 FPS, delivering 5,711 and 4,268 complete frames respectively without
reported loss. Binning did not increase those limits and changes the image's
sampling. Its fresh saved configuration was also restored byte-identically.
No persistent camera network or host NIC settings were changed. These
restoration checks concern fresh performance-test baselines; they do not
resolve the missing original eight-hour baseline described in the
[earlier endurance record](hardware-2026-10-25.md#endurance).

## Regression validation

The candidate passed 1,066 root tests, including receive-after-idle and timer/
shutdown checks under continuous foreign traffic, plus the 30 scripted GVSP
engine cases. Root formatting, clippy with denied warnings, all-feature
warning-free documentation and both WebAssembly checks passed.

The opt-in Manta hardware suite passed **25/25 tests in 77.84 s**, covering
acquisition, formats, resend, chunks, events and feature access. Capability-
guarded steps still report unsupported camera features in the raw log. An
outer fresh-baseline wrapper then loaded and saved successfully and verified
byte-identical saved configuration. Raw results are in
`hardware-final-suite/result.json` and `suite.log` under the local evidence
link. No endurance test was started for this performance batch; further
long-duration qualification is deferred.


## Delivery-queue follow-up

A separate set of 44 five-second loopback runs compared the current four-frame
queue with an experimental async-pop fast path at capacities 4, 16 and 32.
Binaries, source hashes, per-run CPU measurements and JSON remain in ignored
`.local-evidence/delivery-2026-10-26/`; `pilot-build-manifest.json` identifies
all four variants. Sender and receiver were separate processes, with no builds
or other tests during timing. These are scheduling probes, not NIC measurements.

| Case | Baseline CPU s, median of 3 | Fast path at capacity 4, median of 3 |
|---|---:|---:|
| 720 × 540, 280 FPS, paced packets | 1.52 | 1.52 |
| 64 × 64, 10,000 FPS, async | 0.67 | 0.66 |
| 24 × 16, 2,700 FPS, async | 0.83 | 0.81 |
| 24 × 16, 2,700 FPS, blocking | 0.81 | 0.82 |
| 1944 × 56, 762 FPS, blocking | 2.39 | 2.38 |

All ordinary runs at all tested capacities reported no backpressure. In one
controlled 3 ms consumer-pause trial per variant at 2,700 FPS, both capacity-4
variants dropped four frames, while capacities 16 and 32 dropped none. This
supports a scheduling-budget explanation: four frames span about 1.48 ms at
that rate. It does not establish higher sustainable throughput or a lower CPU
cost. The fast path was discarded and these initial probes left capacity at four.
The later [hardware follow-up](hardware-rtvision-2026-10-26.md#high-rate-delivery)
completes the NIC comparison and the production change (SR-28).

The previous hardware Python loop first imported NumPy during acquisition.
Three fresh-process conversion probes with a synthetic 1944 × 56 frame measured
16.5–31.8 ms for the first cold `to_numpy()`, versus 3.7–4.6 µs after importing
NumPy before conversion. Steady conversions took about 1.2 µs; a separate
`payload()` copy took about 1.0 µs. These probes isolate startup and conversion,
not acquisition. Warm NumPy before starting a timed stream, measure conversion
separately from a duplicate payload copy, and repeat on hardware before
attributing its earlier drops to steady Python overhead.

The adapter was absent at the start of 2026-10-27; that software-only batch
changed no camera/NIC configuration and started no long test. The adapter
returned later that day. The [hardware follow-up](hardware-rtvision-2026-10-26.md#studio-and-delivery-follow-up-2026-10-27)
records the completed short native Studio checks, actual-NIC queue comparison,
Python cold/warm conversion and configuration restoration.

# Streaming

Goal of this tutorial:

- Start a **GVSP** stream from your camera.
- See how to:
  - Run streaming from the `viva-camctl` CLI.
  - Run a basic streaming example from the `viva-genicam` crate.
- Understand the key knobs for stability:
  - Packet size / MTU
  - Packet delay
  - Resends and backpressure

You should already have:

- Completed [Discovery](./discovery.md) and know:
  - The camera IP address (e.g. `192.168.0.10`).
  - The host NIC / interface used for the camera (e.g. `192.168.0.5`).
- Ideally gone through [Registers & features](./registers.md) so you can
  configure basic camera settings.

---

## 1. Basics: how GVSP streaming works

Very simplified:

1. On the **control path** (GVCP / GenCP), you configure:
   - Pixel format, ROI, exposure, etc.
   - Streaming destination (host IP / port).
   - Whether the camera uses resends, chunk data, etc.
2. When you tell the camera to start acquisition, it begins sending:
   - GVSP **data packets** (your image payload).
   - Occasionally **leader/trailer** or **event** packets, depending on mode.
3. The host reassembles packets into complete frames, handles resends and
   timeouts, and exposes a stream of “frames + stats” to you.

The `viva-gige` crate owns the low-level GVSP packet handling. The `viva-genicam`
crate builds on that to present a higher-level streaming API. `viva-camctl`
wraps `viva-genicam` in a CLI.

---

## 2. Streaming with `viva-camctl`

The exact flags may evolve; always check:

```bash
cargo run -p viva-camctl -- stream --help
```

for the authoritative list. The examples below illustrate the typical usage
pattern.

### 2.1. Start a basic stream

Start a stream from a camera at 192.168.0.10 using the host interface
192.168.0.5:

```bash
cargo run -p viva-camctl -- \
  stream --ip 192.168.0.10 --iface 192.168.0.5
```

What you should expect:
- A textual status showing:
- Frames received.
- Drops / incomplete frames.
- Resend statistics (if the camera supports resends).
- Measured throughput (MB/s or similar).
- The tool may run until you interrupt it (Ctrl+C), or it may have:
- A --count option (receive N frames).
- A --duration option (run for N seconds).

If you see no frames:
- Double-check that streaming is enabled on the camera.
- Ensure you haven’t configured a different destination IP / port in a vendor
tool.
- Make sure the iface IP you pass is the one the camera can reach.

### 2.2. Saving frames to disk

Many users want to save frames as a quick sanity check or for offline
analysis. If viva-camctl stream exposes options like --output / --dir /
--save, use them; for example:

```bash
cargo run -p viva-camctl -- \
  stream --ip 192.168.0.10 --iface 192.168.0.5 \
  --count 100 --output ./frames
```

Typical behaviour:
- Create a directory.
- Save each frame as:
- Raw bytes (e.g. .raw), or
- PGM/PPM (.pgm / .ppm), or
- Some simple container format.

If you are unsure which formats are supported, check --help or the
viva-camctl crate documentation.

Saved frames are useful to:
- Inspect pixel data in an image viewer or with Python/OpenCV.
- Compare against the vendor’s viewer for debugging.

⸻

## 3. Streaming from Rust using genicam

The genicam crate usually offers one or more streaming examples (search for
stream_ in crates/viva-genicam/examples/).

Run the simplest one, for example:

```bash
cargo run -p viva-genicam --example stream_basic
```

(If the actual example name differs, adapt accordingly.)

What such an example typically does:
1.	Open a device (by IP or index).
2.	Configure basic streaming parameters if needed (pixel format, ROI, exposure).
3.	Build a stream (e.g. using a StreamBuilder or similar).
4.	Start acquisition and iterate over frames in a loop.
5.	Print per-frame stats or a summary.

A typical pseudo-flow (simplified, not exact code):

```rust
// Pseudocode sketch — see the actual example for real API
use genicam::prelude::*;

fn main() -> anyhow::Result<()> {
    // 1. Context and device
    let mut ctx = Context::new()?;
    let mut dev = ctx.open_by_ip("192.168.0.10".parse()?)?;

    // 2. Optional: tweak features before streaming
    let mut nodemap = dev.nodemap()?;
    nodemap.set_enum("PixelFormat", "Mono8")?;
    nodemap.set_float("AcquisitionFrameRate", 30.0)?;

    // 3. Build a stream
    let mut stream = dev.build_stream()?.start()?;

    // 4. Receive frames in a loop
    for (i, frame) in stream.iter().enumerate() {
        let frame = frame?;
        println!(
            "Frame #{i}: {} x {}, ts={:?}, drops={} resends={}",
            frame.width(),
            frame.height(),
            frame.timestamp(),
            frame.stats().dropped_frames,
            frame.stats().resends,
        );

        if i >= 99 {
            break;
        }
    }

    Ok(())
}
```

Use the real example as the ground truth for API names and error handling.

⸻

## 4. Tuning for stability and performance

Streaming is where GigE Vision setup matters most. A few knobs you will
encounter (some via camera features, some via host configuration):

### 4.1. Packet size and MTU
- Packet size too large for your NIC / path MTU:
- Packets get fragmented or dropped.
- High drop/resend counts.
- Packet size too small:
- More packets per frame → more overhead.
- Higher chance of bottleneck at CPU or driver level.

Typical approach:
- Enable jumbo frames on the camera network (e.g. MTU 9000) if your switch/NIC
support it.
- Set camera packet size slightly below MTU (e.g. 8192 for MTU 9000).
- Observe throughput and drop/resend statistics.

### 4.2. Packet delay (inter-packet gap)

Some cameras allow setting an inter-packet delay or packet interval:
- Too little delay:
- Bursty traffic, easily overloading switches, NICs, or host buffers.
- Modest delay:
- Smoother traffic at the cost of slightly higher end-to-end latency.

If your stats show frequent drops/resends at high frame rates:
- Try increasing the packet delay slightly.
- Monitor if drops/resends go down while throughput remains acceptable.

### 4.3. Resends and backpressure

GVSP supports packet resends:
- The host tracks missing packets in a frame.
- It requests resends from the camera.
- The camera re-sends the missing packets.

The viva-gige layer surfaces statistics like:
- Dropped packets.
- Number of resend requests.
- Number of resent packets actually received.

Use these metrics to:
- Detect whether your current network configuration is “healthy”.
- Compare different NICs, cables, or switches.

⸻

## 5. Troubleshooting streaming issues

If streaming starts but is unreliable, here is a practical checklist:
1.	Packet drops / resends spike immediately
	- Check MTU and packet size alignment.
	- Try lowering frame rate or resolution temporarily.
	- Use a dedicated NIC and switch if possible.
2.	No frames arrive, but discovery and feature access work
	- Confirm the camera is configured to send to your host IP / port.
	- Ensure no other tool (vendor viewer) is already consuming the stream.
	- Double-check any firewall rules that might block UDP on the stream port.
3.	Frames arrive but with wrong size or format
	- Verify PixelFormat and ROI in the NodeMap (viva-camctl get / set).
	- Confirm your code interprets the buffer layout correctly (Mono8 vs Bayer).
4.	Intermittent hiccups under load
	- Look at CPU usage and other traffic on the same NIC.
	- Consider enabling jumbo frames and increasing packet delay.
	- On Windows, ensure high-performance power profile and up-to-date NIC drivers.

When in doubt:
- Save a small sequence of frames to disk.
- Capture logs at a higher verbosity (e.g. RUST_LOG=debug).
- Compare behaviour with the vendor’s viewer using the same network setup.

⸻

## 6. Recap

After this tutorial you should be able to:
- Start a GVSP stream using viva-camctl.
- Run a streaming example from the viva-genicam crate.
- Interpret basic streaming stats (frames, drops, resends, throughput).
- Know which knobs to tweak first when streaming is unreliable:
- MTU, packet size, packet delay, frame rate, dedicated NIC.

For more detailed background on how GVCP/GVSP packets are handled internally,
see the [viva-gige￼crate chapter](../crates/viva-gige.md).

Next steps:
- [Networking](../networking.md) — a more systematic look at NIC configuration,
MTU, and common deployment topologies.
- Later: dedicated crate chapters for `viva-gige` and `viva-genicam` for contributor-level details.

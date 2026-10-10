# Manual E2E Test: Service + Fake Camera + Studio

No external tools required. Uses the built-in `viva-fake-gige` camera simulator.

## Setup (3 terminals)

All commands run from the repository root.

**Terminal 1 -- fake camera (stays alive until Ctrl+C):**
```bash
cargo run -p viva-fake-gige
```

Output:
```
Fake camera running on 127.0.0.1:3956 (640x480 Mono8 @ 30 fps)
Press Ctrl+C to stop.
```

Custom dimensions:
```bash
cargo run -p viva-fake-gige -- --width 512 --height 512 --fps 15
```

**Terminal 2 -- camera service:**
```bash
RUST_LOG=viva_service=debug,viva_genicam=info,warn \
  cargo run -p viva-service -- \
    --iface lo0 \
    --zenoh-config studio/config/zenoh-local.json5 \
    -v
```

> On Linux, use `--iface lo` instead of `--iface lo0`.
>
> The `--zenoh-config` flag is **required** for local testing. It makes the
> service listen on `tcp/127.0.0.1:7447` so the studio can connect. Without
> it, the service uses Zenoh multicast scouting which often fails on macOS.

**Terminal 3 -- studio:**
```bash
# Required, and absolute: the `cd` below would break a relative path.
export ZENOH_CONFIG=$(pwd)/studio/config/zenoh-studio.json5
cd studio/apps/viva-studio-tauri
RUST_LOG=viva_studio_tauri=info,viva_streamer=debug,warn cargo tauri dev
```

> The studio does **not** load a Zenoh config on its own — there is no default
> path and no dev-mode auto-detection. Without `ZENOH_CONFIG` it starts in
> embedded mode, which never scans loopback, so this fake camera cannot show up
> (#132). The header chip tells you which mode you got.

## Quick verification with CLI (optional, no studio needed)

With Terminal 1 running, open a new terminal:
```bash
cargo run -p viva-camctl -- list --iface 127.0.0.1
cargo run -p viva-camctl -- get --ip 127.0.0.1 --name Width
cargo run -p viva-camctl -- set --ip 127.0.0.1 --name Width --value 320
cargo run -p viva-camctl -- get --ip 127.0.0.1 --name Width   # should show 320
```

## Test checklist

### 1. Discovery
- [ ] Device appears in sidebar within ~5s
- [ ] Shows name "VivaCam Fake", vendor "vitavision.dev"
- [ ] Device ID shows `cam-deadbeefcafe` (fake MAC DE:AD:BE:EF:CA:FE)

### 2. Connect
- [ ] Click device -> feature tree loads
- [ ] Width=640, Height=480 visible in tree (or 512x512 if custom)
- [ ] DeviceModelName = "VivaCam Fake"
- [ ] PixelFormat = "Mono8"
- [ ] ExposureTime, Gain visible

### 3. Node writes
- [ ] Select Width and PixelFormat without editing -> no "Apply N" button
      appears, and PixelFormat shows its current entry with no error
- [ ] Change Width to 320 -> readback shows 320
- [ ] Change Height to 240 -> readback shows 240
- [ ] Change PixelFormat (Mono8 -> RGB8)
- [ ] Change ExposureTime to 10000
- [ ] Restore original values

### 4. Acquisition
- [ ] Click Start -> image appears in viewer (gradient pattern)
- [ ] FPS counter shows non-zero value
- [ ] Status bar shows resolution and pixel format
- [ ] Image updates with each frame
- [ ] On a real camera: set AcquisitionMode to SingleFrame and click Start ->
      the one frame is shown (the fake streams continuously whatever the mode)

### 5. Stream tools
- [ ] Click "1:1", then zoom further -> single camera pixels show as sharp
      squares, not a blur
- [ ] Open the histogram -> 0 and 255 under the plot, and min/max/mean below;
      overexpose -> "% at 255" appears
- [ ] Zoom in, switch to the Feature Browser and back -> the zoom and the
      stream are as they were
- [ ] Zoom/pan with scroll wheel and drag
- [ ] Histogram toggle works (if available)
- [ ] Pixel inspector shows coordinates on hover

### 6. Snapshot and the Image tab's controls
- [ ] Click the camera icon -> the saved PNG matches the viewer (not shifted
      or striped), for Mono8 and for RGB8
- [ ] Type an exposure value but do not leave the box -> it stays while frames
      and other features update
- [ ] Click into the exposure box without typing, then change Exposure Auto ->
      the box follows the camera, and clicking away writes nothing
- [ ] Set Exposure Auto to Continuous -> Exposure Time greys out and reads
      "Read-only now"; back to Off -> editable again
- [ ] Turn Acquisition Frame Rate Enable off -> Acquisition Frame Rate greys
      out as not available
- [ ] Set Packet size to Fixed, 1400, then Stop and Start -> Transport Layer
      shows GevSCPSPacketSize 1400; back to Auto -> the negotiated size
- [ ] Edit a feature in the Feature Browser without applying, switch to the
      Image tab, press Ctrl+Enter -> nothing is written

### 6a. Live state in the Feature Browser
- [ ] Open a category -> its rows fill in with values (only that category's
      features are read)
- [ ] With Exposure Time selected, change Exposure Auto in the sidebar ->
      the panel shows RO and disables the editor without pressing refresh
- [ ] Set SensorType to Color -> PixelFormat offers only RGB8
- [ ] Export State -> the JSON holds every feature with a value, registers
      left out

### 6c. Camera panel, user sets and dropped frames
- [ ] Diagnostics tab while connected -> "Camera" lists the model, serial and
      firmware, the MAC and the IP address
- [ ] Persistent IP: enter 192.168.1.100 / 255.255.255.0 / 192.168.1.1, Save to
      camera -> a toast says it applies from the next restart;
      `viva-camctl` (or a reconnect) reads it back
- [ ] Remote mode -> the panel says IP configuration is not supported there
- [ ] Image tab -> User Sets: choose a set, Execute Load -> the features it
      holds change
- [ ] While streaming, the header shows the camera's fps in both modes; on a
      real camera over a lossy link it adds "· N dropped" in amber

### 6d. Feature Browser on a large camera
- [ ] Open a category with many features and scroll -> values fill in as
      rows come into view
- [ ] Search for a common word ("Gain", "Selector") -> results appear at once;
      over 200 matches says "First 200 of N matches"
- [ ] Focus a list, use the arrow keys, Home and End -> focus moves through the
      options; Enter opens the focused one
- [ ] Focus a splitter (Tab) and use the arrow keys -> the pane resizes
- [ ] Select a category two levels deep -> the breadcrumb shows both levels

### 6e. A camera on another subnet (real hardware)
- [ ] Put a GigE camera on a static IP outside the host's subnet (or connect
      it to a link-local NIC) -> it appears in the device list but fails to
      connect
- [ ] "Set IP…", enter a free address in the host's subnet, Send -> a toast
      confirms; at the next scan the camera shows the new address and connects
- [ ] With two network cards, the camera on the non-default one -> the same
      works (ForceIP leaves through the interface the camera answered on)

### 6f. A USB3 Vision camera in embedded mode (real hardware)
- [ ] Plug in a USB3 Vision camera, start Studio without `ZENOH_CONFIG` -> it
      appears in the device list with a USB3 badge
- [ ] Connect -> features read and write; Start -> frames and a frame rate
- [ ] Diagnostics -> the Camera panel says IP configuration is GigE's; Save
      report… -> the report's Camera section says "transport: USB3 Vision"
      and holds the camera's XML
- [ ] Linux: without a udev rule granting access, the camera is missing or
      fails to open — note which

### 6b. Report
- [ ] Diagnostics tab -> Save report… -> the file holds
      "# viva-genicam diagnostic report" and a "## Camera" section on the
      connected camera; with "Include the camera's GenApi XML" ticked its
      "## GenApi XML" section holds the XML, unticked it says it was omitted
- [ ] Copy -> the clipboard holds the same text
- [ ] Remote mode: the report says the service wrote it
- [ ] The Feature Browser's toolbar reads "N nodes · M cat" with no
      "not shown" for the fake camera's XML

### 7. Stop & disconnect
- [ ] Click Stop -> frames stop, FPS goes to 0
- [ ] Disconnect device -> feature tree clears
- [ ] Reconnect -> features reload

### 8. Service crash recovery
- [ ] While connected, kill Terminal 2 (Ctrl+C)
- [ ] Studio shows reconnecting banner (or error state)
- [ ] Restart service in Terminal 2
- [ ] Studio reconnects automatically

### 9. A release bundle (after a tag)
- [ ] macOS: the `.dmg` from the GitHub release installs, and opens after
      *Open Anyway* in Privacy & Security; the About box and the report say
      the library's version
- [ ] Windows: the `.msi` installs past SmartScreen's *Run anyway*, and a GigE
      camera is discovered once the firewall prompt is allowed
- [ ] Linux: the `.deb` installs with `apt` and the AppImage runs

## Fake camera features

| Feature | Supported |
|---------|-----------|
| Discovery (GVCP broadcast) | Yes |
| CCP (Control Channel Privilege) | Yes |
| GenApi XML (20+ nodes) | Yes |
| Width/Height/PixelFormat/ExposureTime/Gain | Yes (RW) |
| DeviceModelName/VendorName/SerialNumber | Yes (RO) |
| OffsetX/OffsetY | Yes (RW) |
| AcquisitionStart/Stop | Yes |
| GVSP streaming (Mono8, RGB8) | Yes |
| Device timestamps (1 GHz) | Yes |
| Chunk data (Timestamp, ExposureTime) | Yes (when ChunkModeActive=1) |
| Multicast | No |
| Events / Action commands | No |

## CLI flags (`viva-fake-gige`)

| Flag | Default | Description |
|------|---------|-------------|
| `--width` | 640 | Image width in pixels |
| `--height` | 480 | Image height in pixels |
| `--fps` | 30 | Target frame rate |
| `--bind` | 127.0.0.1 | Bind address |
| `--port` | 3956 | GVCP port |

## Debug checkpoints

If the image view stays blank, check the logs:

- **Terminal 1**: confirm `Fake camera running on ...` is shown
- **Terminal 2 (viva-service)**: look for `connected` and `first GVSP frame received`
- **Terminal 3 (viva_streamer)**: look for `First image/meta received`,
  `First raw image frame received`, `First BMP frame published`
- **Browser/Tauri console**: look for `WebSocket opened` and
  `First binary frame received`

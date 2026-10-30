# Manual E2E Test: Service + Fake Camera + Studio

No external tools required. Uses the built-in `viva-fake-gige` camera simulator.

## Setup

Start the fake camera, `viva-service` and Studio in remote mode as in
[`testing-cookbook.md`](testing-cookbook.md#quick-start-service--fake-camera).
For the debug checkpoints below, give the service
`RUST_LOG=viva_service=debug,viva_genicam=info,warn` and Studio
`RUST_LOG=viva_studio_tauri=info,viva_streamer=debug,warn`.

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

### 6b. Compact preview and replacement controls
- [ ] Features tab: **Image preview** starts collapsed. Expanding it does not
      start acquisition; **Start Acq** keeps this tab visible when expanded
- [ ] Change exposure/gain with the preview visible; image and readback change.
      Stop, fold/unfold and switch Image/Features: no second preview socket or
      decoding in the hidden canvas; the last SingleFrame remains visible
- [ ] Image sidebar: acquisition, exposure/gain, then statistics. On cameras
      with legacy names, absolute/raw controls show their declared units and
      ranges, including read-only/auto restrictions
- [ ] Remote mode: software-reset the secured test camera, let the service
      recover, then verify refreshed controls and acquisition without manually
      disconnecting Studio. Restore every changed feature and network setting
- [ ] Embedded mode: with a camera near 31 FPS, display approaches its 30 FPS
      target rather than halving to 15–16 FPS; acquisition counters stay intact

### 6b.1 Embedded connection recovery

- [ ] With a GigE camera idle, make the camera unreachable: the header reports
      the loss and offers **Cancel reconnect**. Repeat while acquiring: the
      old preview closes rather than remaining a live-looking frozen image
- [ ] Restore connectivity within 30 seconds: the same MAC reconnects, current
      features reload and acquisition remains stopped. **Start Acq** then
      receives frames. Record a software reset or NIC transition separately
      from a physical cable pull or power cycle
- [ ] Cancel reconnect, disconnect, switch camera or exit while recovery is
      pending: no delayed replacement or control claim remains afterward
- [ ] Let recovery expire: a visible error offers manual reconnect
- [ ] Let another application take control without losing reachability: the
      **Camera control lost** banner appears, controls become read-only, and
      there is no automatic reclaim loop. **Reconnect with control** is explicit
- [ ] Disconnect a USB camera while receiving: reader shutdown completes and
      manual reconnect is offered; normal Stop and finite acquisition do not
      trigger camera recovery

### 6c. Camera panel, user sets and stream statistics
- [ ] Diagnostics tab while connected -> "Camera" lists the model, serial and
      firmware, the MAC and the IP address
- [ ] Persistent IP: enter 192.168.1.100 / 255.255.255.0 / 192.168.1.1, Save to
      camera -> a toast says it applies from the next restart;
      `viva-camctl` (or a reconnect) reads it back
- [ ] Remote mode -> the panel says IP configuration is not supported there
- [ ] Image tab -> User Sets: choose a set, Execute Load -> the features it
      holds change
- [ ] While streaming, the header shows the camera's fps in both modes; on a
      real camera over a lossy link it adds "· N lost" in amber, and its
      tooltip says how many were incomplete, missed and dropped by the host
- [ ] Image tab -> Statistics: at a high frame rate Received climbs at the
      camera's rate while Displayed and the status bar's "display" rate stay
      near 30 (15 remote); switch tabs and back -> Received kept counting
- [ ] Statistics shows Packets and Resend for a GigE camera, USB with
      Transfers and Transfer errors for a USB3 Vision one
- [ ] Stop -> Statistics keeps its values under "Stopped"; Start -> they
      count from zero

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

### 6g. Camera events
- [ ] Diagnostics tab -> Camera events lists the fake's entries
      (StartOfTransfer, EndOfTransfer, ExposureEnd)
- [ ] Tick ExposureEnd -> Start -> "Monitoring ExposureEnd"; start
      acquisition -> events arrive newest first, named ExposureEnd with ID
      0x9001; expanding one shows `EventExposureEndFrameID` equal to its block
      and `EventExposureEndTimestamp` equal to its ticks
- [ ] Stop -> "Not monitoring" with no warning; the Features tab's
      EventSelector = ExposureEnd shows EventNotification Off again
- [ ] Start again, then Disconnect -> reconnect: EventNotification is Off
- [ ] `viva-service-u3v --fake`: the same, and an event shows no block
- [ ] A USB3 Vision camera in embedded mode (real hardware): the same; unplugging
      it while monitoring shows a warning that the event settings were not
      restored

### 6h. Configuration files, registers and action commands
- [ ] Diagnostics tab -> Configuration -> Save to file… -> "Saved N values to
      <path>"; the file starts with
      `# {05D8C294-F295-4dfb-9D01-096BD04049F4}` and holds `ExposureTime`
- [ ] Change ExposureTime in the Features tab, then Load from file… the saved
      file -> "Restored N values", no failures, and ExposureTime on screen shows
      the saved value again without a manual refresh
- [ ] Load a file saved from another model, or edit its `ModelName` comment ->
      it loads, with a warning naming the model
- [ ] While acquiring, the panel says locked features are left out of a save;
      a save then lists Width, Height and PixelFormat under "Left out, and why"
- [ ] Features tab -> DeviceUserBuffer shows 16 bytes as a hex dump with an
      ASCII column; typing 3 bytes shows "3 of 16 bytes" and Write stays
      disabled; 16 bytes -> Write -> the dump shows them after the read-back
- [ ] Monitor or control-lost session: Save, Load and Write are disabled, and
      the dump still reads
- [ ] Remote mode: Action command -> "Send an action command…" explains that
      action commands require embedded mode
- [ ] Embedded mode with a GigE camera (real hardware): the destination is the
      camera network's broadcast address; with the camera's keys Send ->
      "acknowledged by 1 device"; a camera without action support is listed as
      refused with its status

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
| Events (StartOfTransfer, EndOfTransfer, ExposureEnd, while streaming) | Yes |
| Action commands (device key 0x42, group key 0x1, group mask 0x1) | Yes |
| A `<Register>` node (DeviceUserBuffer, 16 bytes) | Yes |

## CLI flags (`viva-fake-gige`)

`cargo run -p viva-fake-gige -- --help` lists them: size, frame rate, pixel
format, bind address and port, heartbeat enforcement, a packet-size cap, and
the identity (MAC, model, serial, user name).

## Debug checkpoints

If the image view stays blank, check the logs:

- **Terminal 1**: confirm `Fake camera running on ...` is shown
- **Terminal 2 (viva-service)**: look for `connected` and `first frame received`
- **Terminal 3 (viva_streamer)**: look for `First image/meta received`,
  `First raw image frame received`, `First BMP frame published`
- **Browser/Tauri console**: look for `WebSocket opened` and
  `First binary frame received`

## Direct GigE monitor and subnet discovery

These are short hardware checks for the next available device; software fake
coverage does not close ST-62.

- [ ] A camera on another subnet appears with **Different subnet** and **Set IP…**;
      a routed connection can still be attempted. Remote mode exposes no direct
      monitor or temporary-IP action, and USB monitor is labelled unavailable
- [ ] Another application holds control and configures a multicast stream:
      **Monitor** connects, reads features and offers **Watch stream**; commands,
      feature writes, persistent-IP edits and packet-size changes are disabled
- [ ] Watch receives an image; **Stop watching**, Disconnect and app exit leave
      the controller acquiring. Packet capture shows no monitor claims, writes
      or resend requests, including with induced packet loss
- [ ] A unicast stream produces a clear multicast requirement rather than
      silently reconfiguring the camera
- [ ] After a brief connection interruption, the same MAC reconnects as Monitor;
      its viewer remains stopped, and Watch works again without touching the
      controller's acquisition or settings


## Frame metadata and chunks (ST-52)

Hardware qualification is open as ST-63; software fixture results do not settle
these native-device cases.

- Start a plain GigE or USB camera stream; expand **Frame metadata**. Check
  frame ID, device ticks (when supplied), completeness and missing calibration.
- Enable a supported chunk selector explicitly through **Chunk Data**. Check
  the chunk IDs and XML-derived values against the camera's own data, including
  visible errors for an unreadable chunk feature. Disable it and verify stale
  values disappear on the next displayed frame.
- Freeze while frame IDs change. Metadata must stay with the frozen preview,
  native histogram and samples until Resume, then follow live frames again.
- Switch cameras or recover a session during a delayed preview decode. No
  previous frame metadata may appear on the replacement viewer.
- Repeat through matching API 8 services and Studio, then in multicast Monitor
  mode. Monitor inspection must not write chunk settings or acquisition state.
- Snapshot and verify restoration of any configuration deliberately changed
  during testing. Do not treat disconnecting as configuration restoration.

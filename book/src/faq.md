# FAQ

This page collects short answers to questions that come up often when using
`viva-genicam` or bringing up a new camera.

If you are stuck, also check:

- [Discovery](./tutorials/discovery.md)
- [Streaming](./tutorials/streaming.md)
- [Networking](./networking.md)

and the issues in the GitHub repository.

---

## “Discovery finds no cameras. What do I check first?”

Run:

```bash
cargo run -p viva-camctl -- list
```

If it shows nothing:
1.	Physical link
	- Are the link LEDs lit on camera, NIC, and switch?
	- Try a different cable or port.
2.	IP addresses
	- Host NIC and camera must be on the same subnet (e.g. 192.168.0.x/24).
	- Avoid having two NICs on the same subnet; routing will get confused.
3.	Firewall
    - Allow UDP broadcast/unicast on the NIC used for cameras.
    - On Windows, make sure the binary is allowed on the relevant network
profile (Private / Domain).
4.	Multiple NICs
	- Use --iface <host-ip> to force the interface:

```bash
cargo run -p viva-camctl -- list --iface 192.168.0.5
```

See also: [Discovery tutorial](./tutorials/discovery.md) and
[Networking](./networking.md).

⸻

## “The vendor viewer works but viva-genicam doesn’t. Why?”

Common causes:
- Different NIC / interface:
    - The vendor tool may be using a different NIC or IP selection strategy.
    - Compare which local IP it uses and pass that as --iface to viva-camctl.
- Different stream destination:
    - The camera might be configured to stream to a specific IP/port.
    - Ensure viva-genicam uses the same host IP and port, or reset the camera
configuration to defaults.
- Different MTU / packet size / packet delay:
	- Vendor tools sometimes auto-tune these.
	- Try matching their settings using GenApi features (packet size, frame rate,
inter-packet delay).

When in doubt:
- Capture logs with RUST_LOG=debug and compare behaviour at the same frame
rate and resolution.

See: [Streaming](./tutorials/streaming.md)￼and [Networking](./networking.md).

⸻

## “Does this work on Windows?”

Yes. Windows is a first-class target alongside Linux and macOS.

Notes:
- Make sure the firewall allows discovery and streaming:
	- When Windows asks whether to allow the executable on Private/Public
networks, allow it on the profile you use for the camera network.
- Configure the NIC for the camera network with a static IPv4 address,
separate from your office/internet NIC.
- For high-throughput setups:
	- Consider enabling jumbo frames on the camera NIC.
	- Disable power-saving features that can introduce latency.

See: [Networking](./networking.md) for NIC configuration details.

⸻

## “How do I set exposure, gain, pixel format, etc.?”

Use the GenApi features via viva-camctl or the viva-genicam crate.

Examples with viva-camctl:

```bash
# Read ExposureTime
cargo run -p viva-camctl -- \
  get --ip 192.168.0.10 --name ExposureTime

# Set ExposureTime to 5000 (units depend on camera, often microseconds)
cargo run -p viva-camctl -- \
  set --ip 192.168.0.10 --name ExposureTime --value 5000

# Set PixelFormat by name
cargo run -p viva-camctl -- \
  set --ip 192.168.0.10 --name PixelFormat --value Mono8
```

For more, see: [Registers & features](./tutorials/registers.md).

⸻

## “What are selectors and why do my changes seem to disappear?”

Many cameras use selectors to multiplex multiple logical settings onto one
feature. Example:
- GainSelector = All, Red, Green, Blue, …
- Gain = value for the currently selected channel.

If you set Gain without first setting GainSelector, you might be modifying
a different “row” than you expect.

Typical sequence:

```bash
cargo run -p viva-camctl -- \
  set --ip 192.168.0.10 --name GainSelector --value Red

cargo run -p viva-camctl -- \
  set --ip 192.168.0.10 --name Gain --value 5.0
```

See: [Registers & features](./tutorials/registers.md) and the `selectors_demo`
example in the viva-genicam crate.

⸻

## “Do I need to care about the GenApi XML?”

For most applications, no:
- You can use features by name and let viva-genapi handle the mapping.

You should look at the XML when:
- A feature behaves differently from the SFNC / vendor documentation.
- You are debugging selector or SwissKnife behaviour.
- You are contributing to viva-genapi or genapi-xml.

See: [GenApi XML tutorial](./tutorials/genapi-xml.md)￼and the crate chapters
for `viva-genapi-xml` and `viva-genapi` when they are filled in.

⸻

## “How do I save frames and look at them?”

With viva-camctl:
- Use stream with an option like --count / --output (exact flags depend
on the CLI):

```bash
cargo run -p viva-camctl -- \
  stream --ip 192.168.0.10 --iface 192.168.0.5 \
  --count 100 --output ./frames
```

This typically saves a sequence of frames in a simple format (e.g. raw, PGM/PPM)
that you can inspect with:
- Image viewers.
- Python + NumPy + OpenCV.
- Your own Rust tools.

See: [Streaming](./tutorials/streaming.md).

⸻

## “How do I generate documentation?”
- mdBook (this book):
	- From the repository root:
```bash
cargo install mdbook  # if not already installed
mdbook build book
```
	- The rendered HTML will be under book/book/.
- Rust API docs:
	- From the repository root:
```rust
cargo doc --workspace --all-features
```
	- The rendered HTML will be under target/doc/.

Many users publish these via GitHub Pages or another static host; see the
repository CI configuration for details.

⸻

## “Where should I report bugs or ask questions?”
- For bugs or feature requests, open an issue in the GitHub repository with:
    - A clear description of the problem.
    - Your OS, Rust version, and camera model.
    - A minimal reproduction if possible (CLI commands or small Rust snippet).
    - Relevant logs (e.g. RUST_LOG=debug output).
- For questions that may be general (not specific to this project), link to:
    - The camera’s data sheet or GenICam XML snippet if relevant.
    - Any vendor tools you used to compare behaviour.

Good issues make it much easier to improve the crates for everyone.

---

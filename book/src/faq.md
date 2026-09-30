# FAQ & Troubleshooting

Short answers to questions that come up often when using `viva-genicam` or
bringing up a new camera. Each one links to the chapter that covers it in
full.

---

## "Discovery finds no cameras. What do I check first?"

Run `viva-camctl list`. If nothing appears, check in this order: the link LEDs,
that the host NIC and the camera share a subnet, that the firewall lets the
discovery reply in, and — on a multi-NIC host — that you name the right
interface:

```bash
cargo run -p viva-camctl -- list --iface 192.168.0.5   # host NIC by IPv4 address
cargo run -p viva-camctl -- list --iface eth0          # or by OS name
```

The full checklist is in
[Discovery → Troubleshooting checklist](./tutorials/discovery.md#troubleshooting-checklist).
If the camera's address starts with `169.254.`, go to
[Networking → Link-local (APIPA) cameras](./networking.md#3-link-local-apipa-cameras)
instead. If none of it helps, [send us the report bundle](./reporting.md).

---

## "The vendor viewer works but viva-genicam doesn't. Why?"

Usually because the two tools differ in which host NIC they use, where the
camera is told to stream, or the packet size and delay they set. See
[Networking → Vendor tool works, viva-genicam does not](./networking.md#83-vendor-tool-works-viva-genicam-does-not)
for what to compare.

---

## "Does this work on Windows?"

Yes — Windows, Linux and macOS are all supported. On Windows the usual
obstacles are the firewall prompt and NIC power-saving settings; see
[Networking → Windows](./networking.md#22-windows).

---

## "How do I set exposure, gain, pixel format, etc.?"

By feature name, with `viva-camctl set` or `Camera::set` in Rust:

```bash
cargo run -p viva-camctl -- set --ip 192.168.0.10 --name ExposureTime --value 5000
```

[Registers & features](./tutorials/registers.md) covers reading, writing, why a
write can be refused, and the Rust side.

---

## "What are selectors and why do my changes seem to disappear?"

A selector such as `GainSelector` chooses which channel a feature like `Gain`
refers to. Setting `Gain` without first setting the selector changes whichever
channel is currently selected. See
[Registers & features → Work with selectors](./tutorials/registers.md#step-2--work-with-selectors).

---

## "Do I need to care about the GenApi XML?"

For most applications, no: you use features by name and the NodeMap handles
the mapping. Look at the XML when a feature behaves differently from its
documentation, or when you are debugging selectors or SwissKnife formulas. See
the [GenApi XML tutorial](./tutorials/genapi-xml.md) and the
[`viva-genapi` chapter](./crates/viva-genapi.md).

---

## "How do I save frames and look at them?"

`viva-camctl stream` saves the first `--save N` frames (default 1) to the
current directory as `frame_0001.pgm` (`Mono8`) or `frame_0001.ppm` (other
formats, or always with `--rgb`):

```bash
cargo run -p viva-camctl -- stream \
  --ip 192.168.0.10 --iface 192.168.0.5 --save 100 --duration-s 10
```

Without `--duration-s` the stream runs until Ctrl+C. Both formats are plain
NetPBM, readable by most image viewers, OpenCV and Pillow. Event-vision cameras
produce `block_NNNN.evt3` / `.evt21` files instead; see
[Streaming → Event-vision cameras](./tutorials/streaming.md#23-event-vision-cameras).

---

## "How do I generate documentation?"

From the repository root:

```bash
cargo install mdbook          # if not already installed
mdbook build book             # this book, into book/book/

cargo doc --workspace --all-features --no-deps   # Rust API docs, into target/doc/
```

---

## "Where should I report bugs or ask questions?"

If a camera does not work, start with
[Reporting a camera we can't open](./reporting.md) — it gives you two commands
that collect everything we need.

For anything else, open an issue on
[GitHub](https://github.com/VitalyVorobyev/viva-genicam/issues) with what you
ran and what happened, your OS and camera model, and logs from
`RUST_LOG=debug` or `viva-camctl -vv`.

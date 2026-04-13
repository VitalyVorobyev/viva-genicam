# Python examples

Runnable scripts demonstrating the `viva-genicam` Python package.

| Example | What it shows |
|---|---|
| [`discover.py`](discover.py) | Enumerate GigE + U3V cameras reachable from this host |
| [`get_set_feature.py`](get_set_feature.py) | Read/write features + introspection + typed helpers |
| [`node_browser.py`](node_browser.py) | Walk the NodeMap: categories, kinds, access modes, visibility |
| [`grab_frame.py`](grab_frame.py) | Stream frames, convert to NumPy, save as PNG (needs `Pillow`) |
| [`demo_fake_camera.py`](demo_fake_camera.py) | End-to-end demo using the in-process fake camera — no hardware needed |

## Running without a real camera

Build the fake GigE camera once, then run any example that discovers on loopback:

```bash
cargo build -p viva-fake-gige --release
# In one terminal:
./target/release/viva-fake-gige --bind 127.0.0.1 --port 3956
# In another:
python crates/viva-pygenicam/examples/discover.py --all
```

Or let `demo_fake_camera.py` spawn the binary for you:

```bash
python crates/viva-pygenicam/examples/demo_fake_camera.py
```

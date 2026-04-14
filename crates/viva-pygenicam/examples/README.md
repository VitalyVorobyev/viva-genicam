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

The wheel ships an in-process fake GigE camera. Either run the bundled demo end to end:

```bash
python crates/viva-pygenicam/examples/demo_fake_camera.py
```

…or stand up a fake yourself and point any other example at it:

```python
from viva_genicam.testing import FakeGigeCamera

with FakeGigeCamera(width=640, height=480, fps=10) as fake:
    # fake is now running on 127.0.0.1:3956
    # Run discover.py / get_set_feature.py / node_browser.py in another
    # terminal, or call the APIs directly here.
    ...
```

No `cargo build`, no subprocess, no repo clone.

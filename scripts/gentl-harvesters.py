"""Harvesters, a GenTL consumer we did not write, against viva_gentl.cti.

Harvesters drives the producer through EMVA's reference GenApi (the `genicam`
package): it builds a node map from each module's XML, served through the
producer's ports, and the camera's own node map through the remote port,
then acquires. This checks what our own tests cannot: that a reference
implementation reads our XMLs and our buffers the way we meant them.

Run with viva-fake-gige already serving Mono16 on loopback:

    target/release/viva-fake-gige --width 64 --height 48 --pixel-format mono16 &
    python scripts/gentl-harvesters.py target/gentl/viva_gentl.cti 10

or let scripts/gentl-harness.sh start the fake (HARVESTERS_PYTHON). The
fake's Mono16 image is the byte ramp (i * 7 + block id) mod 256, which this
script recomputes for every frame. Exit status 0 on success.

The book's "Use viva from other tools" page includes the anchored parts.
"""

import sys

import numpy
from harvesters.core import Harvester

FAKE_MODEL = "FakeGigE"


def check(condition, message):
    if not condition:
        raise SystemExit(f"FAIL: {message}")


def check_modules(acquirer):
    """The producer's module node maps and the camera's, through EMVA's GenApi."""
    system = acquirer.system.node_map
    check(system.TLVendorName.value == "viva-genicam", "TLVendorName")
    check(system.TLType.value == "GigEVision", "TLType")
    check(system.GenTLVersionMajor.value == 1, "GenTLVersionMajor")
    print(f"system: {system.TLModelName.value} {system.TLVersion.value} at {system.TLPath.value}")
    interface = acquirer.interface.node_map
    check(interface.InterfaceType.value == "GigEVision", "InterfaceType")
    print(f"interface: {interface.InterfaceID.value}")
    device = acquirer.device.node_map
    check(device.DeviceType.value == "GigEVision", "DeviceType")
    check(device.DeviceModelName.value == FAKE_MODEL, "DeviceModelName")
    check(device.DeviceAccessStatus.value in ("OpenReadWrite", "OpenReadOnly"), "DeviceAccessStatus")
    stream = acquirer.data_streams[0].node_map
    check(stream.StreamBufferHandlingMode.value == "OldestFirst", "StreamBufferHandlingMode")

    # The camera's own node map, read through the remote port.
    remote = acquirer.remote_device.node_map
    check(remote.PixelFormat.value == "Mono16", f"PixelFormat {remote.PixelFormat.value}")
    print(f"camera: {remote.Width.value}x{remote.Height.value} Mono16, PayloadSize {remote.PayloadSize.value}")


def check_frame(image, frame_id, last):
    """The fake's ramp, recomputed from the block id; returns the block id."""
    check(frame_id > last, f"frame {frame_id} after {last}")
    height, width = image.shape
    ramp = (numpy.arange(width * height * 2, dtype=numpy.uint32) * 7 + (frame_id & 0xFFFF)) & 0xFF
    expected = ramp.astype(numpy.uint8).view("<u2").reshape(height, width)
    check(numpy.array_equal(image, expected), f"frame {frame_id}: pixels differ from the ramp")
    print(f"frame {frame_id}: {width}x{height} Mono16, pixels match")
    return frame_id


# ANCHOR: open
def open_camera(cti, model):
    """Load the producer, list what it finds, and open a camera by model."""
    harvester = Harvester()
    harvester.add_file(cti)
    harvester.update()
    print([info.property_dict["model"] for info in harvester.device_info_list])
    return harvester, harvester.create({"model": model})
# ANCHOR_END: open


# ANCHOR: fetch
def fetch_image(acquirer):
    """The next frame, as a 2-D array, and its block ID."""
    with acquirer.fetch(timeout=3) as buffer:
        component = buffer.payload.components[0]
        image = component.data.reshape(component.height, component.width)
        # The buffer goes back to the producer when the block ends: copy
        # what you keep.
        return image.copy(), buffer.module.frame_id
# ANCHOR_END: fetch


def main() -> None:
    cti = sys.argv[1]
    frames = int(sys.argv[2]) if len(sys.argv) > 2 else 10

    harvester, acquirer = open_camera(cti, FAKE_MODEL)
    try:
        check_modules(acquirer)
        acquirer.start()
        last = 0
        for _ in range(frames):
            image, frame_id = fetch_image(acquirer)
            last = check_frame(image, frame_id, last)
        acquirer.stop()
    finally:
        acquirer.destroy()
        harvester.reset()
    print(f"OK: {frames} frames through {cti} with Harvesters")


if __name__ == "__main__":
    main()

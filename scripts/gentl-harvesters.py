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

Against a real camera, name its model as the producer lists it:

    python scripts/gentl-harvesters.py target/gentl/viva_gentl.cti 100 --camera Manta_G-125B

Its pixels are not known in advance, so each frame is checked for
completeness, for the camera's Width, Height and PixelFormat, and for the
bytes that geometry needs; nothing else is assumed of the camera.

The book's "Use viva from other tools" page includes the anchored parts.
"""

import sys

import numpy
from harvesters.core import Harvester

FAKE_MODEL = "FakeGigE"


def check(condition, message):
    if not condition:
        raise SystemExit(f"FAIL: {message}")


def check_modules(acquirer, model=FAKE_MODEL):
    """The producer's module node maps and the camera's, through EMVA's GenApi.

    The camera's own node map is the fake's unless `model` names another.
    """
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
    check(device.DeviceModelName.value == model, f"DeviceModelName {device.DeviceModelName.value}")
    check(device.DeviceAccessStatus.value in ("OpenReadWrite", "OpenReadOnly"), "DeviceAccessStatus")
    stream = acquirer.data_streams[0].node_map
    check(stream.StreamBufferHandlingMode.value == "OldestFirst", "StreamBufferHandlingMode")

    # The camera's own node map, read through the remote port.
    remote = acquirer.remote_device.node_map
    if model == FAKE_MODEL:
        check(remote.PixelFormat.value == "Mono16", f"PixelFormat {remote.PixelFormat.value}")
    print(
        f"camera: {remote.Width.value}x{remote.Height.value} {remote.PixelFormat.value}, "
        f"PayloadSize {remote.PayloadSize.value}"
    )


def check_frame(image, frame_id, last):
    """The fake's ramp, recomputed from the block id; returns the block id."""
    check(frame_id > last, f"frame {frame_id} after {last}")
    height, width = image.shape
    ramp = (numpy.arange(width * height * 2, dtype=numpy.uint32) * 7 + (frame_id & 0xFFFF)) & 0xFF
    expected = ramp.astype(numpy.uint8).view("<u2").reshape(height, width)
    check(numpy.array_equal(image, expected), f"frame {frame_id}: pixels differ from the ramp")
    print(f"frame {frame_id}: {width}x{height} Mono16, pixels match")
    return frame_id


def check_real_frame(acquirer, buffer, last):
    """A real camera's frame: complete, and the size and format it says.

    Returns the block ID. Nothing is assumed of the pixels.
    """
    remote = acquirer.remote_device.node_map
    width, height, pixel_format = remote.Width.value, remote.Height.value, remote.PixelFormat.value
    module = buffer.module
    frame_id = module.frame_id
    check(module.is_complete(), f"frame {frame_id} is incomplete")
    check(frame_id > last, f"frame {frame_id} after {last}")
    component = buffer.payload.components[0]
    check(component.data_format == pixel_format, f"frame {frame_id}: {component.data_format}, not {pixel_format}")
    check(
        (component.width, component.height) == (width, height),
        f"frame {frame_id}: {component.width}x{component.height}, not {width}x{height}",
    )
    # Use the camera's advertised payload, not PFNC's effective bit depth:
    # legacy 10Packed groups, padding and chunks need more than pixels' bits.
    expected = remote.PayloadSize.value
    check(
        module.size_filled == expected,
        f"frame {frame_id}: {module.size_filled} bytes, {width}x{height} {pixel_format} needs {expected}",
    )
    print(f"frame {frame_id}: {width}x{height} {pixel_format}, {module.size_filled} bytes, complete")
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
    args = sys.argv[1:]
    camera = None
    if "--camera" in args:
        at = args.index("--camera")
        if at + 1 >= len(args):
            raise SystemExit("usage: gentl-harvesters.py <cti> [frames] [--camera <model>]")
        camera = args[at + 1]
        del args[at : at + 2]
    if not args:
        raise SystemExit("usage: gentl-harvesters.py <cti> [frames] [--camera <model>]")
    cti = args[0]
    frames = int(args[1]) if len(args) > 1 else 10

    harvester, acquirer = open_camera(cti, camera or FAKE_MODEL)
    try:
        check_modules(acquirer, camera or FAKE_MODEL)
        acquirer.start()
        last = 0
        for _ in range(frames):
            if camera is None:
                image, frame_id = fetch_image(acquirer)
                last = check_frame(image, frame_id, last)
            else:
                with acquirer.fetch(timeout=3) as buffer:
                    last = check_real_frame(acquirer, buffer, last)
        acquirer.stop()
    finally:
        acquirer.destroy()
        harvester.reset()
    print(f"OK: {frames} frames through {cti} with Harvesters")


if __name__ == "__main__":
    main()

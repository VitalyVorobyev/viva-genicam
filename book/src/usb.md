# USB3 Vision setup

A USB3 Vision camera needs no network setup, but the operating system must let
`viva-genicam` claim the camera's USB interfaces. How that works depends on the
platform:

- **macOS**: no setup.
- **Windows**: install the WinUSB driver once per camera, as described below.
- **Linux**: not yet tried on a camera. If it fails for you, please
  [report it](./reporting.md).

`viva-camctl list-usb` lists a camera without claiming it. So a camera can
appear in the list, or in Viva Studio's device list, and still fail to open.

## Windows

Windows has no built-in driver for USB3 Vision cameras, and `viva-genicam`
talks to USB devices through Microsoft's generic **WinUSB** driver. Until
WinUSB is installed for the camera, opening it fails with:

```text
claim the control interface (0): could not determine driver for interface
```

If a vendor driver holds the camera instead, the end of the message reads
`incompatible driver is installed for this interface`. The fix is the same in
both cases.

To install WinUSB with [Zadig](https://zadig.akeo.ie/), a free tool:

1. Close every program that uses the camera, including Viva Studio and the
   vendor's own software.
2. Run Zadig and choose **Options → List All Devices**.
3. In the device list, select the camera's **Interface 0** entry.
4. Choose **WinUSB** as the target driver and click **Install Driver**. If the
   camera already has a driver, the button says **Replace Driver**.
5. Open the camera again.

On the camera we tested (a MATRIX VISION mvBlueFOX3), WinUSB on interface 0 was
enough for streaming too. If yours opens but streaming or
events fail with `claim the stream interface` or `claim the event interface`,
look for the camera's other interfaces in Zadig and install WinUSB on those
too.

WinUSB replaces the vendor's driver for that camera. To go back to the vendor's
SDK, open Device Manager, find the camera, and choose
**Update driver** or **Roll back driver**. Reinstalling the vendor's driver
package also works.

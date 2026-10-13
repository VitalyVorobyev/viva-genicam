# License

viva-genicam is licensed under the [MIT License](https://opensource.org/licenses/MIT);
the [LICENSE](https://github.com/VitalyVorobyev/viva-genicam/blob/main/LICENSE)
file in the repository root has the full text.

The Python package is MIT too. Up to 0.8 its wheel linked libusb
(LGPL-2.1-or-later) for USB3 Vision; since 0.9 USB goes through
[nusb](https://crates.io/crates/nusb), which is pure Rust, and no C USB library
is linked anywhere.

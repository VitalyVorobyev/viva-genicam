# License

viva-genicam is licensed under the [MIT License](https://opensource.org/licenses/MIT);
the [LICENSE](https://github.com/VitalyVorobyev/viva-genicam/blob/main/LICENSE)
file in the repository root has the full text.

The Python wheel additionally links [libusb](https://libusb.info)
(LGPL-2.1-or-later) statically, for USB3 Vision, so the Python package's
license is `MIT AND LGPL-2.1-or-later`.
[`THIRD-PARTY-NOTICES.md`](https://github.com/VitalyVorobyev/viva-genicam/blob/main/crates/viva-pygenicam/THIRD-PARTY-NOTICES.md),
shipped in the wheel and the sdist, gives the details, including how to rebuild
the native module against another libusb.

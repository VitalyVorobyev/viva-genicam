# License

viva-genicam is licensed under the [MIT License](https://opensource.org/licenses/MIT);
the [LICENSE](https://github.com/VitalyVorobyev/viva-genicam/blob/main/LICENSE)
file in the repository root has the full text.

MIT covers the project's code, including commercial use; it does not grant
third-party trademark rights or certify standards compliance. Dependencies keep
their own licenses and notices. GenICam is a trademark of EMVA; GigE Vision and
USB3 Vision are trademarks of A3 (Association for Advancing Automation). This
project is independent and claims no A3 registration or certification. The
[licensing and provenance review](https://github.com/VitalyVorobyev/viva-genicam/blob/main/docs/legal-review.md)
records unresolved standards-related distribution questions separately from MIT.

The Python package is MIT too. Up to 0.8 its wheel linked libusb
(LGPL-2.1-or-later) for USB3 Vision; since 0.9 USB goes through
[nusb](https://crates.io/crates/nusb), which is pure Rust, and no C USB library
is linked anywhere.

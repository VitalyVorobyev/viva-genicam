# viva-fake-u3v

In-process fake USB3 Vision camera for testing.

Provides a `FakeU3vTransport` that implements the `UsbTransfer` trait with an in-memory register map and GenCP handling. Used by integration tests to create fully functional `Camera` instances without USB hardware.

> **Disclaimer** -- Independent open-source Rust implementation of GenICam-related standards.
> Not affiliated with, endorsed by, or the reference implementation of EMVA GenICam.
> GenICam is a trademark of EMVA.

This crate is not published to crates.io. It is used as a dev-dependency by other workspace crates.

Part of the [viva-genicam](https://github.com/VitalyVorobyev/viva-genicam) workspace.

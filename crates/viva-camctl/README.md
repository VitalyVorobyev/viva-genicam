# viva-camctl

CLI tool for GenICam camera discovery, feature control, and streaming.

> **Disclaimer** -- Independent open-source Rust implementation of GenICam-related standards.
> Not affiliated with, endorsed by, or the reference implementation of EMVA GenICam.
> GenICam is a trademark of EMVA.

## Install

```bash
cargo install viva-camctl
```

It also ships with the Python package: `pip install viva-genicam` installs a
`viva-camctl` command, linked into the extension module, so no Rust toolchain
is needed. Each [GitHub release](https://github.com/VitalyVorobyev/viva-genicam/releases)
also attaches it, prebuilt, in the `viva-genicam-<version>-<platform>` archives.
From a checkout, `cargo run -p viva-camctl -- <command>` works too.

## Commands

Discovery, features (`features`, `describe`, `dump`, `get`, `set`,
`execute`), configuration files (`save`, `load`), raw registers (`read-reg`,
`write-reg`), action commands, streaming and benchmarks, events, chunks, IP
configuration, the XML and a diagnostic report — on GigE Vision cameras
(`--ip`, `--index`) and, where the transport allows, USB3 Vision ones
(`--usb`). The command reference, with examples, is in the book:
[The command line (viva-camctl)](https://vitalyvorobyev.github.io/viva-genicam/camctl.html).
`viva-camctl <command> --help` lists each command's flags.

```bash
viva-camctl list
viva-camctl features --ip 192.168.0.10
viva-camctl save --ip 192.168.0.10 --out camera.txt
viva-camctl report --ip 192.168.0.10 --out viva-report.txt
```

Part of the [viva-genicam](https://github.com/VitalyVorobyev/viva-genicam) workspace.

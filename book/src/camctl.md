# The command line (viva-camctl)

`viva-camctl` does from a terminal what a field engineer does with a vendor
tool: find a camera, read it, change it, save and restore its configuration,
reach a register the XML does not describe, and stream from it. It comes with
the Python package (`pip install viva-genicam`), with every
[GitHub release](https://github.com/VitalyVorobyev/viva-genicam/releases), and
from crates.io (`cargo install viva-camctl`).

`viva-camctl <command> --help` lists each command's flags.

## Choosing a camera

Commands that act on one camera take one of:

- `--ip <ADDRESS>` — a GigE Vision camera by address;
- `--index <N>` — the Nth GigE Vision camera `list` finds;
- `--usb <N>` — the Nth USB3 Vision camera `list-usb` finds.

`get`, `set`, `execute`, `features`, `describe`, `dump`, `save`, `load`,
`read-reg`, `write-reg`, `bench` and `report` take `--usb` too. `stream`,
`events`, `chunks`, `xml`, `action` and `set-ip` are GigE Vision only;
`stream-usb` streams from a USB3 Vision camera.

## Global options

These go **before** the command name:

- `-v` / `-vv` — log at `debug` / `trace` (`RUST_LOG` overrides);
- `--json` — machine-readable output, where the command has any;
- `--iface <IFACE>` — the host interface, by one of its IPv4 addresses
  (`192.168.0.5`) or its OS name (`eth0`, or a GUID on Windows). `list`,
  `xml`, `report`, `stream`, `events` and `bench` also take it after the
  command name.

## Commands

| Command | What it does |
|---------|--------------|
| `list` | Discover GigE Vision cameras; `--ip` asks one address directly |
| `list-usb` | Discover USB3 Vision cameras |
| `features` | The category tree: every feature with its type, access and value; `--filter` keeps the names that contain a word, `--all` shows invisible features too |
| `describe` | One feature in full: tooltip, description, unit, range and increment, enumeration entries, the selectors it has or is, and the nodes it depends on; a state it could not read is printed as such, and the command fails |
| `dump` | Every value the camera has to read now, in XML order |
| `get` / `set` | Read a feature, or write it from text |
| `execute` | Run a command feature (`UserSetLoad`) and wait until the camera reports it done |
| `save` / `load` | Save the camera's configuration to a file, and restore it ([how](tutorials/common-tasks.md#saving-and-restoring-a-configuration)) |
| `read-reg` / `write-reg` | Read device registers, or write one 32-bit register, past the XML |
| `action` | Send a GigE Vision action command and count the acknowledgements |
| `stream` | Stream from a GigE Vision camera; save frames, or raw event-vision blocks |
| `stream-usb` | Stream from a USB3 Vision camera |
| `bench` | Sustained streaming benchmark, with an optional JSON report |
| `events` | Configure and read GigE Vision events |
| `chunks` | Switch chunk mode and chunk selectors |
| `set-ip` | Give a GigE Vision camera an address by MAC (FORCEIP with `--force`, persistent otherwise) |
| `xml` | The camera's GenApi XML, unparsed — so it works on a camera the library cannot open |
| `report` | A diagnostic bundle for a bug report ([what to send](reporting.md)) |

## A field engineer's session

```bash
# What is this camera, and how is it set?
viva-camctl features --ip 192.168.0.10
viva-camctl features --ip 192.168.0.10 --filter exposure
viva-camctl describe --ip 192.168.0.10 --name ExposureTime
viva-camctl dump --ip 192.168.0.10 > before.txt

# Keep its configuration, change it, put it back.
viva-camctl save --ip 192.168.0.10 --out camera.txt
viva-camctl set  --ip 192.168.0.10 --name ExposureTime --value 5000
viva-camctl load --ip 192.168.0.10 --file camera.txt

# The same on a USB3 Vision camera.
viva-camctl save --usb 0 --out camera.txt
```

`load` prints each value it could not write, with the reason, and exits with
an error if there was one; it warns when the file was saved from another
camera model.

## Registers

`read-reg` and `write-reg` reach the device without its XML, so they work on
a camera whose XML we cannot read. Addresses and values are decimal or
`0x`-hexadecimal:

```bash
viva-camctl read-reg  --ip 192.168.0.10 --addr 0x0938            # GevHeartbeatTimeout
viva-camctl read-reg  --ip 192.168.0.10 --addr 0x0200 --len 512   # the first URL
viva-camctl write-reg --ip 192.168.0.10 --addr 0x0938 --value 3000
viva-camctl read-reg  --usb 0 --addr 0x0 --len 64
```

A 32-bit value is shown and written in the device's byte order: big-endian on
GigE Vision, little-endian on USB3 Vision. GigE Vision registers are whole
32-bit words at addresses that are multiples of 4; anything else is refused
before it is sent. `write-reg` on GigE Vision takes control privilege for the
write and gives it back.

## Action commands

```bash
viva-camctl action --device-key 0x42 --group-key 1 --group-mask 0xffffffff
viva-camctl action --destination 192.168.0.10 --device-key 0x42 --group-key 1 \
    --group-mask 1 --scheduled-time 1700000000000000000
```

Every camera whose `ActionDeviceKey`, `ActionGroupKey` and `ActionGroupMask`
match acts and acknowledges; `action` reports how many did. Broadcast is the
default; `--destination` sends to one address. A scheduled action carries
its time in the cameras' timestamp ticks.

## Streaming

```bash
viva-camctl stream --ip 192.168.0.10 --save 2                      # keep the camera's packet size
viva-camctl stream --ip 192.168.0.10 --auto --save 2               # size packets from the NIC MTU
viva-camctl stream --ip 192.168.0.10 --save 2 --save-format png    # 16-bit above 8 bits
viva-camctl bench  --ip 192.168.0.10 --duration-s 60 --json-out bench.json
viva-camctl bench  --usb 0 --duration-s 60
viva-camctl stream-usb --save 3
```

Packet size, receive port and resend are covered in the
[Networking Guide](networking.md).

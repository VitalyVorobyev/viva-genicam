# GenICam standards and the crates that implement them

Which EMVA standard each crate implements, and where the sources come from for
the one that is not freely available. What the standards are is in the user
guide's [primer](https://vitalyvorobyev.github.io/viva-genicam/primer.html);
how the crates layer, in [design.md](design.md).

| Standard | What it defines | Crate |
|---|---|---|
| **GenApi** | The camera's XML self-description: typed feature nodes, their registers and the ports those live on (device, chunk, event), formulas (`SwissKnife`, `Converter`), selectors and `pValue` delegation; `<Streamable>` features saved to a file and restored (ADR-0028); `<Cachable>` and `<PollingTime>`, what may be cached and for how long (ADR-0029) | `viva-genapi-xml` parses it; `viva-genapi` builds the node map, evaluates it and routes each register to its port |
| **GenCP** | Register-map access: `READMEM`/`WRITEMEM`, `PENDING_ACK`, events, request ids, status codes, and the ABRM at address 0 | `viva-gencp` (vocabulary); `viva-u3v` (framing) |
| **GVCP** | GigE Vision control: discovery, register and memory access, events, action commands; cameras listen on UDP 3956 | `viva-gencp` (opcodes, the acknowledgement header, the shared status codes); `viva-gige` (its own status codes, `viva_gige::consts`) |
| **GVSP** | GigE Vision streaming: image transfer in UDP packets, reassembled by the host, which may ask for lost ones again (`PACKETRESEND_CMD`, a GVCP command); chunk data travels in the payload, tagged from the end | `viva-gige` (packets, the chunk layout, the resend request); `viva-genicam` (the receive engine); known gaps in [#191](https://github.com/VitalyVorobyev/viva-genicam/issues/191) |
| **USB3 Vision** | GenCP over USB bulk endpoints, plus a streaming interface; discovery by USB enumeration | `viva-u3v` |
| **SFNC** | Standard feature names (`ExposureTime`, not `Exposure`) | `viva-sfnc` |
| **PFNC** | Pixel-format codes (`Mono8` is `0x01080001`) | `viva-pfnc`, the single authority for codes and layouts |
| **GenTL** (1.6) and **GenTL SFNC** (1.2) | The C interface a consumer such as Harvesters loads from a `.cti` file to drive any camera: System, Interface, Device, Data Stream and Buffer modules, each module's port and GenApi XML, the buffer queues, events. viva is the *producer* ([ADR-0031](adrs/adr0031-gentl-producer.md)); `GenTL.h` is not vendored (its licence) | `viva-gentl`, GigE Vision only |

## GenCP and GVCP

GenCP (the Generic Control Protocol, free to download from EMVA) defines how a
host reads and writes a camera's register map. A transport fills in the byte
order and the framing.

USB3 Vision carries GenCP itself, little-endian. GigE Vision's GVCP is a
protocol of its own that GenCP generalised: the same ideas, a shared core of
status codes, but its own opcodes (`READREG`/`WRITEREG` among them), headers
and big-endian fields. The two do not share bytes on the wire. `viva-gencp`
holds GenCP's command ids and flags, GVCP's opcodes and acknowledgement header,
and the status codes the two share (`StatusCode`: `0x0000`, `0x8001`–`0x8007`,
`0x800E`, `0x800F`, `0x8FFF`). Beyond those the tables part — at `0x800B` GVCP
says `NO_MSG` and GenCP `MSG_TIMEOUT` — so each transport decodes a status with
its own type over that core: `viva_gige::GigeStatus` (GVCP's packet-resend
range too, for acknowledgements and GVSP packets alike) and
`viva_u3v::U3vStatus` (USB3 Vision's `0xA0xx`)
([ADR-0020](adrs/adr0020-per-transport-status-codes.md)).

## USB3 Vision sources

USB3 Vision devices use bulk endpoints: one pair for GenCP control, another for
image data. Discovery is standard USB enumeration — interface class `0xEF`,
subclass `0x05`, with the protocol telling control, event and streaming
interfaces apart. Bootstrap registers (ABRM, SBRM, SIRM) describe the device
and configure its stream. The USB3 Vision standard is not freely available;
what this implementation follows, and where that comes from, is in
`crates/viva-u3v/src/bootstrap.rs`.

## Further reading

- [EMVA GenICam website](https://www.emva.org/standards-technology/genicam/) —
  the standard documents

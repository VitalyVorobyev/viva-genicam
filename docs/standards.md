# GenICam standards and the crates that implement them

Which EMVA standard each crate implements, and where the sources come from for
the one that is not freely available. What the standards are is in the user
guide's [primer](https://vitalyvorobyev.github.io/viva-genicam/primer.html);
how the crates layer, in [design.md](design.md).

| Standard | What it defines | Crate |
|---|---|---|
| **GenApi** | The camera's XML self-description: typed feature nodes, their registers and the ports those live on (device, chunk, event), formulas (`SwissKnife`, `Converter`), selectors and `pValue` delegation | `viva-genapi-xml` parses it; `viva-genapi` builds the node map, evaluates it and routes each register to its port |
| **GenCP** | Register-map access: `READMEM`/`WRITEMEM`, `PENDING_ACK`, events, request ids, status codes, and the ABRM at address 0 | `viva-gencp` (vocabulary); `viva-u3v` (framing) |
| **GVCP** | GigE Vision control: discovery, register and memory access, events, action commands; cameras listen on UDP 3956 | `viva-gencp` (opcodes, headers); `viva-gige` |
| **GVSP** | GigE Vision streaming: image transfer in UDP packets, reassembled by the host, which may ask for lost ones again (`PACKETRESEND_CMD`, a GVCP command); chunk data travels in the payload, tagged from the end | `viva-gige` (packets, the chunk layout, the resend request); `viva-genicam` (the receive engine); known gaps in [#191](https://github.com/VitalyVorobyev/viva-genicam/issues/191) |
| **USB3 Vision** | GenCP over USB bulk endpoints, plus a streaming interface; discovery by USB enumeration | `viva-u3v` |
| **SFNC** | Standard feature names (`ExposureTime`, not `Exposure`) | `viva-sfnc` |
| **PFNC** | Pixel-format codes (`Mono8` is `0x01080001`) | `viva-pfnc`, the single authority for codes and layouts |

## GenCP and GVCP

GenCP (the Generic Control Protocol, free to download from EMVA) defines how a
host reads and writes a camera's register map. A transport fills in the byte
order and the framing.

USB3 Vision carries GenCP itself, little-endian. GigE Vision's GVCP is a
protocol of its own that GenCP generalised: the same ideas, a shared core of
status codes, but its own opcodes (`READREG`/`WRITEREG` among them), headers
and big-endian fields. The two do not share bytes on the wire. `viva-gencp`
holds GenCP's command ids and flags, GVCP's opcodes and headers, and the status
codes the two share.

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

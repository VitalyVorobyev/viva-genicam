# Reporting a camera we can't open

If `viva-genicam` cannot discover, open, or read your camera, that is worth
reporting even if you have a workaround. This page explains what to send and
why it matters more than it might look.

## Why your camera is the evidence

This project is a clean-room implementation of the GenICam standards. We can
test it against a specification, against a fake camera, and against a corpus of
real vendor XML documents — and we do. None of that is the same as a device.

Real cameras are routinely non-conformant, internally inconsistent, or at odds
with their own documentation, and **the goal is to work with the hardware that
exists, not the hardware the standard describes**. When a camera and the spec
disagree, we accommodate the camera. So a device we have never seen is the one
thing that can settle a question no amount of reading will.

Two of the worst bugs this project has shipped — a camera that could not be
opened at all in [#35](https://github.com/VitalyVorobyev/viva-genicam/issues/35),
and another in [#45](https://github.com/VitalyVorobyev/viva-genicam/issues/45) —
were each a single vendor XML construct we had never encountered. Both reached
users before they reached us. Both were fixed from an XML document a reporter
attached to the issue, and both documents are now in the test corpus, so that
class of bug has something watching for it.

## What to send

Two commands. Neither needs the camera to open: `xml` never parses the
document, and `report` records a parse or nodemap failure in the bundle instead
of stopping. So both work on a camera we cannot open — which is the only camera
anyone reports.

```bash
# Everything: environment, interfaces, discovery, bootstrap registers, XML
viva-camctl report --ip <CAMERA-IP> --out viva-report.txt

# Just the GenApi XML
viva-camctl xml --ip <CAMERA-IP> --out camera.xml
```

If discovery itself is what fails, drop `--ip` — the report still records your
interfaces and what discovery did or did not hear:

```bash
viva-camctl report --out viva-report.txt
```

The `.txt` extension is not cosmetic: GitHub rejects `.xml` attachments, so the
bundle is written as text you can drag onto an issue. Zip the raw XML if you
send it separately.

Both commands ship with the Python package too, so there is nothing to build
from source:

```bash
pip install viva-genicam
viva-camctl report --ip <CAMERA-IP> --out viva-report.txt
```

## What is in the bundle

Sections, in order:

| Section | Contents |
|---|---|
| Environment | `viva-camctl` version, host OS and architecture |
| Network interfaces | Every interface **as the library sees it**, with index and IPv4 addresses |
| Discovery | Each camera that answered: IP, MAC, manufacturer, model, version, serial, user name |
| Camera | Which device the rest of the report is about |
| Bootstrap registers | The GVCP standard register block, decoded |
| GenApi | XML size, schema version, node and feature counts, and any node that was dropped |
| GenApi XML | The document itself |

The interface list is there because an interface missing from it is invisible
to discovery no matter what the OS reports elsewhere, and that is not obvious
from any other output.

A section that fails says so and the report continues — a camera that refuses
the control channel still produces everything up to that point.

### Before you attach it

The bundle describes your machine: interface names, every IPv4 address on the
host, and the camera's MAC and serial. None of it is secret, but if any of it
is sensitive in your environment, edit the file before attaching — it is plain
text, and we would rather have a redacted report than none. Say what you
redacted so we do not read a blank as a missing value.

Use `--no-xml` to leave the GenApi document out, or `--stdout` to look at the
bundle before writing it anywhere.

## Where to send it

Open an issue at
[github.com/VitalyVorobyev/viva-genicam/issues](https://github.com/VitalyVorobyev/viva-genicam/issues).
Useful alongside the bundle:

- What you ran and what happened, quoted rather than paraphrased.
- Output with `RUST_LOG=debug`, or `viva-camctl -vv`.
- Whether a vendor tool works on the same camera and host — that separates a
  library bug from a network or camera configuration problem.
- A packet capture, if you can take one. `tcpdump -i <iface> -w capture.pcap
  udp port 3956` covers control traffic. A capture shows exactly what the
  camera sent, which can answer wire-level questions that documentation alone
  cannot.

## What happens to it

The XML goes into the vendor corpus — a set of real device descriptions that a
scheduled job parses *and evaluates*, building a full nodemap from each and
exercising every node. Adding yours means the construct that broke your camera
is watched from then on.

The corpus job runs weekly and on demand rather than on every pull request:
the documents are fetched from third-party repositories, and an upstream rename
or a network hiccup must not be able to block an unrelated merge. So a fix will
not appear on the day you report it, but a regression will not go unnoticed
either.

The documents are vendor copyright, published for interoperability by
third-party projects, so the repository fetches them rather than redistributing
them; the fetch script is `scripts/fetch-xml-corpus.sh`. Contributed documents
are fetched from the issue you attach them to — which is why those issue
threads are kept rather than closed quickly.

You can run the same check against your own camera without waiting for us.
Point `VIVA_GENICAM_XML_CORPUS` at a directory holding your XML:

```bash
mkdir -p ~/mycorpus
viva-camctl xml --ip <CAMERA-IP> --out ~/mycorpus/mycam.xml

git clone https://github.com/VitalyVorobyev/viva-genicam && cd viva-genicam
VIVA_GENICAM_XML_CORPUS=~/mycorpus \
  cargo test -p viva-genapi-xml --test vendor_corpus -- --nocapture   # parses
VIVA_GENICAM_XML_CORPUS=~/mycorpus \
  cargo test -p viva-genapi     --test vendor_corpus -- --nocapture   # + evaluates
```

The second stage is the one that matters. Parsing a document only proves the
XML is well-formed; building a nodemap from it and evaluating every node is
what exercises the formula language, the address model and the numeric codecs —
where the defects behind #35 all lived, invisible to the parser.

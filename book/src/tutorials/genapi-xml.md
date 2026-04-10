# GenApi XML

Goal of this tutorial:

- Understand **what** the GenICam XML is and **where** it lives.
- See how `viva-genapi-xml`:
  - Fetches the XML from the device (via the **FirstURL** register).
  - Parses it into a lightweight internal representation.
- Learn how to call this from Rust using a simple **memory reader closure**.
- Know when you actually need to look at the XML (and when you don’t).

You should already have:

- Completed [Discovery](./discovery.md).
- Completed [Registers & features](./registers.md), or at least be comfortable
  with the idea of **features** (ExposureTime, Gain, etc.) backed by registers.

---

## 1. What is the GenICam XML?

Every GenICam-compliant device exposes a **self-description XML file**:

- It lists all **features** the device supports (name, type, access mode, range).
- It defines how those features map to **device registers**.
- It encodes **categories**, **selectors**, and **SwissKnife** expressions.
- It declares which version of the **GenApi schema** the file uses.

This XML is normally stored in the device’s non-volatile memory. On the control
path, the host:

1. Reads the **FirstURL** register at a well-known address (0x0000).
2. Interprets it as a URL that tells where the XML actually lives:
   - Often a “local” memory address + size.
   - In theory, it could be `http://…` or `file://…` as well.
3. Reads the XML bytes from that location.
4. Hands the XML string to a GenApi implementation (here: `viva-genapi`).

The `viva-genapi-xml` crate encapsulates steps 1–3:

- Discover **where** to read XML from.
- Read it over the existing memory read primitive.
- Parse it into a simple, Rust-friendly model that the rest of the stack uses.

---

## 2. Overview of `viva-genapi-xml`

At a high level, `viva-genapi-xml` provides three building blocks:

- A function that **fetches** the XML from the device using a memory reader:

```rust
// Rough shape / pseudocode
pub async fn fetch_and_load_xml<F, Fut>(
    read_mem: F,
) -> Result<String, XmlError>
```
where
```rust
F: FnMut(u64, usize) -> Fut,
Fut: Future<Output = Result<Vec<u8>, XmlError>>;
```
- A function that parses XML into minimal metadata (schema version,
top-level features) without understanding every node type:
```rust
pub fn parse_into_minimal_nodes(xml: &str) -> Result<MinimalXmlInfo, XmlError>;
```
- A function that parses XML into a full XmlModel consisting of a flat
list of node declarations (Integer, Float, Enum, Boolean, Command,
Category, SwissKnife, …), including addressing and selector metadata.

You normally will not call these directly in application code (the genicam
crate does this for you), but they are useful when:
- Debugging why a particular feature behaves a certain way.
- Inspecting how a vendor encoded selectors or SwissKnife expressions.
- Adding support for new node types or schema variations in viva-genapi.

⸻

## 3. Fetching XML from a device in Rust

This section shows how you could call genapi-xml directly. The exact types
in your code will differ depending on whether you start from genicam or
viva-gige, but the pattern is always the same:
1.	Open a device.
2.	Provide a read_mem(addr, len) async function/closure.
3.	Call fetch_and_load_xml(read_mem).

### 3.1. Memory reader closure concept

fetch_and_load_xml does not know about GVCP, sockets, or cameras. It only
knows how to call a function with this shape:
```rust
async fn read_mem(address: u64, length: usize) -> Result<Vec<u8>, XmlError>;
```

Internally it will:
- Read up to a small buffer (e.g. 512 bytes) at address 0x0000.
- Interpret that buffer as a C string containing the FirstURL.
- Parse the URL and decide where to read the XML from.
- Read that region into memory and return it as a String.

Your job is to plug in a closure that uses whatever transport you have:
- A genicam device method (e.g. device.read_memory(address, length)).
- A low-level viva-gige control primitive.

### 3.2. Example: fetch XML using a genicam-style device

Below is illustrative pseudocode. Use it as a template and adapt to the
actual types in your project.

```rust
use viva_genapi_xml::{fetch_and_load_xml, XmlError};
use std::future::Future;

async fn fetch_xml_for_device() -> Result<String, XmlError> {
    // 1. Open your device using the higher-level API.
    //    Exact API varies; adjust to your real `viva-genicam` / `viva-gige` types.
    let mut ctx = genicam::Context::new().map_err(|e| XmlError::Transport(e.to_string()))?;
    let mut dev = ctx
        .open_by_ip("192.168.0.10".parse().unwrap())
        .map_err(|e| XmlError::Transport(e.to_string()))?;

    // 2. Define a memory reader closure.
    //    It must accept (address, length) and return bytes.
    let mut read_mem = move |addr: u64, len: usize| {
        async {
            // Replace `read_memory` with the actual method you have.
            let bytes = dev
                .read_memory(addr, len)
                .await
                .map_err(|e| XmlError::Transport(e.to_string()))?;
            Ok(bytes)
        }
    };

    // 3. Ask `viva-genapi-xml` to follow FirstURL and return the XML document.
    let xml = fetch_and_load_xml(&mut read_mem).await?;
    Ok(xml)
}
```

Key points:
- The closure is async and can perform chunked transfers internally.
- XmlError::Transport is used to wrap any transport-level errors.
- HTTP / file URLs are currently treated as Unsupported in XmlError; the
typical GigE Vision case uses a local memory address.

⸻

## 4. Inspecting minimal XML metadata

Once you have the XML string, you can parse just enough to answer questions
like:
- “Which GenApi schema version does this camera use?”
- “What are the top-level categories / features?”
- “Does this XML look obviously broken?”

`viva-genapi-xml` exposes a lightweight parse function for that:

```rust
use viva_genapi_xml::{parse_into_minimal_nodes, XmlError};

fn inspect_xml(xml: &str) -> Result<(), XmlError> {
    let info = parse_into_minimal_nodes(xml)?;

    if let Some(schema) = &info.schema_version {
        println!("GenApi schema version: {schema}");
    } else {
        println!("GenApi schema version: (not found)");
    }

    println!("Top-level features / categories:");
    for name in &info.top_level_features {
        println!("  - {name}");
    }

    Ok(())
}
```

This is intentionally lossy: it does not understand every node type. Its
job is to be:
- Fast enough for quick sanity checks.
- Robust to schema extensions that are not yet implemented.

Use this when you just need to confirm that:
- The XML is parseable at all.
- It roughly matches expectations for your camera family.

⸻

## 5. From XML to a full NodeMap

The next step (handled elsewhere in the stack) is:
1.	Parse XML into an XmlModel: a flat list of NodeDecl entries that
carry:
    - Feature name and type (Integer/Float/Enum/Bool/Command/Category/SwissKnife).
    - Addressing information (fixed / selector-based / indirect).
    - Access mode (RO/WO/RW).
    - Bitfield and byte-order information.
    - Selector relationships and SwissKnife expressions.
2.	Feed this XmlModel into viva-genapi, which:
	- Instantiates a NodeMap.
	- Resolves feature dependencies, selectors, and expressions at runtime.
	- Exposes typed getters/setters like get_float("ExposureTime").

You do not need to perform this plumbing manually in a typical application:
- The genicam crate will fetch and parse XML as part of its device setup.
- The viva-camctl CLI uses that same pipeline when you call get / set on
features.

If you want the gory details, see:
	- GenApi XML loader: genapi-xml￼
	- GenApi core & NodeMap: viva-genapi￼

(these chapters go into internal structures and how to extend them).

⸻

## 6. When should you look at the XML?

Most of the time, you can treat the XML as an implementation detail and just:
- Use viva-camctl for manual experimentation.
- Use genicam’s NodeMap accessors from Rust.

You should crack open the XML when:
- A feature behaves differently from the SFNC documentation.
- Selectors are not doing what you expect.
- You hit a SwissKnife or bitfield corner case.
- You are adding support for a new vendor-specific wrinkle to viva-genapi.

Typical workflow:
1.	Use your transport or genicam helper to dump the XML to a file.
2.	Run parse_into_minimal_nodes to quickly confirm schema and top-level
layout.
3.	Run the “full” XML → XmlModel path (via the crate internals) when working
on viva-genapi changes.
4.	Use a normal XML editor / viewer when manually exploring categories and
features.

⸻

## 7. Recap

After this tutorial you should:
- Know what the GenICam XML is and how it relates to features and registers.
- Understand how genapi-xml uses FirstURL and a memory reader closure to
retrieve the XML document from the device.
- Be able to write a small Rust helper that:
- Fetches the XML with fetch_and_load_xml.
- Inspects basic metadata with parse_into_minimal_nodes.
- Know when it is worth digging into XML versus staying at the feature level.

Next up: [Streaming](./streaming.md) — actually getting image data out of the
camera, now that you know how its configuration is described.

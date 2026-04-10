# viva-genapi

`viva-genapi` builds and evaluates the **NodeMap** from the device’s GenApi XML. It supports common node kinds and **SwissKnife** expressions.

## Responsibilities
- Parse the `viva-genapi-xml` intermediate representation into in‑memory nodes.
- Resolve node references and dependencies (incl. Selectors).
- Provide `get_*`/`set_*` operations that either access registers or evaluate expressions.

## Node kinds (current)
- **Integer / Float / Boolean / Enumeration / Command / String / Register**
- **SwissKnife** — expression node referencing other nodes (read‑only). Typical syntax supports arithmetic, comparisons, logical ops, and ternary‑like conditionals.

## Selectors
When a feature has selectors (e.g., `GainSelector`), evaluation temporarily switches the active selector context so reads/writes map to the correct addresses.

## Read/write examples (via façade)
Below is a minimal flow using the `viva-genicam` façade.

```rust
use genicam::Client; // façade crate

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1) Connect over GigE Vision to a known IP
    let mut cam = Client::connect("192.168.0.10").await?;

    // 2) Read a numeric feature (backed by register or computed by SwissKnife)
    let exposure: f64 = cam.get_f64("ExposureTime")?; // microseconds
    println!("ExposureTime = {} µs", exposure);

    // 3) Set a feature (register-backed)
    cam.set_f64("ExposureTime", 5000.0)?; // 5 ms

    // 4) Read a computed feature (SwissKnife)
    let derived: f64 = cam.get_f64("LinePeriodUs")?; // example name
    println!("LinePeriodUs (computed) = {}", derived);

    Ok(())
}
```

> Any SwissKnife node is evaluated transparently when you call `get_*`. If it references other nodes, those nodes are read or computed first.

## Caching & invalidation (overview)

* Values are cached within an evaluation pass; writes invalidate dependent caches.
* Selector changes create a new evaluation context so the correct addresses/branches are used.

## Errors you may see

* **OutOfRange**: requested value outside `[Min, Max, Inc]`.
* **AccessDenied**: node not writable in the current state.
* **DependencyMissing**: referenced node not found/visible.
* **Transport**: I/O error while reading/writing registers.

## Tips for contributors

* Implement new node kinds behind a common evaluation trait.
* Keep pure computation separate from transport; call into the transport via a narrow interface.
* Add unit tests with synthetic NodeMaps (no device required) plus integration tests that hit real hardware when available.

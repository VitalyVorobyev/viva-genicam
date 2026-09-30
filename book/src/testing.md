# Running the tests

Everything below runs without hardware or external tools: the integration
tests start in-process fake cameras (`viva-fake-gige`, `viva-fake-u3v`) on
loopback. What the fakes can do, and how to use them in your own code or
interactively, is in [Testing without hardware](tutorials/fake-camera.md).

## The whole suite

```bash
cargo test --workspace
```

Unit tests live next to the code in `mod tests { }` blocks; the integration
suites below run as part of this command too.

## Individual suites

```bash
# GigE integration: discovery, features, streaming against the fake camera
cargo test -p viva-genicam --test fake_camera

# GenApi access predicates (pIsLocked, pIsAvailable, ...) end to end
cargo test -p viva-genicam --test predicates

# Control-channel keepalive
cargo test -p viva-genicam --test heartbeat

# Which GVCP command leaves the host for a register access
cargo test -p viva-genicam --test register_access

# USB3 Vision, against viva-fake-u3v
cargo test -p viva-genicam --test fake_u3v_camera --features u3v

# viva-service end to end over Zenoh
cargo test -p viva-service --test fake_camera_e2e

# Every code include in this book resolves to a real example anchor
cargo test -p viva-genicam --test book_includes
```

Every fake-camera suite binds UDP port 3956, so two test processes running at
the same time (for example in two checkouts) interfere with each other and fail
spuriously. Run them one at a time.

## Vendor XML corpus

Two further tests parse and evaluate real vendor GenApi documents. The
documents are fetched rather than committed, and the tests do nothing when the
corpus directory is absent:

```bash
scripts/fetch-xml-corpus.sh
cargo test -p viva-genapi-xml --test vendor_corpus -- --nocapture   # parses
cargo test -p viva-genapi     --test vendor_corpus -- --nocapture   # + evaluates
```

To run them against XML from your own camera, see
[Reporting a camera we can't open → What happens to it](reporting.md#what-happens-to-it).

## Logging

```bash
RUST_LOG=debug cargo test --workspace -- --nocapture
```

# Vendored crates

Patched copies of crates.io dependencies, wired in through `[patch.crates-io]`
in the root `Cargo.toml`. The first commit touching each directory is the
pristine upstream release; later commits are our edits (`git log -- vendor/<crate>`),
and every edit is marked `// TRON PATCH:`.

| Crate | Upstream | Why | Drop when |
|-------|----------|-----|-----------|
| `xwt-web` | 0.15.0 | Firefox's `datagrams.readable` is not a byte stream, so BYOB `getReader` throws and the browser connection never completes. Falls back to a default reader. | The Bevy bump brings xwt-web >= 0.22 (has the same fallback). |
| `lightyear_transport` | 0.28.0 | `MAX_PACKET_SIZE` is a hard-coded 1200; Firefox reports `maxDatagramSize` 1024 and silently drops larger datagrams. Lowered to 1024. Changes the wire format: bump `PROTOCOL_VERSION`. | Lightyear makes the packet size configurable. |

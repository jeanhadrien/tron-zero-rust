# Production Build

Shipping builds. Dev builds live in `docs/dev.md`.

## Server — Linux (prod target)

The server deploys on Linux. Two ways to produce a Linux binary from a Windows dev machine:

### Option A — build in WSL (recommended)

WSL already has `rustup` + cargo 1.96.1. The Linux C toolchain is missing by default, so install it once:

```
sudo apt update && sudo apt install -y build-essential
```

Then build the server targeting Linux, from WSL:

```
cd /mnt/c/dev/tron-zero-rust
cargo build --release -p tron-zero-server
```

Output: `target/x86_64-unknown-linux-gnu/release/tron-zero-server` (or `target/release/...` if the host triple already matches). Copy to the server.

Caveat: building from `/mnt/c/...` reads source over the 9P bridge. For a one-shot release build that's fine; for repeated prod builds, copy the repo into the WSL filesystem first (`cp -r /mnt/c/dev/tron-zero-rust ~/tron-zero-rust && cd ~/tron-zero-rust`).

### Option B — `cross` (hermetic, Docker)

No WSL toolchain touches needed; runs the build in a container:

```
cargo install cross --git https://github.com/cross-rs/cross
cross build --release -p tron-zero-server --target x86_64-unknown-linux-gnu
```

Requires Docker Desktop with WSL2 backend. Slower than Option A per build, but reproducible across machines.

## Client — Windows (native)

```
cargo build --release -p tron-zero-client
```

Output: `target/release/tron-zero-client.exe`. Distribute the `.exe` alongside any required runtime DLLs (none expected for a static release build; verify with a clean-machine test).

## Client — WASM (web, not yet wired)

Per PLAN.md the client also targets `wasm32-unknown-unknown`. Bootstrap does not set this up. To enable:

```
rustup target add wasm32-unknown-unknown
cargo build -p tron-zero-client --target wasm32-unknown-unknown
```

Expect bevy/wgpu WASM feature-gating work beyond adding the target — follow up before relying on this path.

## Release notes

- `rust-toolchain.toml` pins `stable` + `clippy`/`rustfmt` for every contributor; no per-machine drift.
- `Cargo.lock` is committed (workspace with binaries) — reproducible builds.
- Release profile uses Cargo defaults (`opt-level = 3`). Tune in `[profile.release]` only if profiling demands it.

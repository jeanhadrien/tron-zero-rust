# Production Build

Status: updated 2026-11-02. Shipping builds and runtime configuration. Dev
builds live in `docs/dev.md`; releases and hosting for friends in `docs/release.md`.

## Ports and configuration

The game server listens on **UDP** (raw, unauthenticated, unencrypted for now;
see `NETWORK.md`) bound to `0.0.0.0`. The lobby manager speaks HTTP.

Server (`tron-zero-server`, all optional):

| Variable | Default | Effect |
|----------|---------|--------|
| `PORT` | `5000` | UDP port to bind (also the default advertised port). |
| `MAX_PLAYERS` | `10` | Human capacity; the 11th player is rejected `ServerFull`. Bots do not count. |
| `SERVER_NAME` | `Unnamed Server` | Name shown in the server browser. |
| `MANAGER_URL` | `http://localhost:3001` | Lobby to register with. |
| `ADVERTISED_HOST` | `127.0.0.1` | Public host clients should use. A loopback value with a non-local `MANAGER_URL` is refused (no announce, game still runs). |
| `ADVERTISED_PORT` | `PORT` | Public port, if NAT maps it differently. |
| `WEB_PORT` | `5001` | UDP port of the WebTransport listener for browser players (QUIC; open it in the firewall next to `PORT`). `0` disables it. |
| `ADVERTISED_WEB_PORT` | `WEB_PORT` | Public WebTransport port, if NAT maps it differently. Announced as `webPort`. |
| `WEB_CERT` / `WEB_KEY` | unset | PEM certificate chain and key (e.g. Let's Encrypt) for the WebTransport listener. Unset: a self-signed 14-day ECDSA certificate is generated and its SHA-256 announced as `certHash`. |
| `WEB_MAX_UPTIME_HOURS` | `312` | Self-signed only: exit cleanly (after unregistering) once this long has passed, so the container restart policy (`always`/`unless-stopped`, exit code 0) restarts it with a fresh certificate. |
| `ADVERTISED_SECURE` | unset | `true` marks the room `secure` (reserved for WebTransport). |
| `ALLOW_LOOPBACK_ANNOUNCE` | unset | `1`/`true` (dev only) turns the loopback refusal into a warning. |
| `TRON_BOT_DEBUG` | unset | Truthy: replicate bot decision snapshots for the client debug overlay (extra bandwidth; not for public servers). |
| `RUST_LOG` | `info` | Standard Bevy/tracing filter. |

Client (`tron-zero-client`): `MANAGER_URL` (runtime) / `TRONZERO_MANAGER_URL`
(build time; the release workflow bakes in the production lobby),
`TRON_CONFIG_FILE`, `TRON_AUTOCONNECT=host:port`, `TRON_SIM_LATENCY_MS`,
`TRON_SIM_JITTER_MS`, `TRON_SIM_LOSS_PCT`, `TRON_BOT_DEBUG`. Semantics are in
`docs/dev.md`. Settings live in `client.json` under the platform config dir.

Manager (`server-manager/`): `PORT` (3001; `MANAGER_PORT` fallback), `HOST`,
`LOG_LEVEL`, `TRUST_PROXY`, `REGISTER_LIMIT_PER_MIN`; see its README. The
Docker image has never been built here (`NETWORK.md`).

## Server — Linux (prod target)

The server deploys on Linux. Two ways to produce a Linux binary from a Windows dev machine:

### Option A — build in WSL (recommended)

WSL needs `rustup` and the Linux C toolchain, installed once:

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

The server browser's room fetch already works on web: it uses `ehttp`,
which performs a plain browser fetch on wasm. Room listing therefore works
in the browser, but every Connect button is disabled there — play needs the
native UDP transport until the WebTransport migration in PLAN.md lands.

## Release notes

- `rust-toolchain.toml` pins `stable` + `clippy`/`rustfmt` for every contributor; no per-machine drift.
- `Cargo.lock` is committed (workspace with binaries) — reproducible builds.
- Release profile uses Cargo defaults (`opt-level = 3`). Tune in `[profile.release]` only if profiling demands it.

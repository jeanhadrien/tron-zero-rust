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
It runs on Cloud Run at `https://server-manager.tronzero.dev`.

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

### Option C — container image (GCE VM)

`Dockerfile.server` (context = repo root) builds a slim Debian image: cargo-chef
dependency caching, non-root user, `ca-certificates` for the HTTPS announcer,
`EXPOSE 5000/udp 5001/udp`. The server is PID 1 and exits on SIGTERM (`ctrlc`).

```
docker build -f Dockerfile.server -t tron-zero-server .
docker run --rm -p 5000:5000/udp -e MANAGER_URL=... -e ADVERTISED_HOST=... tron-zero-server
```

`cloudbuild.server.yaml` builds and pushes it to Artifact Registry
(`gcloud builds submit --config cloudbuild.server.yaml --substitutions=_TAG=v1 .`).
The server crate depends on Bevy with default features off (`bevy_log`,
`bevy_state` only): no winit/alsa/udev in the image.

## Client — Windows (native)

```
cargo build --release -p tron-zero-client
```

Output: `target/release/tron-zero-client.exe`. Distribute the `.exe` alongside any required runtime DLLs (none expected for a static release build; verify with a clean-machine test).

## Client — WASM (web, spike)

The client compiles and starts in the browser (menu, settings, server browser
list; WebGL2). Connecting is not wired yet: the browser transport is
Lightyear's WebTransport client (`WebTransportClientIo`), which needs the
server-side WebTransport listener and its certificate digest
(`?TRON_CERT_DIGEST=` query parameter).

```
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version <wasm-bindgen version in Cargo.lock> --locked
npm i -g binaryen            # wasm-opt
./web/build.ps1              # -> web/dist (serve it with any static server)
```

`[profile.wasm-release]` is fat LTO (about 25 min, ~12 GB RAM, plus ~10 min of
wasm-opt); for iteration use `-SkipOpt` or a plain
`cargo build --target wasm32-unknown-unknown`. `--cfg=web_sys_unstable_apis`
and the getrandom `wasm_js` cfg live in `.cargo/config.toml`. Browsers have no
environment variables: `platform::env_var` reads URL query parameters of the
same name (`?MANAGER_URL=...`), preferences use `localStorage`, and the Quit
button is hidden.

## Release notes

- `rust-toolchain.toml` pins `stable` + `clippy`/`rustfmt` for every contributor; no per-machine drift.
- `Cargo.lock` is committed (workspace with binaries) — reproducible builds.
- Release profile uses Cargo defaults (`opt-level = 3`). Tune in `[profile.release]` only if profiling demands it.

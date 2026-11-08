# Production Build

Status: updated 2026-11-08. Shipping builds and runtime configuration. Dev
builds live in `docs/guides/dev.md`; releases and hosting for friends in `docs/guides/release.md`.

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
| `RANKED_KEY` | none | `<keyId>.<secret>` from the manager's `add-key.ts`: registers the room as ranked (join tickets required, results move ratings). For community servers. |
| `RANKED_IDENTITY` | none | `gcp`: register as ranked with the VM service account's ID token from the GCE metadata server instead of a key. The manager must trust the account and set `IDENTITY_AUDIENCE` equal to `MANAGER_URL` (see `deploy/gce/README.md`). Wins over `RANKED_KEY`. |
| `ADVERTISED_HOST` | `127.0.0.1` | Public host clients should use. A loopback value with a non-local `MANAGER_URL` is refused (no announce, game still runs). |
| `ADVERTISED_PORT` | `PORT` | Public port, if NAT maps it differently. |
| `WEB_PORT` | `5001` | UDP port of the WebTransport listener for browser players (QUIC; open it in the firewall next to `PORT`). `0` disables it. Equal to `PORT` is refused at startup. The room is announced with `webPort`/`certHash` only once the listener is open. |
| `ADVERTISED_WEB_PORT` | `WEB_PORT` | Public WebTransport port, if NAT maps it differently. Announced as `webPort`. |
| `WEB_CERT` / `WEB_KEY` | unset | PEM certificate chain and key (e.g. Let's Encrypt) for the WebTransport listener. Unset: a self-signed 14-day ECDSA certificate is generated and its SHA-256 announced as `certHash`. |
| `WEB_MAX_UPTIME_HOURS` | `288` | Self-signed only (certificates die after 14 days). Past this many hours the server exits cleanly (after unregistering) at the first moment no human is connected; at the hard limit (+24 h, at most 332 h) it exits regardless. A supervisor must restart it with a fresh certificate: systemd `Restart=always` around `docker run --rm` on GCE (a plain `docker run --rm` alone would not come back). Clamped to 1..=331 with a warning. |
| `ADVERTISED_SECURE` | unset | `true` marks the room `secure` in the lobby. Informational: no client acts on it (browsers always use WebTransport over TLS). |
| `ALLOW_LOOPBACK_ANNOUNCE` | unset | `1`/`true` (dev only) turns the loopback refusal into a warning. |
| `TRON_BOT_DEBUG` | unset | Truthy: replicate bot decision snapshots for the client debug overlay (extra bandwidth; not for public servers). |
| `RUST_LOG` | `info` | Standard Bevy/tracing filter. |

Client (`tron-zero-client`): `MANAGER_URL` (runtime) / `TRONZERO_MANAGER_URL`
(build time; the release workflow bakes in the production lobby),
`TRON_CONFIG_FILE`, `TRON_AUTOCONNECT=host:port`, `TRON_SIM_LATENCY_MS`,
`TRON_SIM_JITTER_MS`, `TRON_SIM_LOSS_PCT`, `TRON_BOT_DEBUG`. Semantics are in
`docs/guides/dev.md`. Settings live in `client.json` under the platform config dir.

Manager (`server-manager/`): `PORT` (3001; `MANAGER_PORT` fallback), `HOST`,
`LOG_LEVEL`, `TRUST_PROXY`, `REGISTER_LIMIT_PER_MIN` and the ranked/store
settings; see its README. It runs on Cloud Run at
`https://server-manager.tronzero.dev`.

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
docker run --rm -p 5000:5000/udp -p 5001:5001/udp -e MANAGER_URL=... -e ADVERTISED_HOST=... tron-zero-server
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

## Client - WASM (web)

The browser client plays over WebTransport against rooms that announce a
`webPort` and `certHash` (see `docs/specs/web.md`); build, serve and deploy steps are
in `web/README.md`.

```
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version <wasm-bindgen version in Cargo.lock> --locked
./web/build.ps1              # or ./web/build.sh -> web/dist (serve with any static server)
```

`[profile.wasm-release]` is fat LTO, one codegen unit, `opt-level = 3` (tens of
minutes, lots of RAM); for iteration pass `-CargoProfile release` /
`CARGO_PROFILE=release`. `--cfg=web_sys_unstable_apis` and the getrandom
`wasm_js` cfg live in `.cargo/config.toml`. Browsers have no environment
variables: `platform::env_var` reads URL query parameters of the same name
(`?MANAGER_URL=...`) **only when the page is served from a local dev host**
(`localhost`, `*.localhost`, `127.0.0.1`, `[::1]`); on the production origin
they are ignored, so a crafted link cannot repoint the lobby or auto-join a
foreign server. There, settings come from the build-time `TRONZERO_MANAGER_URL`
and saved preferences (`localStorage`). The Quit button is hidden.

Bevy features: the workspace dependency has `default-features = false` and each
crate opts in. The client enables only 2D rendering (`2d_bevy_render`: camera,
gizmos, `Text2d`), `bevy_winit` and the default font; `bevy_egui` runs with
`render` only (no `bevy_ui`, picking, URL opening). No 3D, PBR, UI, audio,
gamepad or scene support is compiled in.

Size of `tron-zero-client_bg.wasm` (`wasm-release`, 2026-11-02):

| Build | raw | gzip -9 | brotli -q 11 |
|-------|-----|---------|--------------|
| Bevy default features, names kept | 57.5 MB | n/a | 8.7 MB |
| Trimmed bevy/egui features, names kept | 57.1 MB | 12.6 MB | 7.6 MB |
| + name/producers sections stripped (**current**) | 43.7 MB | 10.1 MB | 6.0 MB |
| + `wasm-opt -O3` (opt-in) | 39.1 MB | 9.9 MB | 6.0 MB |

Feature trimming alone barely moves the needle (Bevy's ECS monomorphisation
dominates the code); the debug `name` section was 11 MB of the raw file.
`wasm-opt -O3` (5 minutes) saves 10% raw but nothing on the wire, so it is
opt-in (`WASM_OPT=1` / `-WasmOpt`); frame time was not measurable in the pane.

## Release notes

- `rust-toolchain.toml` pins `stable` + `clippy`/`rustfmt` for every contributor; no per-machine drift.
- `Cargo.lock` is committed (workspace with binaries) — reproducible builds.
- Release profile uses Cargo defaults (`opt-level = 3`). Tune in `[profile.release]` only if profiling demands it.

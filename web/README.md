# Web client

The browser build of the game: `wasm32-unknown-unknown` + `wasm-bindgen`,
WebGL2 rendering, WebTransport to the server. Design: `docs/web.md`.

## Build

One-time setup:

```
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version <wasm-bindgen version in Cargo.lock> --locked
```

Then (the scripts do the same thing; both refuse a mismatched CLI version):

```
./web/build.ps1     # Windows (PowerShell 7)
./web/build.sh      # Linux / macOS / Git Bash; this is what CI runs
```

Output is `web/dist` (`index.html`, `tron-zero-client.js`,
`tron-zero-client_bg.wasm`). Set `TRONZERO_MANAGER_URL` to bake the lobby URL
into the build; without it the page falls back to `http://localhost:3001`
(override at runtime with `?MANAGER_URL=...`).

Fat LTO makes the `wasm-release` profile slow (tens of minutes, lots of RAM).
For iteration: `CARGO_PROFILE=release ./web/build.sh` (or
`-Profile release` with the PowerShell script).

`wasm-opt` is off by default (see `docs/building.md` for the measurement); opt in
with `WASM_OPT=1` / `-WasmOpt` (needs binaryen).

## Serve locally

Any static server works; the wasm needs `application/wasm`:

```
python -m http.server 8080 --directory web/dist
```

Open `http://localhost:8080/?MANAGER_URL=http://localhost:3001`. Query
parameters stand in for environment variables in the browser:

| Parameter | Meaning |
|-----------|---------|
| `MANAGER_URL` | Lobby base URL |
| `TRON_CERT_DIGEST` | Certificate hash of a local dev server, enables the pinned "Localhost" row (the server logs it at startup) |
| `TRON_AUTOCONNECT=ip:webport` | Join straight away (with `TRON_CERT_DIGEST`) |

The room list shows only rooms that announce a `webPort`. A room is joinable
from a browser when it also announces a `certHash` and an IP host.

## Browser notes

Verified in Chromium (Claude desktop browser pane): a room announcing
`127.0.0.1` with a self-signed `certHash` joins fine, so `ADVERTISED_HOST=127.0.0.1`
works for local testing. The server certificate is ECDSA P-256, <= 14 days, which
is what `serverCertificateHashes` requires. Session/browse/turn flow: join via the
room list, or `?TRON_AUTOCONNECT=127.0.0.1:5001&TRON_CERT_DIGEST=<hash>`.
Keyboard: arrows turn, Space/Enter respawns.

## Deploy

`.github/workflows/deploy.yml` job `client-web` builds on Linux with
`web/build.sh` and publishes `web/dist` to GitHub Pages (custom domain
`play.tronzero.dev`). One-time repository setup: Settings > Pages > Source:
GitHub Actions; custom domain `play.tronzero.dev`; DNS `CNAME play ->
<owner>.github.io`. Like the Windows job it builds the latest `client-v*` tag
(so `web/build.sh` must exist at that tag) and bakes `MANAGER_URL`.

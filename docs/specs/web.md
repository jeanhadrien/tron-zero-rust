# Web Client (WASM) and Browser Transport

Status: updated 2026-11-02. Design decisions for the browser build. The
desktop client (native UDP) is unchanged and stays the lowest-latency way to
play.

## Decisions

| Topic | Decision | Why |
|-------|----------|-----|
| Transport | **WebTransport datagrams** (Lightyear `webtransport`, Aeronet/xwt on wasm) | The closest a browser gets to UDP: QUIC over UDP, unreliable unordered datagrams, no head-of-line blocking. Lightyear's reliable channels sit on top exactly as they do on raw UDP. |
| Fallback | **None** (no WebSocket) | WebTransport has been Baseline since March 2026 (Chrome/Edge, Firefox 125+, Safari 26.4+ with cert hashes). A TCP fallback would mean head-of-line stalls on every lost packet, which the turn/ack design can't hide. Desktop is the answer for anything older. |
| Server listeners | **One Lightyear server entity, two transports**: raw UDP on `PORT` (5000) for desktop, WebTransport on `WEB_PORT` (5001, UDP) for browsers; `crates/server/src/web.rs` turns each WebTransport session into a `LinkOf` child of the same server | Same game, same simulation, same admission/session/turn code. Two server entities break Lightyear's `Replicate::to_clients` (it resolves a single started `Server`); QUIC can't share the raw socket. |
| Certificate | **Self-signed ECDSA P-256, ≤14 days, pinned by SHA-256** (`serverCertificateHashes`) by default; a real CA cert (`WEB_CERT`/`WEB_KEY`) when the server has a DNS name | Self-signed works for any IP host (friends hosting, a VM without DNS), and needs no ACME. Browsers only accept pinned certs that are ECDSA and valid ≤14 days, so the server restarts itself before expiry (`WEB_MAX_UPTIME_HOURS`) and re-announces the new hash. |
| Discovery | The manager's `RoomEntry` gains optional `webPort` and `certHash` (64 lowercase hex). The browser lists only rooms with a `webPort`; desktop keeps using `port`. | A room without `webPort` is native-only. A room with `webPort` but no `certHash` (server started with `WEB_CERT`/`WEB_KEY`) would need CA validation against a DNS name, but Lightyear's client only dials `https://<ip>:<port>`, so the browser lists such rooms disabled ("no certificate hash announced"). Same for a room whose host is a hostname instead of an IP: the browser dials a `SocketAddr`, so it is disabled with a message. |
| Renderer | **WebGL2** | The game draws a few thousand line segments; GPU is never the bottleneck. WebGL2 runs everywhere; WebGPU buys nothing here and costs a second code path. |
| Build | `wasm32-unknown-unknown`, `wasm-bindgen --target web` (name and producers sections stripped), profile `wasm-release` (fat LTO, `codegen-units = 1`, `opt-level = 3`); `wasm-opt -O3` is optional (`WASM_OPT=1`) | Speed over size. Size is paid once per visit, then cached; frame time is paid every frame. `wasm-opt` shrinks the raw file by about 10% but not the compressed transfer, and costs 5+ minutes. |
| Hosting | Static files: `web/dist` on GitHub Pages at `play.tronzero.dev` (free, CDN, gzip, correct `application/wasm`) | No server-side logic needed; no GCP load balancer (about $18/month just for a forwarding rule). |

## Performance model

The browser runs the same `simulate_players` at 120 Hz with prediction and
rollback. Where the time goes, in order:

1. **Rollback re-simulation.** On a mispredict the client re-runs up to
   RTT worth of ticks in one frame. wasm is single-threaded (no Bevy task
   pool on web without `SharedArrayBuffer` + COOP/COEP), so this cost lands on
   the main thread. Keep `simulate_players` allocation-free; it already uses
   `+ - * /` only, which compiles to straight wasm f32 ops (bit-identical to
   native, so prediction matches the server).
2. **Trail rendering.** Gizmo/mesh rebuilds per frame; same as native.
3. **egui.** Only the HUD while playing.

Not worth doing: WebGPU, wasm threads (would need cross-origin isolation
headers, which GitHub Pages can't set, for a sim that runs in microseconds per
tick), SIMD hand-tuning.

Worth doing: fat LTO, `panic = "abort"`, no debug asserts in
the release profile, no logging below `info` on web.

## Browser constraints and how they're handled

- **Lightyear `RawClient` + WebTransport** needs a placeholder
  `LocalAddr(0.0.0.0:0)` at spawn, or it never becomes `Connected`.
- **No UDP socket, no env vars, no filesystem.** Transport is WebTransport;
  configuration comes from the build-time `TRONZERO_MANAGER_URL` and saved
  preferences; preferences live in `localStorage`. URL query parameters
  (`MANAGER_URL`, `TRON_AUTOCONNECT`, `TRON_CERT_DIGEST`, `TRON_SIM_*`, debug
  flags) stand in for env vars **only on a local dev host** (`localhost`,
  `*.localhost`, `127.0.0.1`, `[::1]`); on the production origin they are
  ignored, because otherwise a crafted link could point the page at an
  attacker's lobby or auto-join an attacker's server.
- **No `std::time::Instant`.** Use `bevy::platform::time::Instant` (web-time
  backed) everywhere the client measures time.
- **No process exit.** The Quit button is hidden on web; closing the tab
  drops the session and the server's lease/idle timeout cleans up.
- **Background tabs.** `requestAnimationFrame` stops in a hidden tab, so the
  client stops ticking. QUIC keep-alives (1 s, 10 s idle timeout set by the
  server) run in the browser's network process, so the transport survives,
  but the game session lease expires without our app-level heartbeat.
  Observed with a throttled tab (1-3 fps): Lightyear aborts rollbacks longer
  than 100 ticks, so prediction goes stale and respawns never reconcile on the
  client. Whether the client recovers once the tab renders again is
  **unverified**; reloading the page is the reliable way back. Accepted for
  now: a backgrounded lightcycle is dead anyway.
- **Keyboard.** The canvas takes focus and the page prevents default on
  arrows/space so the page doesn't scroll. Chat text is read from the key
  events' `text` (no IME; mobile on-screen keyboards are untested).
- **Mixed content.** The page is HTTPS, the manager is HTTPS, WebTransport is
  HTTPS by construction. Nothing plain-HTTP is reachable from the page.

## Latency expectations

WebTransport adds QUIC framing and encryption (a few dozen bytes and
microseconds per packet) but no extra round trips once connected; it is not a
relay. Expect browser ping within a millisecond or two of desktop ping on the
same network. The handshake (one QUIC + one HTTP/3 CONNECT) happens once.

## Operations

- Ports: open **UDP 5000** (desktop) and **UDP 5001** (browser) on the host
  or firewall. Nothing TCP is needed for gameplay.
- A self-signed server must restart within 14 days. After
  `WEB_MAX_UPTIME_HOURS` (default 12 days) it exits at the first moment no
  human is connected, and at the hard limit (soft + 24 h, at most 332 h)
  regardless. The supervisor (on GCE: systemd `Restart=always` around
  `docker run --rm`) brings it back with a new certificate and hash, which it
  re-announces. The room advertises `webPort`/`certHash` only while the
  listener is open; `WEB_PORT` must differ from `PORT`.
- Build and serve the web client locally: see `web/README.md`.

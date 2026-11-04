# Ways to Run It

Status: updated 2026-11-02. One page per question: "I want to …, what do I
start?" Details live in the linked docs; this page only routes.

## The pieces

| Piece | Local | Production |
|-------|-------|------------|
| Game server | `cargo run -p tron-zero-server` (UDP 5000 desktop, UDP 5001 browser) | GCE VM `tron-zero-eu`, stopped by default (`deploy/gce/README.md`) |
| Lobby (server-manager) | `cd server-manager && bun run dev` (:3001) | `https://server-manager.tronzero.dev` (Cloud Run) |
| Desktop client | `cargo run -p tron-zero-client` | Windows zip on each `client-v*` GitHub Release |
| Web client | `web\build.ps1`, then any static server on `web/dist` | `https://play.tronzero.dev` (GitHub Pages) |

Two rules explain most surprises:

- **The server always announces to a lobby**: `MANAGER_URL`, default
  `http://localhost:3001`. With no lobby running it retries quietly; the game
  still works through direct connect.
- **The client picks its lobby** from `MANAGER_URL` (env var on desktop, URL
  parameter on a localhost page), else the one saved in settings, else the one
  baked into a CI build (prod), else `http://localhost:3001`. A lobby URL saved
  in the settings screen beats the build default, so check it when the room
  list looks wrong.

## Scenarios

### 1. Play against bots, desktop (the everyday loop)

```
play-local.bat            # or: play-local.bat release
```

Server with bots + a client that joins by itself with a simulated 20 ms ping.
No lobby involved. `set TRON_SIM_LATENCY_MS=0` first to remove the simulated
ping. Hot reload instead: `dev-watch.bat`.

### 2. Web client against a local server

```
web\build.ps1 -CargoProfile release          # once per client change, a few minutes
cargo run --release -p tron-zero-server      # copy cert_hash from the "WebTransport listener starting" line
python -m http.server 8080 --directory web/dist
```

Open in Chrome/Edge:
`http://localhost:8080/?TRON_AUTOCONNECT=127.0.0.1:5001&TRON_CERT_DIGEST=<cert_hash>`

The hash changes on every server restart. Desktop clients (scenario 1's
`cargo run -p tron-zero-client`) can join the same server at the same time.
Build options and URL parameters: `web/README.md`.

### 3. Full local stack, lobby included

```
cd server-manager && bun run dev             # terminal 1
cargo run -p tron-zero-server                # terminal 2, announces to localhost:3001 by default
cargo run -p tron-zero-client                # terminal 3, Browse Servers lists the room
```

For the web client, serve `web/dist` as in scenario 2 and open
`http://localhost:8080/?MANAGER_URL=http://localhost:3001`; the room is
joinable without copying the hash, because the server announces it.

### 4. Your PC's server on the production lobby

Only you can join it (it advertises `127.0.0.1`), but the room is visible to
everyone on the public list. Useful to test the prod lobby with local code:
the desktop client and `https://play.tronzero.dev` on this PC both list it
and can join it.

```
dev-cloud.bat "JH dev"
```

To let **friends** join a server on your PC instead: forward UDP 5000 and
5001 on your router to this PC, then start the server with
`MANAGER_URL=https://server-manager.tronzero.dev`,
`ADVERTISED_HOST=<your public IP>` and `SERVER_NAME=<name>` (no
`ALLOW_LOOPBACK_ANNOUNCE`). More in `docs/guides/release.md`.

### 5. The production server on GCP

```
gcloud compute instances start tron-zero-eu --zone europe-west1-b --project tron-zero-js
gcloud compute instances stop  tron-zero-eu --zone europe-west1-b --project tron-zero-js
```

It registers with the prod lobby about 30 s after boot; join from
`https://play.tronzero.dev` or the released desktop client. Stop it when done
(about $0.02/h running). Deploying new server code (Cloud Build, then VM
update): `deploy/gce/README.md`.

### 6. Shipping a release

Push `main`; release-please opens or updates the `chore: release main` PR.
Merging it tags `client-v*` and runs `.github/workflows/deploy.yml`: Windows
zip on the Release, web build to Pages. Rebuild without a new version:
`gh workflow run deploy.yml --ref main`. Details: `docs/guides/release.md`.

### 7. Without any window

- Mechanics, bots: `cargo test -p tron-zero-lab`, scenarios in `crates/lab/scenarios/` (`docs/guides/dev.md` § Lab).
- Netcode over real UDP/WebTransport: `cargo test -p tron-zero-nettest`.

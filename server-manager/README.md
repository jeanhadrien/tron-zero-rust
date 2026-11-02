# server-manager

Status: updated 2026-11-02.

Lobby service for Tron Zero: game servers register themselves, the client's
server browser lists them. In-memory only (rooms vanish on restart; servers
re-register when a heartbeat returns 404). Express 5 + `cors` on Bun.

```
bun install
bun run dev         # watch mode
bun run start
bun run typecheck
```

## Env vars

| Var | Default | Notes |
| --- | --- | --- |
| `PORT` | `3001` | Injected by Cloud Run. `MANAGER_PORT` is a fallback. |
| `HOST` | `0.0.0.0` | Bind address. |
| `LOG_LEVEL` | `info` | `debug`, `info`, `warn`, `error`, `none`. |
| `TRUST_PROXY` | `0` | Reverse-proxy hops to trust for the client IP (rate limiting). The Dockerfile sets `1` for Cloud Run; leave `0` when exposed directly, or `X-Forwarded-For` becomes spoofable. |
| `REGISTER_LIMIT_PER_MIN` | `20` | Max `POST /api/rooms` per IP per minute (`429` + `Retry-After` beyond). |

## API

All bodies are JSON. CORS is read-only (`GET`): browsers can list rooms but not mutate them.

### Authentication

Registering returns a per-room secret `token` (once; it is stored hashed and never listed).
Heartbeat and DELETE require `Authorization: Bearer <token>`.

| Endpoint | Body | Response |
| --- | --- | --- |
| `GET /health` | | `{ ok, uptime }` (seconds) |
| `GET /api/rooms` | | `RoomEntry[]` (public fields only, never a token) |
| `POST /api/rooms` | `{ host, port, displayName, maxPlayers, secure?, webPort?, certHash? }` | `RoomEntry` plus `token`; `400` bad payload; `409` host:port already registered and no/wrong token; `429` rate limited; `503` if 200 rooms are registered |
| `POST /api/rooms/:id/heartbeat` | `{ playerCount, botCount? }` + bearer token | `{ ok: true }`; `404` unknown/evicted; `401` missing/wrong token; `400` if a count is not a number |
| `DELETE /api/rooms/:id` | bearer token | `{ ok: true, existed }`; `401` missing/wrong token for a live room (an unknown id is still `200`, `existed: false`) |

`RoomEntry` = `{ id, host, port, secure?, webPort?, certHash?, displayName, playerCount, botCount?, maxPlayers, lastHeartbeat }`
(`lastHeartbeat` is epoch ms). `playerCount` is **humans only** and is capped by `maxPlayers`;
`botCount` is optional, informational and never counts toward capacity.

Behaviour:

- A room idle for more than 40s (30s interval + 10s grace) is evicted; scanned every 10s.
- Registering a `host:port` that is already registered succeeds only with that room's token (a
  restart that kept its token); the old id then 404s. Without it: `409`, so nobody can evict a
  live room. A server that crashed and lost its token gets `409` until its old entry is evicted
  (up to 40s) and the Rust announcer keeps retrying every 5s.
- Registration is rate limited per client IP (see `REGISTER_LIMIT_PER_MIN`).
- Validation: `host` non-empty string (max 253), `port` integer 1-65535, `webPort` (WebTransport UDP port for browsers) integer 1-65535 if present, `certHash` (SHA-256 of a self-signed WebTransport cert) 64 hex digits if present, stored lowercase, `maxPlayers` integer 1-10000, `displayName` non-empty string (truncated to 64 chars), `secure` boolean if present. Unknown fields are dropped.
- `playerCount` is truncated to an integer and clamped to `0..maxPlayers`.

## How the Rust side uses it

- Server (`crates/server/src/announce.rs`): `MANAGER_URL` (default `http://localhost:3001`) selects the manager. A detached thread registers (retrying every 5s), stores the returned `token`, then heartbeats every 2s with `playerCount` (humans only) and `botCount`, sending `Authorization: Bearer <token>`. On a heartbeat 404 it re-registers; on 401 it logs an error, waits 5s and re-registers; a register `409`/`429` is logged and retried. On SIGINT/SIGTERM it sends a best-effort `DELETE`, and a shutdown that lands during re-registration unregisters the new room too. It sends `ADVERTISED_HOST`, `ADVERTISED_PORT`, `SERVER_NAME`, `MAX_PLAYERS`, and `secure: true` only when `ADVERTISED_SECURE=true`. If `ADVERTISED_HOST` is loopback (the default `127.0.0.1`) while `MANAGER_URL` is not local, it **refuses to announce** and logs an error: nobody else could reach that address, and servers sharing the default endpoint would collide. The game itself still runs. Dev-only escape hatch: `ALLOW_LOOPBACK_ANNOUNCE=1` (`1` or `true`) downgrades the refusal to a warning and announces anyway (`dev-cloud.bat` sets it).
- Client (`crates/client/src/browser.rs`): `GET {MANAGER_URL}/api/rooms` (env at runtime, else the URL baked in via `TRONZERO_MANAGER_URL` at build time). It reads `id, host, port, displayName, playerCount, maxPlayers, lastHeartbeat` and optional `botCount`, and ignores `secure`. A row is Full when `playerCount >= maxPlayers` (humans only, matching server admission); bots are shown as `(+N bots)`.

## Docker / Cloud Run

Deployed as the Cloud Run service `server-manager` (project `tron-zero-js`, `europe-west1`),
mapped to `https://server-manager.tronzero.dev`. Rooms live in memory, so the service must
keep **max instances = 1** (two instances would split the room list); min instances is 0
(a live server's 2 s heartbeats keep the instance warm, and an empty list after a cold
start is correct, since servers re-register on 404). The Dockerfile sets `TRUST_PROXY=1`.

```
docker build -t server-manager .
docker run --rm -p 3001:3001 -e PORT=3001 server-manager
```

The build context is this folder only; Cloud Run injects `PORT`.

Deploy a new version (from this folder; about 30 s of Cloud Build):

```
TAG=rust-$(git rev-parse --short HEAD)
gcloud builds submit . --project tron-zero-js --region europe-west1 \
  --tag europe-west1-docker.pkg.dev/tron-zero-js/tron/server-manager:$TAG
gcloud run deploy server-manager --project tron-zero-js --region europe-west1 \
  --image europe-west1-docker.pkg.dev/tron-zero-js/tron/server-manager:$TAG \
  --min-instances 0 --max-instances 1 --memory 256Mi --cpu 1
```

Every room requires its token; the Rust announcer refuses a registration without one.

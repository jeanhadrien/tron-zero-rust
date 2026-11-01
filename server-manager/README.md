# server-manager

Lobby service for Tron Zero: game servers register themselves, the client's
server browser lists them. In-memory only (rooms vanish on restart; servers
re-register when a heartbeat returns 404). Express on Bun, no other deps.

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

## API

All bodies are JSON, CORS is open.

| Endpoint | Body | Response |
| --- | --- | --- |
| `GET /health` | | `{ ok, uptime }` (seconds) |
| `GET /api/rooms` | | `RoomEntry[]` |
| `POST /api/rooms` | `{ host, port, displayName, maxPlayers, secure? }` | the new `RoomEntry` (incl. `id`); `400` on bad payload; `503` if 200 rooms are registered |
| `POST /api/rooms/:id/heartbeat` | `{ playerCount }` | `{ ok: true }`; `404` if unknown/evicted; `400` if `playerCount` is not a number |
| `DELETE /api/rooms/:id` | | `{ ok: true, existed }` (always 200) |

`RoomEntry` = `{ id, host, port, secure?, displayName, playerCount, maxPlayers, lastHeartbeat }`
(`lastHeartbeat` is epoch ms).

Behaviour:

- A room idle for more than 40s (30s interval + 10s grace) is evicted; scanned every 10s.
- Registering the same `host:port` replaces the earlier room (restart dedupe); the old id then 404s.
- Validation: `host` non-empty string (max 253), `port` integer 1-65535, `maxPlayers` integer 1-10000, `displayName` non-empty string (truncated to 64 chars), `secure` boolean if present. Unknown fields are dropped.
- `playerCount` is truncated to an integer and clamped to `0..maxPlayers`.

## How the Rust side uses it

- Server (`crates/server/src/announce.rs`): `MANAGER_URL` (default `http://localhost:3001`) selects the manager. A detached thread registers (retrying every 5s), then heartbeats every 2s. On a heartbeat 404 it re-registers; other errors keep the id. On shutdown it sends a best-effort `DELETE`. It sends `ADVERTISED_HOST`, `ADVERTISED_PORT`, `SERVER_NAME`, `MAX_PLAYERS`, and `secure: true` only when `ADVERTISED_SECURE=true`.
- Client (`crates/client/src/browser.rs`): `GET {MANAGER_URL}/api/rooms` (env at runtime, else the URL baked in via `TRONZERO_MANAGER_URL` at build time). It reads `id, host, port, displayName, playerCount, maxPlayers, lastHeartbeat` and ignores `secure`.

## Docker / Cloud Run

```
docker build -t server-manager .
docker run --rm -p 3001:3001 -e PORT=3001 server-manager
```

The build context is this folder only; Cloud Run injects `PORT`.

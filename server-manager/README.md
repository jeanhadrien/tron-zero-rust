# server-manager

Status: updated 2026-11-08.

Lobby service for Tron Zero: game servers register themselves, the client's
server browser lists them. Rooms are in memory (they vanish on restart; servers
re-register when a heartbeat returns 404). Accounts, ratings, ranked keys and
join tickets live in memory by default or in Firestore (`STORE=firestore`, see
below). Express 5 + `cors` on Bun.

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
| `STORE` | memory | `firestore` keeps accounts, ratings, ranked keys and batch cursors in Firestore (needs Application Default Credentials and `roles/datastore.user`); otherwise everything is lost on restart. |
| `FIRESTORE_PROJECT` | from ADC | Project id for `STORE=firestore`. `FIRESTORE_EMULATOR_HOST` also works. |
| `RANKED_KEYS` | none | Memory store only: comma-separated `keyId:secret:official` (`1`/`0`). With Firestore, issue keys with `STORE=firestore bun scripts/add-key.ts <keyId> [official]`. |
| `IDENTITY_AUDIENCE` | none | Enables keyless ranked registration: the audience a game server's identity token must carry, i.e. the manager URL the servers use as `MANAGER_URL` (exactly, no trailing slash). Unset: identity tokens are refused. |
| `TRUSTED_IDENTITIES` | none | Memory store only: comma-separated `serviceAccountEmail:official` (`1`/`0`). With Firestore, trust one with `STORE=firestore bun scripts/add-identity.ts <email> [official]`. |
| `TICKET_SIGNING_KEY` | ephemeral | Ed25519 key that signs join tickets; unset generates a new one per start (tickets die with a restart). |
| `TICKET_VERIFY_KEYS` | none | Extra Ed25519 public keys (raw hex, comma-separated) that are advertised to game servers and accepted when verifying tickets, never used to sign. Rotation: deploy once listing the new key's public half (servers re-register on a manager restart and learn it), then deploy with `TICKET_SIGNING_KEY` switched to the new key and the old public key listed until its tickets expire (60 s). `bun scripts/rotation.ts` checks the verification side. |
| `WEB_ORIGINS` | none | Origins allowed to call `/api/accounts`, `/api/tickets` and `/api/me` from a browser. |

Firestore needs the two composite indexes in `firestore.indexes.json` (`firebase deploy --only firestore:indexes`, or create it from the console). `bun scripts/firestore-live.ts` exercises the store against a real database and cleans up after itself (set `GCLOUD_TOKEN=$(gcloud auth print-access-token)` if there are no Application Default Credentials).

## API

All bodies are JSON. CORS is read-only (`GET`) except on `/api/accounts`,
`/api/tickets`, `/api/me` and `/api/play`, which allow the origins in
`WEB_ORIGINS`: browsers can list rooms but not register or mutate them.

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

Accounts, tickets and ratings (see `docs/specs/matchmaking-elo.md`). Account endpoints use `Authorization: Bearer <account credential>`:

| Endpoint | Body | Response |
| --- | --- | --- |
| `POST /api/accounts` | `{ displayName? }` (10/min/IP) | `{ id, credential }`; the credential is shown once |
| `POST /api/tickets` | `{ roomId, displayName? }` (60/min/IP) | `{ ticket }` (base64) for a ranked room; `401` bad credential, `404` unknown or unranked room |
| `POST /api/play` | `{ exclude?, displayName? }` (30/min/IP) | `{ room, ticket }`: the manager places the caller in an official ranked room; `404` if none has a free slot |
| `GET /api/me` | | `{ id, displayName, rating, ratedEvents, rank }` (`rank` null until eligible); `404` without a rating |
| `GET /api/leaderboard` | `?limit=` (default 50, max 100) | `[{ rank, id, kind, displayName, rating }]` |
| `GET /api/keys` | | Ed25519 public keys for verifying tickets |
| `POST /api/rooms/:id/results` | elimination report + room bearer token | `{ ok, ... }`; `403` if the room is not ranked |

`RoomEntry` = `{ id, host, port, secure?, webPort?, certHash?, displayName, playerCount, botCount?, maxPlayers, lastHeartbeat }`
(`lastHeartbeat` is epoch ms). `playerCount` is **humans only** and is capped by `maxPlayers`;
`botCount` is optional, informational and never counts toward capacity.

Behaviour:

- A room idle for more than 40s (30s expected heartbeat interval + 10s grace) is evicted; scanned every 10s. The Rust server heartbeats every 2s.
- Registering a `host:port` that is already registered succeeds only with that room's token (a
  restart that kept its token); the old id then 404s. Without it: `409`, so nobody can evict a
  live room. A server that crashed and lost its token gets `409` until its old entry is evicted
  (up to 40s) and the Rust announcer keeps retrying every 5s.
- Registration is rate limited per client IP (see `REGISTER_LIMIT_PER_MIN`).
- Validation: `host` non-empty string (max 253), `port` integer 1-65535, `webPort` (WebTransport UDP port for browsers) integer 1-65535 if present, `certHash` (SHA-256 of a self-signed WebTransport cert) 64 hex digits if present, stored lowercase, `maxPlayers` integer 1-10000, `displayName` non-empty string (truncated to 64 chars), `secure` boolean if present. Unknown fields are dropped.
- `playerCount` is truncated to an integer and clamped to `0..maxPlayers`.

## How the Rust side uses it

- Server (`crates/server/src/announce.rs`): `MANAGER_URL` (default `http://localhost:3001`) selects the manager. A detached thread registers (retrying every 5s), stores the returned `token`, then heartbeats every 2s with `playerCount` (humans only) and `botCount`, sending `Authorization: Bearer <token>`. On a heartbeat 404 it re-registers; on 401 it logs an error, waits 5s and re-registers; a register `409`/`429` is logged and retried. On SIGINT/SIGTERM it sends a best-effort `DELETE`, and a shutdown that lands during re-registration unregisters the new room too. It sends `ADVERTISED_HOST`, `ADVERTISED_PORT`, the config file's `server.name` and `server.max_players`, and `secure: true` only when `ADVERTISED_SECURE=true`. With the WebTransport listener it adds `webPort`/`certHash`, but only while the listener is actually open: if it fails to bind (or closes later) the server re-registers without them, so browsers never see a room they cannot join. If `ADVERTISED_HOST` is loopback (the default `127.0.0.1`) while `MANAGER_URL` is not local, it **refuses to announce** and logs an error: nobody else could reach that address, and servers sharing the default endpoint would collide. The game itself still runs. Dev-only escape hatch: `ALLOW_LOOPBACK_ANNOUNCE=1` (`1` or `true`) downgrades the refusal to a warning and announces anyway (`scripts/dev-cloud.bat` sets it).
- Client (`crates/client/src/browser.rs`): `GET {MANAGER_URL}/api/rooms` (env at runtime on desktop, or the `MANAGER_URL` query parameter in the browser on a local dev host only; else the saved preference, else the URL baked in via `TRONZERO_MANAGER_URL` at build time). It reads `id, host, port, displayName, playerCount, maxPlayers, lastHeartbeat` and optional `botCount`, `webPort` and `certHash` (the browser lists only rooms with a `webPort`), and ignores `secure`. A row is Full when `playerCount >= maxPlayers` (humans only, matching server admission); bots are shown as `(+N bots)`.

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

### Ranked mode in production

Ratings, accounts and ranked keys must survive restarts, so the service runs with
`STORE=firestore` and a stable `TICKET_SIGNING_KEY` from Secret Manager. Room state
stays in memory (still max instances = 1). `scripts/setup-gcp.sh` does the one-time
setup (APIs, database, leaderboard index, `roles/datastore.user`, the secret) and
prints the deploy flags; it changes the project, so read it first. Then issue a key per
ranked game server with `scripts/add-key.ts` and start the server with `RANKED_KEY`.
`WEB_ORIGINS` must list the browser client's origin for sign-in from the web build.

A server registers with one of two credentials (sending both is a `400`); a credential that
is presented and fails is refused (`403`, reason in the manager log), never listed as a
plain room:

- **Ranked key** (`RANKED_KEY`): a secret from `add-key.ts`. Fits community servers.
- **Platform identity** (`RANKED_IDENTITY=gcp`): the server sends an ID token from the GCE
  metadata server and the manager checks Google's signature, `aud` = `IDENTITY_AUDIENCE`,
  a verified email, and that the email is in the `trustedIdentities` collection
  (`scripts/add-identity.ts`). No secret exists. If Google's keys cannot be fetched the
  manager answers `503` and the server retries.

`GET /api/rooms` states each room's tier (`ranked`, `official`). Only official rooms are
chosen by `/api/play`.


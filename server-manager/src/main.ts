import express from 'express';
import cors from 'cors';
import { randomUUID, randomBytes } from 'crypto';
import { Logger } from './logger';
import { registerRoom, removeRoom, getRoom, getAllRooms, evictStale, findByEndpoint, isAuthorized, isFull, MAX_ROOMS } from './roomStore';
import { rateLimit } from './rateLimit';
import type { RegisterPayload, RegisterResponse, RoomEntry } from './room';
import { authenticate, createAccount, sanitizeName } from './accounts';
import { checkRankedKey, forgetRankedRoom, getRankedRoom, markRanked, pruneRankedRooms, recordRoster } from './ranked';
import { issueTicket, publicKeys } from './tickets';
import { applyResults, parseResults } from './results';
import { store } from './store';
import { candidatesFor, forgetReservations, pickRoom, pruneReservations, reserve } from './placement';
import { humanRatingId } from './rating/pool';
import { RATING_CONFIG } from './rating/config';

const HEARTBEAT_INTERVAL = 30_000; // servers should heartbeat every 30s
const EVICT_GRACE = 10_000; // extra grace period => 40s idle eviction
const EVICT_SCAN_MS = 10_000; // scan interval

const MAX_HOST_LEN = 253;
const MAX_NAME_LEN = 64;
const MAX_PLAYERS_LIMIT = 10_000;
const MAX_BOTS_LIMIT = 1_000;

// Register attempts per IP per minute. A healthy server registers once; the Rust
// announcer retries every 5s at worst (12/min), so 20 leaves headroom.
const REGISTER_LIMIT = Number(process.env.REGISTER_LIMIT_PER_MIN) || 20;

const logger = new Logger('ServerManager');
const startedAt = Date.now();

const app = express();
// Behind Cloud Run / a load balancer req.ip is the proxy unless told how many hops
// to trust (the Dockerfile sets 1). Default 0: never trust X-Forwarded-For.
app.set('trust proxy', Number(process.env.TRUST_PROXY) || 0);
// Account routes are called by the web client with a bearer credential, so they
// get an origin allow-list (WEB_ORIGINS, comma-separated; empty = no browser
// origin). Registered before the global CORS so their preflight is answered here.
const webOrigins = (process.env.WEB_ORIGINS ?? '').split(',').map((o) => o.trim()).filter(Boolean);
const accountCors = cors({
  origin: webOrigins,
  methods: ['GET', 'POST'],
  allowedHeaders: ['Authorization', 'Content-Type'],
});
app.use(['/api/accounts', '/api/tickets', '/api/me'], accountCors);
// Everything else is read-only for browsers: they may list rooms but not mutate
// them (non-GET preflights fail). Game servers are not browsers and need no CORS.
app.use(cors({ methods: ['GET'] }));
// Result batches are larger than any other body; they get their own limit.
const smallJson = express.json({ limit: '16kb' });
const resultsJson = express.json({ limit: '256kb' });
app.use((req, res, next) => (req.path.endsWith('/results') ? resultsJson : smallJson)(req, res, next));

app.get('/health', (_req, res) => {
  res.json({ ok: true, uptime: Math.floor((Date.now() - startedAt) / 1000) });
});

// SHA-256 as 64 hex digits, no separators (what browsers' serverCertificateHashes take).
const CERT_HASH = /^[0-9a-fA-F]{64}$/;

const isInt = (v: unknown, min: number, max: number): v is number =>
  typeof v === 'number' && Number.isInteger(v) && v >= min && v <= max;

// Returns a clean payload or an error message. Unknown fields are dropped.
function parseRegister(raw: unknown): RegisterPayload | string {
  const b = (raw && typeof raw === 'object' ? raw : {}) as Record<string, unknown>;
  if (b.host == null || b.port == null || b.displayName == null || b.maxPlayers == null) {
    return 'Missing fields: host, port, displayName, maxPlayers';
  }
  if (typeof b.host !== 'string' || !b.host.trim() || b.host.length > MAX_HOST_LEN) return 'Invalid host';
  if (!isInt(b.port, 1, 65535)) return 'Invalid port';
  if (typeof b.displayName !== 'string' || !b.displayName.trim()) return 'Invalid displayName';
  if (!isInt(b.maxPlayers, 1, MAX_PLAYERS_LIMIT)) return 'Invalid maxPlayers';
  if (b.secure != null && typeof b.secure !== 'boolean') return 'Invalid secure';
  if (b.webPort != null && !isInt(b.webPort, 1, 65535)) return 'Invalid webPort';
  if (b.certHash != null && (typeof b.certHash !== 'string' || !CERT_HASH.test(b.certHash))) return 'Invalid certHash';
  if (b.rankedKey != null && (typeof b.rankedKey !== 'string' || b.rankedKey.length > 200)) return 'Invalid rankedKey';
  return {
    host: b.host,
    port: b.port,
    displayName: b.displayName.slice(0, MAX_NAME_LEN), // overlong names are truncated, not rejected
    maxPlayers: b.maxPlayers,
    ...(b.secure != null && { secure: b.secure }),
    ...(b.webPort != null && { webPort: b.webPort }),
    ...(typeof b.certHash === 'string' && { certHash: b.certHash.toLowerCase() }),
    ...(typeof b.rankedKey === 'string' && { rankedKey: b.rankedKey }),
  };
}

// `Authorization: Bearer <token>` => token, else undefined.
function bearerToken(header: string | undefined): string | undefined {
  return header?.match(/^Bearer\s+(\S+)\s*$/i)?.[1];
}

// Register a new game room. Re-registering a live host:port only works with that
// room's token (restart/re-register); otherwise 409, so nobody can evict a room.
app.post('/api/rooms', rateLimit(REGISTER_LIMIT, 60_000), async (req, res) => {
  const body = parseRegister(req.body);
  if (typeof body === 'string') {
    logger.warn('Register rejected:', body);
    res.status(400).json({ error: body });
    return;
  }
  const existing = findByEndpoint(body.host, body.port);
  if (existing && !isAuthorized(existing, bearerToken(req.header('authorization')))) {
    logger.warn(`Register conflict — ${body.host}:${body.port} is already registered (ip=${req.ip})`);
    res.status(409).json({ error: 'Endpoint already registered' });
    return;
  }
  if (isFull() && !existing) {
    logger.warn(`Room cap (${MAX_ROOMS}) reached`);
    res.status(503).json({ error: 'Room limit reached' });
    return;
  }

  // A ranked key is checked before anything is created, so a bad key leaves no room behind.
  const key = body.rankedKey ? await checkRankedKey(body.rankedKey) : undefined;
  if (key && !key.ok) {
    logger.warn(`Register rejected: ${key.reason} (ip=${req.ip})`);
    res.status(403).json({ error: key.reason });
    return;
  }

  const id = randomUUID();
  const token = randomBytes(32).toString('hex');
  const entry = registerRoom(body, id, token);
  if (key?.ok) {
    markRanked(id, key.keyId, key.official);
    entry.ranked = true;
    entry.official = key.official;
  }
  const response: RegisterResponse = { ...entry, token };
  if (key?.ok) {
    response.keyId = key.keyId;
    response.publicKeys = publicKeys();
  }
  logger.info(`Room registered — id=${id} name="${body.displayName}" host=${body.host}:${body.port} ranked=${!!key}`);
  res.json(response);
});

// Looks up the room and checks its token; sends the error response itself.
function authorizedRoom(req: express.Request<{ id: string }>, res: express.Response): RoomEntry | undefined {
  const room = getRoom(req.params.id);
  if (!room) {
    logger.warn(`Request for unknown room — id=${req.params.id}`);
    res.status(404).json({ error: 'Room not found' });
    return undefined;
  }
  if (!isAuthorized(room, bearerToken(req.header('authorization')))) {
    logger.warn(`Unauthorized request — id=${req.params.id} ip=${req.ip}`);
    res.status(401).json({ error: 'Invalid or missing room token' });
    return undefined;
  }
  return room.entry;
}

const isCount = (v: unknown): v is number => typeof v === 'number' && Number.isFinite(v);
const clampCount = (v: number, max: number): number => Math.min(Math.max(Math.trunc(v), 0), max);

const MAX_ROSTER = 1_000;

// Lenient on purpose: a malformed roster only means nobody is proven present.
function parseRoster(raw: unknown): { humans: string[]; tickets: string[] } | undefined {
  const r = (raw && typeof raw === 'object' ? raw : undefined) as Record<string, unknown> | undefined;
  const strings = (v: unknown): string[] =>
    Array.isArray(v) ? v.filter((s): s is string => typeof s === 'string').slice(0, MAX_ROSTER) : [];
  return r ? { humans: strings(r.humans), tickets: strings(r.tickets) } : undefined;
}

// Heartbeat from a game room
app.post('/api/rooms/:id/heartbeat', (req, res) => {
  const room = authorizedRoom(req, res);
  if (!room) return;

  const { playerCount, botCount } = (req.body ?? {}) as { playerCount?: unknown; botCount?: unknown };
  if (playerCount != null && !isCount(playerCount)) {
    res.status(400).json({ error: 'Invalid playerCount' });
    return;
  }
  if (botCount != null && !isCount(botCount)) {
    res.status(400).json({ error: 'Invalid botCount' });
    return;
  }
  const ranked = getRankedRoom(req.params.id);
  if (ranked) {
    const roster = parseRoster((req.body as { roster?: unknown } | undefined)?.roster);
    if (roster) recordRoster(ranked, roster);
  }
  if (playerCount != null) room.playerCount = clampCount(playerCount, room.maxPlayers);
  if (botCount != null) room.botCount = clampCount(botCount, MAX_BOTS_LIMIT);
  room.lastHeartbeat = Date.now();
  res.json({ ok: true });
});

// Graceful unregister on server shutdown. Unknown ids stay a harmless 200.
app.delete('/api/rooms/:id', (req, res) => {
  if (getRoom(req.params.id) && !authorizedRoom(req, res)) return;
  const existed = removeRoom(req.params.id);
  forgetRankedRoom(req.params.id);
  forgetReservations(req.params.id);
  logger.info(`Room unregistered — id=${req.params.id} existed=${existed}`);
  res.json({ ok: true, existed });
});

// Client server browser endpoint
app.get('/api/rooms', (_req, res) => {
  const rooms = getAllRooms();
  logger.debug(`Listed ${rooms.length} room(s)`);
  res.json(rooms);
});

// --- Accounts, tickets, ratings -------------------------------------------

// Anonymous account on first launch. The credential is shown once.
app.post('/api/accounts', rateLimit(10, 60_000), async (req, res) => {
  const displayName = sanitizeName((req.body as { displayName?: unknown } | undefined)?.displayName);
  const account = await createAccount(displayName);
  logger.info(`Account created — id=${account.id}`);
  res.json({ id: account.id, credential: account.credential });
});

// Join ticket for a ranked room. The latest display name travels with the request.
app.post('/api/tickets', rateLimit(60, 60_000), async (req, res) => {
  const account = await authenticate(bearerToken(req.header('authorization')));
  if (!account) {
    res.status(401).json({ error: 'Invalid or missing account credential' });
    return;
  }
  const body = (req.body ?? {}) as { roomId?: unknown; displayName?: unknown };
  const room = typeof body.roomId === 'string' ? getRoom(body.roomId) : undefined;
  const ranked = room && getRankedRoom(room.entry.id);
  if (!room || !ranked) {
    res.status(404).json({ error: 'Ranked room not found' });
    return;
  }
  const displayName = sanitizeName(body.displayName ?? account.displayName);
  if (displayName !== account.displayName) await store.setDisplayName(account.id, displayName);
  const ticket = issueTicket({ accountId: account.id, displayName, serverKeyId: ranked.keyId, roomId: room.entry.id });
  res.json({ ticket: ticket.toString('base64') });
});

// "Play now": the manager picks an official ranked room for the caller (spec:
// Placement) and returns it with a ticket. `exclude` lists rooms the client
// already failed to join. Always places someone when any official room has a slot.
app.post('/api/play', rateLimit(30, 60_000), async (req, res) => {
  const account = await authenticate(bearerToken(req.header('authorization')));
  if (!account) {
    res.status(401).json({ error: 'Invalid or missing account credential' });
    return;
  }
  const body = (req.body ?? {}) as { exclude?: unknown; displayName?: unknown };
  const exclude = new Set(Array.isArray(body.exclude) ? body.exclude.filter((v): v is string => typeof v === 'string') : []);
  const rating = (await store.rating(humanRatingId(account.id)))?.rating ?? RATING_CONFIG.startRating;
  const chosen = pickRoom(rating, await candidatesFor(getAllRooms(), exclude));
  const ranked = chosen && getRankedRoom(chosen.entry.id);
  if (!chosen || !ranked) {
    res.status(404).json({ error: 'No ranked server has a free slot right now' });
    return;
  }
  const displayName = sanitizeName(body.displayName ?? account.displayName);
  if (displayName !== account.displayName) await store.setDisplayName(account.id, displayName);
  reserve(chosen.entry.id, account.id, rating);
  const ticket = issueTicket({ accountId: account.id, displayName, serverKeyId: ranked.keyId, roomId: chosen.entry.id });
  logger.info(`Placed account ${account.id} (${Math.round(rating)}) in room ${chosen.entry.id}`);
  res.json({ room: chosen.entry, ticket: ticket.toString('base64') });
});

// Verification keys for game servers (they also get them at register).
app.get('/api/keys', (_req, res) => {
  res.json(publicKeys());
});

// The caller's own rating and rank.
app.get('/api/me', async (req, res) => {
  const account = await authenticate(bearerToken(req.header('authorization')));
  if (!account) {
    res.status(401).json({ error: 'Invalid or missing account credential' });
    return;
  }
  const rating = await store.rating(`acct:${account.id}`);
  if (!rating) {
    res.status(404).json({ error: 'No rating' });
    return;
  }
  const rank = rating.eligible ? (await store.countEligibleAbove(rating.rating)) + 1 : null;
  res.json({ id: rating.id, displayName: rating.displayName, rating: Math.round(rating.rating), ratedEvents: rating.ratedEvents, rank });
});

const MAX_LEADERBOARD = 100;

app.get('/api/leaderboard', async (req, res) => {
  const limit = Math.min(Math.max(Number(req.query.limit) || 50, 1), MAX_LEADERBOARD);
  const rows = await store.leaderboard(limit);
  res.json(rows.map((r, i) => ({ rank: i + 1, id: r.id, kind: r.kind, displayName: r.displayName, rating: Math.round(r.rating) })));
});

// Elimination results from a ranked room (room token auth, like heartbeats).
app.post('/api/rooms/:id/results', async (req, res) => {
  const room = authorizedRoom(req, res);
  if (!room) return;
  const ranked = getRankedRoom(req.params.id);
  if (!ranked) {
    res.status(403).json({ error: 'Room is not ranked' });
    return;
  }
  const body = parseResults(req.body);
  if (typeof body === 'string') {
    res.status(400).json({ error: body });
    return;
  }
  const outcome = await applyResults(store, ranked, body);
  if (!outcome.duplicate) logger.debug(`Results — room=${req.params.id} applied=${outcome.applied} skipped=${outcome.skipped}`);
  res.json({ ok: true, ...outcome });
});

// Malformed JSON bodies etc. => JSON error instead of an HTML stack page.
app.use((err: { status?: number; statusCode?: number }, _req: express.Request, res: express.Response, _next: express.NextFunction) => {
  const status = err.status ?? err.statusCode ?? 500;
  if (status >= 500) logger.error('Unhandled error', err);
  res.status(status).json({ error: status >= 500 ? 'Internal error' : 'Bad request' });
});

// Periodic eviction of stale rooms
const evictionInterval = setInterval(() => {
  const removed = evictStale(HEARTBEAT_INTERVAL + EVICT_GRACE);
  const liveIds = new Set(getAllRooms().map((room) => room.id));
  pruneRankedRooms(liveIds);
  pruneReservations(liveIds);
  if (removed > 0) {
    logger.warn(`Evicted ${removed} stale room(s)`);
  }
}, EVICT_SCAN_MS);

export function stopEviction(): void {
  clearInterval(evictionInterval);
}

export default app;

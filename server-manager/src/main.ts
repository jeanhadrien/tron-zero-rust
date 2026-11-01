import express from 'express';
import cors from 'cors';
import { randomUUID, randomBytes } from 'crypto';
import { Logger } from './logger';
import { registerRoom, removeRoom, getRoom, getAllRooms, evictStale, findByEndpoint, isAuthorized, isFull, MAX_ROOMS } from './roomStore';
import { rateLimit } from './rateLimit';
import type { RegisterPayload, RegisterResponse, RoomEntry } from './room';

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
// Read-only CORS: browsers may list rooms but not mutate them (non-GET preflights
// fail). Game servers are not browsers and need no CORS.
app.use(cors({ methods: ['GET'] }));
app.use(express.json({ limit: '16kb' }));

app.get('/health', (_req, res) => {
  res.json({ ok: true, uptime: Math.floor((Date.now() - startedAt) / 1000) });
});

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
  return {
    host: b.host,
    port: b.port,
    displayName: b.displayName.slice(0, MAX_NAME_LEN), // overlong names are truncated, not rejected
    maxPlayers: b.maxPlayers,
    ...(b.secure != null && { secure: b.secure }),
  };
}

// `Authorization: Bearer <token>` => token, else undefined.
function bearerToken(header: string | undefined): string | undefined {
  return header?.match(/^Bearer\s+(\S+)\s*$/i)?.[1];
}

// Register a new game room. Re-registering a live host:port only works with that
// room's token (restart/re-register); otherwise 409, so nobody can evict a room.
app.post('/api/rooms', rateLimit(REGISTER_LIMIT, 60_000), (req, res) => {
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

  const id = randomUUID();
  const token = randomBytes(32).toString('hex');
  const entry = registerRoom(body, id, token);
  logger.info(`Room registered — id=${id} name="${body.displayName}" host=${body.host}:${body.port}`);
  const response: RegisterResponse = { ...entry, token };
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
  if (playerCount != null) room.playerCount = clampCount(playerCount, room.maxPlayers);
  if (botCount != null) room.botCount = clampCount(botCount, MAX_BOTS_LIMIT);
  room.lastHeartbeat = Date.now();
  res.json({ ok: true });
});

// Graceful unregister on server shutdown. Unknown ids stay a harmless 200.
app.delete('/api/rooms/:id', (req, res) => {
  if (getRoom(req.params.id) && !authorizedRoom(req, res)) return;
  const existed = removeRoom(req.params.id);
  logger.info(`Room unregistered — id=${req.params.id} existed=${existed}`);
  res.json({ ok: true, existed });
});

// Client server browser endpoint
app.get('/api/rooms', (_req, res) => {
  const rooms = getAllRooms();
  logger.debug(`Listed ${rooms.length} room(s)`);
  res.json(rooms);
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
  if (removed > 0) {
    logger.warn(`Evicted ${removed} stale room(s)`);
  }
}, EVICT_SCAN_MS);

export function stopEviction(): void {
  clearInterval(evictionInterval);
}

export default app;

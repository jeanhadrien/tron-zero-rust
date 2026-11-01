import express from 'express';
import cors from 'cors';
import { randomUUID } from 'crypto';
import { Logger } from './logger';
import { registerRoom, removeRoom, getRoom, getAllRooms, evictStale, hasEndpoint, isFull, MAX_ROOMS } from './roomStore';
import type { RegisterPayload } from './room';

const HEARTBEAT_INTERVAL = 30_000; // servers should heartbeat every 30s
const EVICT_GRACE = 10_000; // extra grace period => 40s idle eviction
const EVICT_SCAN_MS = 10_000; // scan interval

const MAX_HOST_LEN = 253;
const MAX_NAME_LEN = 64;
const MAX_PLAYERS_LIMIT = 10_000;

const logger = new Logger('ServerManager');
const startedAt = Date.now();

const app = express();
app.use(cors());
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

// Register a new game room
app.post('/api/rooms', (req, res) => {
  const body = parseRegister(req.body);
  if (typeof body === 'string') {
    logger.warn('Register rejected:', body);
    res.status(400).json({ error: body });
    return;
  }
  if (isFull() && !hasEndpoint(body.host, body.port)) {
    logger.warn(`Room cap (${MAX_ROOMS}) reached`);
    res.status(503).json({ error: 'Room limit reached' });
    return;
  }

  const id = randomUUID();
  const entry = registerRoom(body, id);
  logger.info(`Room registered — id=${id} name="${body.displayName}" host=${body.host}:${body.port}`);
  res.json(entry);
});

// Heartbeat from a game room
app.post('/api/rooms/:id/heartbeat', (req, res) => {
  const room = getRoom(req.params.id);
  if (!room) {
    logger.warn(`Heartbeat for unknown room — id=${req.params.id}`);
    res.status(404).json({ error: 'Room not found' });
    return;
  }

  const playerCount = (req.body as { playerCount?: unknown } | undefined)?.playerCount;
  if (playerCount != null) {
    if (typeof playerCount !== 'number' || !Number.isFinite(playerCount)) {
      res.status(400).json({ error: 'Invalid playerCount' });
      return;
    }
    room.playerCount = Math.min(Math.max(Math.trunc(playerCount), 0), room.maxPlayers);
  }
  room.lastHeartbeat = Date.now();
  res.json({ ok: true });
});

// Graceful unregister on server shutdown
app.delete('/api/rooms/:id', (req, res) => {
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

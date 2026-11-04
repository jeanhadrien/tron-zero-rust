import { applyKill, applyMutual, applySuicide, type Change, type Rated } from './rating/elo';
import { isBotId, newBotRating, withChange } from './rating/pool';
import { isPresent, type RankedRoom } from './ranked';
import type { RatingDoc, Store, Tx } from './store/types';

/** One elimination as a game server reports it. Ids are rating ids. */
export type WireEvent =
  | { type: 'kill'; victim: string; killer: string }
  | { type: 'mutual'; a: string; b: string }
  | { type: 'suicide'; victim: string; alive: string[] };

export interface ResultsBody {
  /** Random per game-server process; `seq` restarts with it. */
  session: string;
  seq: number;
  events: WireEvent[];
}

export interface ApplyOutcome {
  /** The batch was already applied; nothing changed. */
  duplicate: boolean;
  applied: number;
  skipped: number;
}

export const MAX_EVENTS_PER_BATCH = 500;
const MAX_ALIVE = 64;
const MAX_ID_LEN = 100;

const isId = (v: unknown): v is string => typeof v === 'string' && v.length > 0 && v.length <= MAX_ID_LEN;

/** Shape-check a request body; returns an error message for a malformed one. */
export function parseResults(raw: unknown): ResultsBody | string {
  const b = (raw && typeof raw === 'object' ? raw : {}) as Record<string, unknown>;
  if (!isId(b.session)) return 'Invalid session';
  if (typeof b.seq !== 'number' || !Number.isInteger(b.seq) || b.seq < 0) return 'Invalid seq';
  if (!Array.isArray(b.events) || b.events.length > MAX_EVENTS_PER_BATCH) return 'Invalid events';
  const events: WireEvent[] = [];
  for (const e of b.events as Record<string, unknown>[]) {
    if (e?.type === 'kill' && isId(e.victim) && isId(e.killer)) {
      events.push({ type: 'kill', victim: e.victim, killer: e.killer });
    } else if (e?.type === 'mutual' && isId(e.a) && isId(e.b)) {
      events.push({ type: 'mutual', a: e.a, b: e.b });
    } else if (
      e?.type === 'suicide' &&
      isId(e.victim) &&
      Array.isArray(e.alive) &&
      e.alive.length <= MAX_ALIVE &&
      e.alive.every(isId)
    ) {
      events.push({ type: 'suicide', victim: e.victim, alive: e.alive });
    } else {
      return 'Invalid event';
    }
  }
  return { session: b.session, seq: b.seq, events };
}

/**
 * Apply a batch from a ranked room, atomically with its cursor. Events a
 * ranked key may not move are skipped one by one:
 *  - humans must be proven present on this server;
 *  - bots move only on official servers, and an event touching a bot on a
 *    community server is dropped for both sides, never applied one-sidedly.
 */
export async function applyResults(
  store: Store,
  room: RankedRoom,
  body: ResultsBody,
  now = Date.now(),
): Promise<ApplyOutcome> {
  return store.transaction(async (tx) => {
    const cursor = await tx.cursor(room.keyId);
    if (cursor && cursor.session === body.session && body.seq <= cursor.seq) {
      return { duplicate: true, applied: 0, skipped: 0 };
    }

    const known = new Map<string, RatingDoc>();
    // A rider's row, created on first sight for bots; undefined if it may not move.
    const load = async (id: string): Promise<RatingDoc | undefined> => {
      const cached = known.get(id);
      if (cached) return cached;
      let doc = await tx.rating(id);
      if (!doc && isBotId(id)) doc = newBotRating(id);
      if (!doc) return undefined;
      known.set(id, doc);
      return doc;
    };
    const movable = (id: string): boolean =>
      isBotId(id) ? room.official : isPresent(room, id, now);
    const rated = async (id: string): Promise<Rated | undefined> => {
      if (!movable(id)) return undefined;
      return load(id);
    };

    let applied = 0;
    let skipped = 0;
    for (const event of body.events) {
      const changes = await changesFor(event, rated, room.official);
      // No changes covers a self-kill, a same-bot duel and a suicide with nobody else alive.
      if (!changes || changes.length === 0) {
        skipped++;
        continue;
      }
      commit(changes, known, tx);
      applied++;
    }
    tx.putCursor(room.keyId, { session: body.session, seq: body.seq });
    return { duplicate: false, applied, skipped };
  });
}

async function changesFor(
  event: WireEvent,
  rated: (id: string) => Promise<Rated | undefined>,
  official: boolean,
): Promise<Change[] | undefined> {
  switch (event.type) {
    case 'kill': {
      const [killer, victim] = [await rated(event.killer), await rated(event.victim)];
      // Both sides must be movable, or neither moves.
      return killer && victim ? applyKill(killer, victim) : undefined;
    }
    case 'mutual': {
      const [a, b] = [await rated(event.a), await rated(event.b)];
      return a && b ? applyMutual(a, b) : undefined;
    }
    case 'suicide': {
      const victim = await rated(event.victim);
      if (!victim) return undefined;
      const field: Rated[] = [];
      for (const id of event.alive) {
        // Riders that may not move are not part of the field either.
        if (id === event.victim || (isBotId(id) && !official)) continue;
        const other = await rated(id);
        if (other) field.push(other);
      }
      return applySuicide(victim, field);
    }
  }
}

/** Write changes back to the transaction and keep the working copies current. */
function commit(changes: Change[], known: Map<string, RatingDoc>, tx: Tx): void {
  for (const change of changes) {
    const doc = known.get(change.id);
    if (!doc) continue;
    const next = withChange(doc, change.delta, change.countsAsEvent);
    known.set(change.id, next);
    tx.putRating(next);
  }
}

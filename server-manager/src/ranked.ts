import { timingSafeEqual } from 'node:crypto';
import { sha256Hex, store } from './store';
import { verifyTicket } from './tickets';

/** How long after its last heartbeat mention a player still counts as present. */
export const PRESENCE_GRACE_MS = 60_000;
/**
 * Oldest ticket accepted as proof of presence. A server resends the tickets of
 * its current players after re-registering, so this spans a long session.
 */
const PROOF_MAX_AGE_MS = 6 * 60 * 60 * 1000;

/** What the manager knows about a ranked room beyond its public entry. */
export interface RankedRoom {
  /** Stable id of the ranked key: survives the room re-registering. */
  keyId: string;
  official: boolean;
  /** Accounts this server proved it admitted (it holds a ticket we signed). */
  proven: Set<string>;
  /** Proven accounts last reported on the roster, by `acct:` rating id. */
  lastSeen: Map<string, number>;
}

const rankedRooms = new Map<string, RankedRoom>();

export const getRankedRoom = (roomId: string): RankedRoom | undefined => rankedRooms.get(roomId);

export function forgetRankedRoom(roomId: string): void {
  rankedRooms.delete(roomId);
}

/** Drop state of rooms that no longer exist. */
export function pruneRankedRooms(liveRoomIds: Set<string>): void {
  for (const id of rankedRooms.keys()) if (!liveRoomIds.has(id)) rankedRooms.delete(id);
}

export type KeyCheck =
  | { ok: true; keyId: string; official: boolean }
  | { ok: false; reason: string; /** The check could not run (retry later), as opposed to failing. */ transient?: boolean };

/** Validate a presented `<keyId>.<secret>` against the stored ranked keys. */
export async function checkRankedKey(presented: string): Promise<KeyCheck> {
  const [id, secret] = presented.split('.');
  const key = id && secret ? await store.rankedKey(id) : undefined;
  if (!key || key.revoked) return { ok: false, reason: 'Unknown or revoked ranked key' };
  const given = Buffer.from(sha256Hex(secret), 'hex');
  const stored = Buffer.from(key.secretHash, 'hex');
  if (given.length !== stored.length || !timingSafeEqual(given, stored)) {
    return { ok: false, reason: 'Unknown or revoked ranked key' };
  }
  return { ok: true, keyId: key.id, official: key.official };
}

export function markRanked(roomId: string, keyId: string, official: boolean): void {
  rankedRooms.set(roomId, { keyId, official, proven: new Set(), lastSeen: new Map() });
}

export interface RosterReport {
  /** Account ids (without the `acct:` prefix) currently connected. */
  humans: string[];
  /** Base64 tickets of players admitted since the last report. */
  tickets: string[];
}

/**
 * Fold a heartbeat roster into the room's presence. A ticket we signed for
 * this room's key proves the server admitted that account; only proven
 * accounts count as present, so a server cannot name players it never had.
 */
export function recordRoster(room: RankedRoom, roster: RosterReport, now = Date.now()): void {
  for (const encoded of roster.tickets) {
    const payload = verifyTicket(Buffer.from(encoded, 'base64'));
    if (payload && payload.serverKeyId === room.keyId && now - payload.iat <= PROOF_MAX_AGE_MS) {
      room.proven.add(payload.accountId);
    }
  }
  for (const id of roster.humans) {
    if (room.proven.has(id)) room.lastSeen.set(`acct:${id}`, now);
  }
}

/** Whether `ratingId` (an `acct:` id) is a proven player seen recently. */
export function isPresent(room: RankedRoom, ratingId: string, now = Date.now()): boolean {
  const seen = room.lastSeen.get(ratingId);
  return seen !== undefined && now - seen <= PRESENCE_GRACE_MS;
}

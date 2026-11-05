import { expected } from './rating/elo';
import { RATING_CONFIG } from './rating/config';
import { getRankedRoom, isPresent } from './ranked';
import { store } from './store';
import type { RoomEntry } from './room';

/** Tuning of `/api/play` (spec: "Placement"). */
export const PLACEMENT = {
  /** Riders (humans and bots) a server is happiest at. */
  targetRiders: 6,
  /** Weight of the population term next to unfairness (which lies in 0..0.5). */
  populationWeight: 0.1,
  /** Unfairness assumed for a server with nobody to compare against. */
  neutralUnfairness: 0.25,
  /** How long a placed player holds a slot while connecting. */
  reservationTtlMs: 30_000,
};

interface Reservation {
  accountId: string;
  rating: number;
  expiresAt: number;
}

/** Players sent to a room who have not shown up on its roster yet. */
const reservations = new Map<string, Reservation[]>();

/** Live reservations of a room: expired ones and ones that connected are dropped. */
function liveReservations(roomId: string, now: number): Reservation[] {
  const room = getRankedRoom(roomId);
  const live = (reservations.get(roomId) ?? []).filter(
    (r) => r.expiresAt > now && !(room && isPresent(room, `acct:${r.accountId}`, now)),
  );
  if (live.length > 0) reservations.set(roomId, live);
  else reservations.delete(roomId);
  return live;
}

export function reserve(roomId: string, accountId: string, rating: number, now = Date.now()): void {
  const others = liveReservations(roomId, now).filter((r) => r.accountId !== accountId);
  reservations.set(roomId, [...others, { accountId, rating, expiresAt: now + PLACEMENT.reservationTtlMs }]);
}

export function forgetReservations(roomId: string): void {
  reservations.delete(roomId);
}

/** Drop reservations of rooms that no longer exist. */
export function pruneReservations(liveRoomIds: Set<string>): void {
  for (const id of reservations.keys()) if (!liveRoomIds.has(id)) reservations.delete(id);
}

/** Mean `|E(player, rider) - 0.5|` over the roster; undefined when it is empty. */
export function unfairness(player: number, riders: number[]): number | undefined {
  if (riders.length === 0) return undefined;
  const total = riders.reduce((sum, rating) => sum + Math.abs(expected(player, rating) - 0.5), 0);
  return total / riders.length;
}

export interface Candidate {
  entry: RoomEntry;
  /** Ratings of the humans we can see (present or reserved). */
  humanRatings: number[];
  /** Everybody counted against capacity and population: humans, bots, reservations. */
  riders: number;
}

/** Lower is better: unfairness plus distance from the target population. */
export function score(player: number, candidate: Candidate): number {
  const u = unfairness(player, candidate.humanRatings) ?? PLACEMENT.neutralUnfairness;
  const crowd = Math.abs(candidate.riders + 1 - PLACEMENT.targetRiders) / PLACEMENT.targetRiders;
  return u + PLACEMENT.populationWeight * crowd;
}

/** The best candidate for `player`; ties go to the fuller server so players concentrate. */
export function pickRoom(player: number, candidates: Candidate[]): Candidate | undefined {
  let best: Candidate | undefined;
  let bestScore = Infinity;
  for (const candidate of candidates) {
    const s = score(player, candidate);
    if (s < bestScore - 1e-9 || (Math.abs(s - bestScore) <= 1e-9 && best && candidate.riders > best.riders)) {
      best = candidate;
      bestScore = s;
    }
  }
  return best;
}

/**
 * Official ranked rooms with a free slot, with the humans we can see on them.
 * Bots are not on the manager's roster, so they count toward population only.
 */
export async function candidatesFor(
  rooms: RoomEntry[],
  exclude: ReadonlySet<string>,
  now = Date.now(),
): Promise<Candidate[]> {
  const out: Candidate[] = [];
  for (const entry of rooms) {
    const ranked = getRankedRoom(entry.id);
    if (!ranked?.official || exclude.has(entry.id)) continue;
    const held = liveReservations(entry.id, now);
    if (entry.maxPlayers > 0 && entry.playerCount + held.length >= entry.maxPlayers) continue;
    const present = [...ranked.lastSeen.keys()].filter((id) => isPresent(ranked, id, now));
    const docs = await Promise.all(present.map((id) => store.rating(id)));
    const humanRatings = [
      ...docs.map((doc) => doc?.rating ?? RATING_CONFIG.startRating),
      ...held.map((r) => r.rating),
    ];
    out.push({ entry, humanRatings, riders: entry.playerCount + (entry.botCount ?? 0) + held.length });
  }
  return out;
}

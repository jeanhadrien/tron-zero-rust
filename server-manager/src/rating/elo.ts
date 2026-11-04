import { RATING_CONFIG as C } from './config.js';

/** What the rating maths needs to know about a rider. */
export interface Rated {
  id: string;
  rating: number;
  ratedEvents: number;
}

/**
 * One rider's outcome of an elimination. `countsAsEvent` is false for riders
 * who only receive a share of a suicide loss: they do not leave the
 * provisional K phase by merely surviving.
 */
export interface Change {
  id: string;
  delta: number;
  countsAsEvent: boolean;
}

/** Expected score of `a` against `b`. */
export function expected(ra: number, rb: number): number {
  return 1 / (1 + 10 ** ((rb - ra) / C.scale));
}

/** Per-player K: large while provisional, small once settled. */
export function kFor(ratedEvents: number): number {
  return ratedEvents < C.settledAfterEvents ? C.kProvisional : C.kSettled;
}

/** Whether the player is settled enough to appear on the leaderboard. */
export function isEligible(ratedEvents: number): boolean {
  return ratedEvents >= C.settledAfterEvents;
}

/** Each side moves by K * (S - E) against the other, with its own K. */
function duel(a: Rated, b: Rated, scoreA: number): Change[] {
  const ea = expected(a.rating, b.rating);
  const eb = 1 - ea;
  return [
    { id: a.id, delta: kFor(a.ratedEvents) * (scoreA - ea), countsAsEvent: true },
    { id: b.id, delta: kFor(b.ratedEvents) * (1 - scoreA - eb), countsAsEvent: true },
  ];
}

/**
 * `victim` died on `killer`'s trail. Equal ids (two instances of one bot, or
 * a bogus self-kill) carry no information and change nothing.
 */
export function applyKill(killer: Rated, victim: Rated): Change[] {
  if (killer.id === victim.id) return [];
  return duel(killer, victim, 1);
}

/** `a` and `b` died on each other's trail on the same tick. */
export function applyMutual(a: Rated, b: Rated): Change[] {
  if (a.id === b.id) return [];
  return duel(a, b, 0.5);
}

/**
 * `victim` died on a wall or its own trail. It is compared with the field
 * (mean rating of the riders alive); the loss is shared among them equally.
 * Nothing changes when nobody else is alive.
 */
export function applySuicide(victim: Rated, alive: Rated[]): Change[] {
  const others = alive.filter((r) => r.id !== victim.id);
  if (others.length === 0) return [];
  const field = others.reduce((sum, r) => sum + r.rating, 0) / others.length;
  const loss = C.suicideFactor * kFor(victim.ratedEvents) * expected(victim.rating, field);
  const share = loss / others.length;
  return [
    { id: victim.id, delta: -loss, countsAsEvent: true },
    ...others.map((r) => ({ id: r.id, delta: share, countsAsEvent: false })),
  ];
}

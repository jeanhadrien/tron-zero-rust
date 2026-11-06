import { RATING_CONFIG } from './config.js';
import { isEligible } from './elo.js';
import type { RatingDoc } from '../store/types';

export { isBotId } from './elo.js';

/** Rating row id of a human account. */
export const humanRatingId = (accountId: string): string => `acct:${accountId}`;

/** Skill knob range of the bots crate (`ELO_ANCHORS` ends) and its rating remap. */
const SKILL_RANGE = [500, 4000] as const;
const INITIAL_RATING_RANGE = [800, 2500] as const;

/**
 * First rating of a bot identity (`bot:<personality>:<skill>`): the skill
 * knob remapped linearly onto the rating scale, as `Elo::initial_rating` in
 * `crates/bots` does. Measurement corrects it over time.
 */
export function initialBotRating(id: string): number {
  const skill = Number(id.split(':')[2]);
  if (!Number.isFinite(skill)) return RATING_CONFIG.startRating;
  const [lo, hi] = SKILL_RANGE;
  const [from, to] = INITIAL_RATING_RANGE;
  const t = Math.min(1, Math.max(0, (skill - lo) / (hi - lo)));
  return Math.round(from + (to - from) * t);
}

export function newHumanRating(accountId: string, displayName: string): RatingDoc {
  return {
    id: humanRatingId(accountId),
    kind: 'human',
    displayName,
    rating: RATING_CONFIG.startRating,
    ratedEvents: 0,
    eligible: false,
  };
}

export function newBotRating(id: string): RatingDoc {
  return {
    id,
    kind: 'bot',
    displayName: id.slice('bot:'.length).replace(':', ' '),
    rating: initialBotRating(id),
    ratedEvents: 0,
    eligible: false,
  };
}

/** Apply one rating change to a row, keeping the eligibility flag in step. */
export function withChange(doc: RatingDoc, delta: number, countsAsEvent: boolean): RatingDoc {
  const ratedEvents = doc.ratedEvents + (countsAsEvent ? 1 : 0);
  return { ...doc, rating: doc.rating + delta, ratedEvents, eligible: isEligible(ratedEvents) };
}

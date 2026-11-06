import { RATING_CONFIG } from './config.js';
import { isEligible } from './elo.js';
import type { RatingDoc } from '../store/types';

export { isBotId } from './elo.js';

/** Rating row id of a human account. */
export const humanRatingId = (accountId: string): string => `acct:${accountId}`;

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

/** Bot ids are `bot:<name>:<fingerprint>`; the leaderboard shows the name. */
const botDisplayName = (id: string): string => id.split(':')[1] || id;

/** A bot identity starts like any newcomer. */
export function newBotRating(id: string): RatingDoc {
  return {
    id,
    kind: 'bot',
    displayName: botDisplayName(id),
    rating: RATING_CONFIG.startRating,
    ratedEvents: 0,
    eligible: false,
  };
}

/** Apply one rating change to a row, keeping the eligibility flag in step. */
export function withChange(doc: RatingDoc, delta: number, countsAsEvent: boolean): RatingDoc {
  const ratedEvents = doc.ratedEvents + (countsAsEvent ? 1 : 0);
  return { ...doc, rating: doc.rating + delta, ratedEvents, eligible: isEligible(ratedEvents) };
}

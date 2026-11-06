// Tuning constants for the rating system. Starting values from
// docs/specs/matchmaking-elo.md; to be tuned with the lab validation run.

export const RATING_CONFIG = {
  /** Rating every new account starts at. */
  startRating: 1500,
  /** Elo scale: a 400-point gap means ~10:1 expected odds. */
  scale: 400,
  /** Human K while still provisional. High on purpose (MVP): a little play moves far. */
  kProvisional: 40,
  /** Human K once settled. */
  kSettled: 16,
  /** Rated events after which a human's K drops from provisional to settled. */
  settledAfterEvents: 20,
  /** Bot K while provisional. Bots are the stable reference: kept at the original values. */
  kBotProvisional: 16,
  /** Bot K once settled. */
  kBotSettled: 6,
  /** Rated events after which a bot's K drops. */
  botSettledAfterEvents: 100,
  /** Rated events after which the leaderboard lists a rider (low for the MVP). */
  leaderboardMinEvents: 3,
  /** A suicide is weaker evidence than a loss to an opponent. */
  suicideFactor: 0.33,
} as const;

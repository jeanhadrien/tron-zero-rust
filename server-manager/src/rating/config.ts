// Tuning constants for the rating system. Starting values from
// docs/specs/matchmaking-elo.md; to be tuned with the lab validation run.

export const RATING_CONFIG = {
  /** Rating every new account starts at. */
  startRating: 1500,
  /** Elo scale: a 400-point gap means ~10:1 expected odds. */
  scale: 400,
  /** K while a player is still provisional. */
  kProvisional: 16,
  /** K once a player has settled. */
  kSettled: 6,
  /** Rated events after which K drops and the leaderboard lists the player. */
  settledAfterEvents: 100,
  /** A suicide is weaker evidence than a loss to an opponent. */
  suicideFactor: 0.5,
} as const;

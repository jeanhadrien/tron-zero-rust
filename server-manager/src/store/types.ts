// Storage seam for accounts, ratings and ranked keys. The in-memory
// implementation backs local development and tests; Firestore is the planned
// production implementation of the same interface.

/** What a rating row is for. Humans are `acct:<uuid>`, bots `bot:<name>:<fingerprint>`. */
export interface RatingDoc {
  id: string;
  kind: 'human' | 'bot';
  displayName: string;
  rating: number;
  ratedEvents: number;
  /** True once `ratedEvents` reaches the leaderboard threshold. */
  eligible: boolean;
}

export interface AccountDoc {
  id: string;
  /** SHA-256 (hex) of the account secret. The secret itself is never stored. */
  secretHash: string;
  displayName: string;
  createdAt: number;
}

/** An operator-issued credential that lets a game server report results. */
export interface RankedKeyDoc {
  id: string;
  /** SHA-256 (hex) of the key secret. */
  secretHash: string;
  /** Official servers may move bot ratings; community servers may not. */
  official: boolean;
  revoked: boolean;
}

/**
 * A platform identity (the verified email of a cloud service account) allowed to
 * register as a ranked server, an alternative to a ranked key.
 */
export interface TrustedIdentityDoc {
  /** The service account email. */
  id: string;
  official: boolean;
  revoked: boolean;
}

/** Last accepted result batch of one ranked key, for idempotent retries. */
export interface BatchCursor {
  /** Random per game-server process: a restart resets `seq` without a clash. */
  session: string;
  seq: number;
}

/** Reads and writes inside one atomic rating update. */
export interface Tx {
  rating(id: string): Promise<RatingDoc | undefined>;
  putRating(doc: RatingDoc): void;
  cursor(keyId: string): Promise<BatchCursor | undefined>;
  putCursor(keyId: string, cursor: BatchCursor): void;
}

export interface Store {
  /** Run `fn` atomically; writes become visible only when it resolves. */
  transaction<T>(fn: (tx: Tx) => Promise<T>): Promise<T>;
  createAccount(account: AccountDoc, rating: RatingDoc): Promise<void>;
  account(id: string): Promise<AccountDoc | undefined>;
  rating(id: string): Promise<RatingDoc | undefined>;
  /** Keep the account's and its rating row's display name in step. */
  setDisplayName(accountId: string, displayName: string): Promise<void>;
  rankedKey(id: string): Promise<RankedKeyDoc | undefined>;
  trustedIdentity(email: string): Promise<TrustedIdentityDoc | undefined>;
  /** Eligible players, best first. */
  leaderboard(limit: number): Promise<RatingDoc[]>;
  /** Eligible players rated strictly above `rating`. */
  countEligibleAbove(rating: number): Promise<number>;
}

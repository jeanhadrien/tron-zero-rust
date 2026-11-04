import type { AccountDoc, BatchCursor, RankedKeyDoc, RatingDoc, Store, Tx } from './types';

/** Process-local store. Everything is lost on restart. */
export class MemoryStore implements Store {
  private accounts = new Map<string, AccountDoc>();
  private ratings = new Map<string, RatingDoc>();
  private keys = new Map<string, RankedKeyDoc>();
  private cursors = new Map<string, BatchCursor>();
  /** Serializes transactions, like a single writer would. */
  private queue: Promise<unknown> = Promise.resolve();

  constructor(rankedKeys: RankedKeyDoc[] = []) {
    for (const key of rankedKeys) this.keys.set(key.id, key);
  }

  transaction<T>(fn: (tx: Tx) => Promise<T>): Promise<T> {
    const run = async (): Promise<T> => {
      const ratingWrites = new Map<string, RatingDoc>();
      const cursorWrites = new Map<string, BatchCursor>();
      const tx: Tx = {
        rating: async (id) => ratingWrites.get(id) ?? this.ratings.get(id),
        putRating: (doc) => void ratingWrites.set(doc.id, { ...doc }),
        cursor: async (keyId) => cursorWrites.get(keyId) ?? this.cursors.get(keyId),
        putCursor: (keyId, cursor) => void cursorWrites.set(keyId, { ...cursor }),
      };
      const result = await fn(tx);
      // Commit only after fn succeeded: a throw leaves no partial update.
      for (const [id, doc] of ratingWrites) this.ratings.set(id, doc);
      for (const [id, cursor] of cursorWrites) this.cursors.set(id, cursor);
      return result;
    };
    const next = this.queue.then(run, run);
    this.queue = next.catch(() => undefined);
    return next;
  }

  async createAccount(account: AccountDoc, rating: RatingDoc): Promise<void> {
    this.accounts.set(account.id, account);
    this.ratings.set(rating.id, rating);
  }

  async account(id: string) {
    return this.accounts.get(id);
  }

  async rating(id: string) {
    return this.ratings.get(id);
  }

  async setDisplayName(accountId: string, displayName: string) {
    const account = this.accounts.get(accountId);
    if (account) account.displayName = displayName;
    const rating = this.ratings.get(`acct:${accountId}`);
    if (rating) rating.displayName = displayName;
  }

  async rankedKey(id: string) {
    return this.keys.get(id);
  }

  async leaderboard(limit: number) {
    return [...this.ratings.values()]
      .filter((r) => r.eligible)
      .sort((a, b) => b.rating - a.rating)
      .slice(0, limit);
  }

  async countEligibleAbove(rating: number) {
    let n = 0;
    for (const r of this.ratings.values()) if (r.eligible && r.rating > rating) n++;
    return n;
  }
}

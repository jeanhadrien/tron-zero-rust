import { Firestore, type DocumentReference } from '@google-cloud/firestore';
import type { AccountDoc, BatchCursor, RankedKeyDoc, RatingDoc, Store, TrustedIdentityDoc, Tx } from './types';

/** How long a leaderboard page is served from memory. */
const LEADERBOARD_CACHE_MS = 30_000;

/**
 * Firestore-backed store. Collections:
 * - `accounts/{id}`        public part of an account (display name, created)
 * - `accountSecrets/{id}`  secret hash, kept apart so reads of the above never load it
 * - `ratings/{id}`         humans and bots alike; the leaderboard queries it
 * - `rankedKeys/{id}`      operator-issued keys (see `scripts/add-key.ts`)
 * - `cursors/{keyId}`      last accepted result batch of a key
 *
 * The leaderboard query needs the composite index in `firestore.indexes.json`.
 * REST is preferred over gRPC, which is unreliable under Bun. Setting
 * `FIRESTORE_EMULATOR_HOST` points the client at an emulator.
 */
export class FirestoreStore implements Store {
  private db: Firestore;
  /** Serializes rating transactions in this process to keep contention low. */
  private queue: Promise<unknown> = Promise.resolve();
  private board: { limit: number; at: number; rows: RatingDoc[] } | undefined;

  constructor(db: Firestore = new Firestore({ projectId: process.env.FIRESTORE_PROJECT || undefined, preferRest: true })) {
    this.db = db;
  }

  private ref(collection: string, id: string): DocumentReference {
    return this.db.collection(collection).doc(id);
  }

  transaction<T>(fn: (tx: Tx) => Promise<T>): Promise<T> {
    const run = (): Promise<T> =>
      this.db.runTransaction(async (t) => {
        // Fresh buffers per attempt: Firestore re-runs this callback on contention.
        const ratingWrites = new Map<string, RatingDoc>();
        const cursorWrites = new Map<string, BatchCursor>();
        const tx: Tx = {
          rating: async (id) => ratingWrites.get(id) ?? ((await t.get(this.ref('ratings', id))).data() as RatingDoc | undefined),
          putRating: (doc) => void ratingWrites.set(doc.id, { ...doc }),
          cursor: async (keyId) => cursorWrites.get(keyId) ?? ((await t.get(this.ref('cursors', keyId))).data() as BatchCursor | undefined),
          putCursor: (keyId, cursor) => void cursorWrites.set(keyId, { ...cursor }),
        };
        const result = await fn(tx);
        // Every read is done; writes go last, as Firestore requires.
        for (const [id, doc] of ratingWrites) t.set(this.ref('ratings', id), doc);
        for (const [id, cursor] of cursorWrites) t.set(this.ref('cursors', id), cursor);
        return result;
      });
    const next = this.queue.then(run, run);
    this.queue = next.catch(() => undefined);
    return next;
  }

  async createAccount(account: AccountDoc, rating: RatingDoc): Promise<void> {
    const batch = this.db.batch();
    batch.create(this.ref('accounts', account.id), { displayName: account.displayName, createdAt: account.createdAt });
    batch.create(this.ref('accountSecrets', account.id), { secretHash: account.secretHash });
    batch.create(this.ref('ratings', rating.id), rating);
    await batch.commit();
  }

  async account(id: string): Promise<AccountDoc | undefined> {
    const [pub, secret] = await this.db.getAll(this.ref('accounts', id), this.ref('accountSecrets', id));
    if (!pub.exists || !secret.exists) return undefined;
    const { displayName, createdAt } = pub.data() as Pick<AccountDoc, 'displayName' | 'createdAt'>;
    const { secretHash } = secret.data() as Pick<AccountDoc, 'secretHash'>;
    return { id, displayName, createdAt, secretHash };
  }

  async rating(id: string): Promise<RatingDoc | undefined> {
    return (await this.ref('ratings', id).get()).data() as RatingDoc | undefined;
  }

  async setDisplayName(accountId: string, displayName: string): Promise<void> {
    const account = this.ref('accounts', accountId);
    const rating = this.ref('ratings', `acct:${accountId}`);
    await this.db.runTransaction(async (t) => {
      const [a, r] = await Promise.all([t.get(account), t.get(rating)]);
      if (a.exists) t.update(account, { displayName });
      if (r.exists) t.update(rating, { displayName });
    });
    this.board = undefined;
  }

  async rankedKey(id: string): Promise<RankedKeyDoc | undefined> {
    const doc = await this.ref('rankedKeys', id).get();
    return doc.exists ? ({ ...(doc.data() as Omit<RankedKeyDoc, 'id'>), id } as RankedKeyDoc) : undefined;
  }

  async trustedIdentity(email: string): Promise<TrustedIdentityDoc | undefined> {
    const doc = await this.ref('trustedIdentities', email).get();
    return doc.exists ? ({ ...(doc.data() as Omit<TrustedIdentityDoc, 'id'>), id: email } as TrustedIdentityDoc) : undefined;
  }

  async leaderboard(limit: number): Promise<RatingDoc[]> {
    const now = Date.now();
    if (this.board && this.board.limit >= limit && now - this.board.at < LEADERBOARD_CACHE_MS) {
      return this.board.rows.slice(0, limit);
    }
    const snapshot = await this.db.collection('ratings').where('eligible', '==', true).orderBy('rating', 'desc').limit(limit).get();
    const rows = snapshot.docs.map((doc) => doc.data() as RatingDoc);
    this.board = { limit, at: now, rows };
    return rows;
  }

  async countEligibleAbove(rating: number): Promise<number> {
    const query = this.db.collection('ratings').where('eligible', '==', true).where('rating', '>', rating);
    return (await query.count().get()).data().count;
  }
}

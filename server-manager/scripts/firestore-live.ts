// Exercises FirestoreStore against a real database (or emulator) and removes
// what it wrote. `FIRESTORE_PROJECT=<id> bun scripts/firestore-live.ts`
// Credentials: Application Default Credentials, or `GCLOUD_TOKEN=$(gcloud auth print-access-token)`.
import { Firestore } from '@google-cloud/firestore';
import { OAuth2Client } from 'google-auth-library';
import { FirestoreStore } from '../src/store/firestore';
import type { RatingDoc } from '../src/store/types';

const project = process.env.FIRESTORE_PROJECT || undefined;
const authClient = process.env.GCLOUD_TOKEN ? new OAuth2Client() : undefined;
authClient?.setCredentials({ access_token: process.env.GCLOUD_TOKEN });
const db = new Firestore({ projectId: project, preferRest: true, ...(authClient ? { authClient } : {}) });
const store = new FirestoreStore(db);
const run = `livetest-${Date.now()}`;
const ids = { a: `${run}-a`, b: `${run}-b` };
const ratingOf = (id: string, rating: number, eligible: boolean): RatingDoc => ({ id, kind: 'human', displayName: id, rating, ratedEvents: 1, eligible });

const check = (name: string, ok: boolean, extra?: unknown) => {
  console.log(ok ? 'ok  ' : 'FAIL', name, ok ? '' : JSON.stringify(extra));
  if (!ok) process.exitCode = 1;
};

try {
  const secretHash = 'ab'.repeat(32);
  await store.createAccount({ id: ids.a, secretHash, displayName: 'LiveA', createdAt: 1 }, ratingOf(`acct:${ids.a}`, 9_000_001, true));
  await store.createAccount({ id: ids.b, secretHash, displayName: 'LiveB', createdAt: 1 }, ratingOf(`acct:${ids.b}`, 9_000_000, true));
  const account = await store.account(ids.a);
  check('account round trip', account?.displayName === 'LiveA' && account.secretHash === secretHash, account);
  check('missing account', (await store.account(`${run}-none`)) === undefined);

  const out = await store.transaction(async (tx) => {
    const a = await tx.rating(`acct:${ids.a}`);
    tx.putRating({ ...a!, rating: 9_000_005 });
    tx.putCursor(run, { session: 's', seq: 1 });
    return 'done';
  });
  check('transaction commits rating and cursor', out === 'done' && (await store.rating(`acct:${ids.a}`))?.rating === 9_000_005);

  await store.transaction(async (tx) => {
    tx.putRating(ratingOf(`acct:${ids.a}`, 1, false));
    throw new Error('boom');
  }).catch(() => undefined);
  check('a throwing transaction leaves no write', (await store.rating(`acct:${ids.a}`))?.rating === 9_000_005);

  await store.setDisplayName(ids.a, 'Renamed');
  check('display name follows to the rating row', (await store.account(ids.a))?.displayName === 'Renamed' && (await store.rating(`acct:${ids.a}`))?.displayName === 'Renamed');

  const board = await store.leaderboard(2);
  check('leaderboard is best first (needs the composite index)', board[0]?.id === `acct:${ids.a}` && board[1]?.id === `acct:${ids.b}`, board.map((r) => r.id));
  check('rank counts eligible players above', (await store.countEligibleAbove(9_000_000)) === 1);
  check('unknown ranked key', (await store.rankedKey(`${run}-nokey`)) === undefined);
} finally {
  for (const [collection, id] of [
    ['accounts', ids.a], ['accounts', ids.b], ['accountSecrets', ids.a], ['accountSecrets', ids.b],
    ['ratings', `acct:${ids.a}`], ['ratings', `acct:${ids.b}`], ['cursors', run],
  ]) await db.collection(collection).doc(id).delete().catch(() => undefined);
}
process.exit(process.exitCode ?? 0);

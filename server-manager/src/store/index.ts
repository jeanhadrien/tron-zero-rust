import { createHash } from 'node:crypto';
import { Logger } from '../logger';
import { MemoryStore } from './memory';
import type { RankedKeyDoc, Store, TrustedIdentityDoc } from './types';

const logger = new Logger('Store');

export const sha256Hex = (value: string): string => createHash('sha256').update(value).digest('hex');

/**
 * Ranked keys for the in-memory store, from `RANKED_KEYS`:
 * comma-separated `keyId:secret:official` (official = 1 or 0).
 * A key is presented by a game server as `keyId.secret`.
 */
function rankedKeysFromEnv(): RankedKeyDoc[] {
  return (process.env.RANKED_KEYS ?? '')
    .split(',')
    .map((entry) => entry.trim())
    .filter(Boolean)
    .map((entry) => {
      const [id, secret, official] = entry.split(':');
      return { id, secretHash: sha256Hex(secret ?? ''), official: official === '1', revoked: false };
    });
}

/**
 * Trusted platform identities for the in-memory store, from `TRUSTED_IDENTITIES`:
 * comma-separated `serviceAccountEmail:official` (official = 1 or 0).
 */
function trustedIdentitiesFromEnv(): TrustedIdentityDoc[] {
  return (process.env.TRUSTED_IDENTITIES ?? '')
    .split(',')
    .map((entry) => entry.trim())
    .filter(Boolean)
    .map((entry) => {
      const [id, official] = entry.split(':');
      return { id, official: official === '1', revoked: false };
    });
}

// `STORE=firestore` selects Firestore (keys live in its `rankedKeys` collection,
// see scripts/add-key.ts; identities in `trustedIdentities`, see
// scripts/add-identity.ts); otherwise everything is in memory and lost on restart.
async function openStore(): Promise<Store> {
  if (process.env.STORE === 'firestore') {
    const { FirestoreStore } = await import('./firestore');
    logger.info('Using Firestore');
    return new FirestoreStore();
  }
  logger.info('Using the in-memory store: ratings are lost on restart');
  return new MemoryStore(rankedKeysFromEnv(), trustedIdentitiesFromEnv());
}

export const store: Store = await openStore();

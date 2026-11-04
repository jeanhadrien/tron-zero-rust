import { createHash } from 'node:crypto';
import { Logger } from '../logger';
import { MemoryStore } from './memory';
import type { RankedKeyDoc, Store } from './types';

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

// The Firestore implementation of `Store` plugs in here (selected by env).
export const store: Store = new MemoryStore(rankedKeysFromEnv());
logger.info('Using the in-memory store: ratings are lost on restart');

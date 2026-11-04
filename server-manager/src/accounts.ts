import { randomBytes, randomUUID, timingSafeEqual } from 'node:crypto';
import { newHumanRating } from './rating/pool';
import { sha256Hex, store } from './store';
import type { AccountDoc } from './store/types';

const MAX_NAME_LEN = 24;

/** Same rules as the game server's name sanitizing: printable, trimmed, bounded. */
export function sanitizeName(raw: unknown): string {
  const text = typeof raw === 'string' ? raw : '';
  const clean = text.replace(/[\u0000-\u001f\u007f]/g, '').trim().slice(0, MAX_NAME_LEN);
  return clean || 'Player';
}

export interface CreatedAccount {
  id: string;
  /** `<id>.<secret>`: the only credential, shown once. */
  credential: string;
}

export async function createAccount(displayName: string): Promise<CreatedAccount> {
  const id = randomUUID();
  const secret = randomBytes(32).toString('hex');
  const account: AccountDoc = { id, secretHash: sha256Hex(secret), displayName, createdAt: Date.now() };
  await store.createAccount(account, newHumanRating(id, displayName));
  return { id, credential: `${id}.${secret}` };
}

/** The account a `Bearer <id>.<secret>` credential belongs to, if it is genuine. */
export async function authenticate(credential: string | undefined): Promise<AccountDoc | undefined> {
  const [id, secret] = credential?.split('.') ?? [];
  if (!id || !secret) return undefined;
  const account = await store.account(id);
  if (!account) return undefined;
  const given = Buffer.from(sha256Hex(secret), 'hex');
  const stored = Buffer.from(account.secretHash, 'hex');
  return given.length === stored.length && timingSafeEqual(given, stored) ? account : undefined;
}

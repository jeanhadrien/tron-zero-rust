import { createHash, createPrivateKey, createPublicKey, generateKeyPairSync, randomBytes, sign, verify } from 'node:crypto';
import type { KeyObject } from 'node:crypto';
import { Logger } from './logger';

const logger = new Logger('Tickets');

/** Tickets are short-lived: long enough for a join, short enough to limit theft. */
export const TICKET_TTL_MS = 60_000;
/** Allowed clock difference between manager and game server. */
export const TICKET_SKEW_MS = 30_000;
const SIGNATURE_LEN = 64;

export interface TicketPayload {
  v: 1;
  /** Which signing key made this ticket. */
  kid: string;
  accountId: string;
  displayName: string;
  /** Stable id of the ranked key of the server this ticket is for. */
  serverKeyId: string;
  /** Room at issue time; only a hint, rooms re-register under new ids. */
  roomId: string;
  iat: number;
  exp: number;
  nonce: string;
}

interface SigningKey {
  kid: string;
  privateKey: KeyObject;
  publicKey: KeyObject;
  /** Raw 32-byte Ed25519 public key, hex. */
  publicKeyHex: string;
}

function signingKeyFrom(privateKey: KeyObject): SigningKey {
  const publicKey = createPublicKey(privateKey);
  const x = publicKey.export({ format: 'jwk' }).x;
  if (!x) throw new Error('ed25519 public key has no x coordinate');
  const publicKeyHex = Buffer.from(x, 'base64url').toString('hex');
  const kid = createHash('sha256').update(publicKeyHex).digest('hex').slice(0, 8);
  return { kid, privateKey, publicKey, publicKeyHex };
}

/** The signing key: `TICKET_SIGNING_KEY` (PKCS8 PEM) or an ephemeral one for dev. */
function loadSigningKey(): SigningKey {
  const pem = process.env.TICKET_SIGNING_KEY;
  if (pem) return signingKeyFrom(createPrivateKey(pem.replace(/\\n/g, '\n')));
  logger.warn('TICKET_SIGNING_KEY not set: using an ephemeral key, tickets die with this process');
  return signingKeyFrom(generateKeyPairSync('ed25519').privateKey);
}

const current = loadSigningKey();

/** A verification-only key: one that signs tickets elsewhere or used to. */
interface VerifyKey {
  kid: string;
  publicKey: KeyObject;
  publicKeyHex: string;
}

const kidOf = (publicKeyHex: string): string => createHash('sha256').update(publicKeyHex).digest('hex').slice(0, 8);

/** Raw 32-byte Ed25519 public key (hex) as a verification key. */
function verifyKeyFromHex(publicKeyHex: string): VerifyKey {
  const x = Buffer.from(publicKeyHex, 'hex').toString('base64url');
  const publicKey = createPublicKey({ key: { kty: 'OKP', crv: 'Ed25519', x }, format: 'jwk' });
  return { kid: kidOf(publicKeyHex), publicKey, publicKeyHex };
}

/**
 * Extra public keys from `TICKET_VERIFY_KEYS` (comma-separated hex). They are
 * advertised to game servers and accepted when verifying, but never sign.
 * Rotating the signing key takes two deploys: first list the new key's public
 * half here (servers re-register on a manager restart and so learn it), then
 * switch `TICKET_SIGNING_KEY` to the new key and list the old public key here
 * until its last tickets have expired.
 */
const extraKeys: VerifyKey[] = (process.env.TICKET_VERIFY_KEYS ?? '')
  .split(',')
  .map((k) => k.trim())
  .filter(Boolean)
  .map(verifyKeyFromHex);

const verifyKeys: VerifyKey[] = [current, ...extraKeys.filter((k) => k.kid !== current.kid)];

/** Public keys game servers verify tickets with: the signing key, then the extras. */
export function publicKeys(): { kid: string; publicKey: string }[] {
  return verifyKeys.map((k) => ({ kid: k.kid, publicKey: k.publicKeyHex }));
}

/**
 * A signed ticket: the UTF-8 JSON payload followed by its 64-byte Ed25519
 * signature. Signing the exact bytes means verifiers never canonicalize.
 */
export function issueTicket(
  fields: Pick<TicketPayload, 'accountId' | 'displayName' | 'serverKeyId' | 'roomId'>,
  now = Date.now(),
): Buffer {
  const payload: TicketPayload = {
    v: 1,
    kid: current.kid,
    ...fields,
    iat: now,
    exp: now + TICKET_TTL_MS,
    nonce: randomBytes(12).toString('hex'),
  };
  const body = Buffer.from(JSON.stringify(payload), 'utf8');
  return Buffer.concat([body, sign(null, body, current.privateKey)]);
}

/** The payload of a ticket signed by one of our keys, or undefined if none did. */
export function verifyTicket(ticket: Buffer): TicketPayload | undefined {
  if (ticket.length <= SIGNATURE_LEN) return undefined;
  const body = ticket.subarray(0, ticket.length - SIGNATURE_LEN);
  const signature = ticket.subarray(ticket.length - SIGNATURE_LEN);
  try {
    const payload = JSON.parse(body.toString('utf8')) as TicketPayload;
    const key = verifyKeys.find((k) => k.kid === payload.kid);
    // The kid only picks the key; nothing is trusted until the signature holds.
    if (!key || !verify(null, body, key.publicKey, signature)) return undefined;
    return payload.v === 1 ? payload : undefined;
  } catch {
    return undefined;
  }
}

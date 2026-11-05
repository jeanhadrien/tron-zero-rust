// Key rotation check: `bun scripts/rotation.ts`. A ticket signed by a key listed
// in TICKET_VERIFY_KEYS verifies and is advertised; one from an unknown key does not.
import { createHash, generateKeyPairSync, sign } from 'node:crypto';
import type { KeyObject } from 'node:crypto';

const hexOf = (key: KeyObject): string => Buffer.from(key.export({ format: 'jwk' }).x!, 'base64url').toString('hex');
const next = generateKeyPairSync('ed25519');
const stranger = generateKeyPairSync('ed25519');
const old = generateKeyPairSync('ed25519');
process.env.TICKET_SIGNING_KEY = old.privateKey.export({ type: 'pkcs8', format: 'pem' }).toString();
process.env.TICKET_VERIFY_KEYS = hexOf(next.publicKey);

const { issueTicket, verifyTicket, publicKeys } = await import('../src/tickets');
const check = (name: string, ok: boolean) => {
  console.log(ok ? 'ok  ' : 'FAIL', name);
  if (!ok) process.exitCode = 1;
};
const signedBy = (privateKey: KeyObject, publicKey: KeyObject): Buffer => {
  const kid = createHash('sha256').update(hexOf(publicKey)).digest('hex').slice(0, 8);
  const now = Date.now();
  const payload = { v: 1, kid, accountId: 'u1', displayName: 'Neo', serverKeyId: 'k1', roomId: 'r', iat: now, exp: now + 60_000, nonce: 'n' };
  const body = Buffer.from(JSON.stringify(payload));
  return Buffer.concat([body, sign(null, body, privateKey)]);
};

check('both keys are advertised, signing key first', publicKeys().length === 2 && publicKeys()[0].publicKey === hexOf(old.publicKey) && publicKeys()[1].publicKey === hexOf(next.publicKey));
check('a ticket from the current key verifies', !!verifyTicket(issueTicket({ accountId: 'u1', displayName: 'Neo', serverKeyId: 'k1', roomId: 'r' })));
check('a ticket from the next key verifies', !!verifyTicket(signedBy(next.privateKey, next.publicKey)));
check('a ticket from an unlisted key is refused', !verifyTicket(signedBy(stranger.privateKey, stranger.publicKey)));
const forged = signedBy(next.privateKey, next.publicKey);
forged[20] ^= 1;
check('a tampered ticket is refused', !verifyTicket(forged));

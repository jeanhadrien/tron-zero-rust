// Issue a ranked key in Firestore: `STORE=firestore bun scripts/add-key.ts <keyId> [official]`.
// Prints the key a game server presents as RANKED_KEY. The secret is shown once
// and only its hash is stored.
import { randomBytes } from 'node:crypto';
import { Firestore } from '@google-cloud/firestore';
import { sha256Hex } from '../src/store';

const [id, official] = process.argv.slice(2);
if (!id || id.includes('.') || id.includes(':')) {
  console.error('usage: bun scripts/add-key.ts <keyId> [official]   (keyId must not contain "." or ":")');
  process.exit(1);
}
const secret = randomBytes(24).toString('hex');
const db = new Firestore({ projectId: process.env.FIRESTORE_PROJECT || undefined, preferRest: true });
await db.collection('rankedKeys').doc(id).create({ secretHash: sha256Hex(secret), official: official === 'official', revoked: false });
console.log(`RANKED_KEY=${id}.${secret}`);

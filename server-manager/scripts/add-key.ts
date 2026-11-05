// Issue a ranked key in Firestore: `STORE=firestore bun scripts/add-key.ts <keyId> [official]`.
// Prints the key a game server presents as RANKED_KEY. The secret is shown once
// and only its hash is stored.
import { randomBytes } from 'node:crypto';
import { Firestore } from '@google-cloud/firestore';
import { OAuth2Client } from 'google-auth-library';
import { sha256Hex } from '../src/store';

const [id, official] = process.argv.slice(2);
if (!id || id.includes('.') || id.includes(':')) {
  console.error('usage: bun scripts/add-key.ts <keyId> [official]   (keyId must not contain "." or ":")');
  process.exit(1);
}
const secret = randomBytes(24).toString('hex');
// Credentials: Application Default Credentials, or GCLOUD_TOKEN=$(gcloud auth print-access-token).
const authClient = process.env.GCLOUD_TOKEN ? new OAuth2Client() : undefined;
authClient?.setCredentials({ access_token: process.env.GCLOUD_TOKEN });
const db = new Firestore({ projectId: process.env.FIRESTORE_PROJECT || undefined, preferRest: true, ...(authClient ? { authClient } : {}) });
await db.collection('rankedKeys').doc(id).create({ secretHash: sha256Hex(secret), official: official === 'official', revoked: false });
console.log(`RANKED_KEY=${id}.${secret}`);

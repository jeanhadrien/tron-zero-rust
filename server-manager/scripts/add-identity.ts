// Trust a cloud service account as a ranked game server in Firestore:
//   STORE=firestore bun scripts/add-identity.ts <serviceAccountEmail> [official]
// Game servers running as that account then register ranked with no secret at all
// (see deploy/gce/README.md). Revoke by setting `revoked: true` on the document.
import { Firestore } from '@google-cloud/firestore';
import { OAuth2Client } from 'google-auth-library';

const [email, official] = process.argv.slice(2);
if (!email?.includes('@') || email.includes('/')) {
  console.error('usage: bun scripts/add-identity.ts <serviceAccountEmail> [official]');
  process.exit(1);
}
// Credentials: Application Default Credentials, or GCLOUD_TOKEN=$(gcloud auth print-access-token).
const authClient = process.env.GCLOUD_TOKEN ? new OAuth2Client() : undefined;
authClient?.setCredentials({ access_token: process.env.GCLOUD_TOKEN });
const db = new Firestore({ projectId: process.env.FIRESTORE_PROJECT || undefined, preferRest: true, ...(authClient ? { authClient } : {}) });
await db.collection('trustedIdentities').doc(email).set({ official: official === 'official', revoked: false });
console.log(`Trusted ${email} (${official === 'official' ? 'official' : 'community'})`);
